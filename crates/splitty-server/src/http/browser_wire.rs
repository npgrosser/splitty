//! RFC6455 carrier with counted buffers. Reuse Tungstenite's handshake and
//! frame-header definitions; read payloads directly into their delivery owner.
//! No library reassembly/read/write Vec can grow outside the shared budget.
use splitty_transport::{
    budget::Charged,
    codec::Frame,
    peer::Sender,
    queue::Context,
    wire::{self, Kind},
};
use std::{
    io::{self, Cursor},
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_tungstenite::tungstenite::protocol::frame::{
    coding::{CloseCode, Control, Data, OpCode},
    FrameHeader,
};

pub(crate) struct ControlFrame {
    pub opcode: Control,
    pub body: Charged<Vec<u8>>,
}
enum Destination {
    Preparing(Vec<Charged<Frame>>),
    Admitted(Sender),
}
struct Input {
    context: Context,
    destination: Mutex<Destination>,
}
impl Input {
    fn deliver(&self, frame: Charged<Frame>) -> io::Result<()> {
        let mut destination = self.destination.lock().expect("browser input");
        match &mut *destination {
            Destination::Preparing(early) if early.len() < 64 => early.push(frame),
            Destination::Preparing(_) => {
                return Err(protocol(1008, "preparation input queue exhausted"))
            }
            Destination::Admitted(sender) => sender.data_frame(frame)?,
        }
        Ok(())
    }
}

pub(crate) struct Reader {
    controls: mpsc::Receiver<ControlFrame>,
    task: JoinHandle<()>,
    close: Arc<AtomicU16>,
    input: Arc<Input>,
}
impl Reader {
    pub fn start<R: AsyncRead + Unpin + Send + 'static>(read: R, sender: Sender) -> Self {
        let context = sender.context();
        Self::with_input(read, context, Destination::Admitted(sender))
    }
    pub fn preparing<R: AsyncRead + Unpin + Send + 'static>(read: R, context: Context) -> Self {
        Self::with_input(read, context, Destination::Preparing(Vec::new()))
    }
    fn with_input<R: AsyncRead + Unpin + Send + 'static>(
        read: R,
        context: Context,
        destination: Destination,
    ) -> Self {
        let (controls, receiver) = mpsc::channel(64);
        let close = Arc::new(AtomicU16::new(1000));
        let failed = close.clone();
        let input = Arc::new(Input {
            context,
            destination: Mutex::new(destination),
        });
        let task_input = input.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = read_loop(read, &task_input, controls, wire::FRAME_TIMEOUT).await {
                let code = error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<Protocol>())
                    .map_or(1001, |error| error.code);
                failed.store(code, Ordering::Release);
                tracing::debug!(%error, "browser WebSocket reader ended");
                task_input.context.close();
            }
        });
        Self {
            controls: receiver,
            task,
            close,
            input,
        }
    }
    /// Switch once, after the fixed host's Ready exchange. All early input
    /// retains its allocation/lease and order; failed setup drops it instead.
    pub fn admit(&self, sender: Sender) -> io::Result<()> {
        let mut destination = self.input.destination.lock().expect("browser input");
        let Destination::Preparing(early) = &mut *destination else {
            return Err(protocol(1002, "browser input already admitted"));
        };
        for frame in early.drain(..) {
            sender.data_frame(frame)?;
        }
        *destination = Destination::Admitted(sender);
        Ok(())
    }
    pub async fn next(&mut self) -> Option<ControlFrame> {
        self.controls.recv().await
    }
    pub fn close_code(&self) -> u16 {
        self.close.load(Ordering::Acquire)
    }
    pub async fn finish(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) struct Writer<W> {
    io: W,
    pub tainted: bool,
}
impl<W: AsyncWrite + Unpin> Writer<W> {
    pub fn new(io: W) -> Self {
        Self { io, tainted: false }
    }
    pub async fn send(&mut self, opcode: OpCode, payload: &[u8]) -> io::Result<()> {
        if self.tainted {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "WebSocket carrier has an unfinished write",
            ));
        }
        if matches!(opcode, OpCode::Control(_)) && payload.len() > 125 {
            return Err(protocol(1002, "oversized control"));
        }
        let header = FrameHeader {
            is_final: true,
            opcode,
            ..Default::default()
        };
        let mut bytes = [0; 14];
        let mut cursor = Cursor::new(bytes.as_mut_slice());
        header
            .format(payload.len() as u64, &mut cursor)
            .map_err(io::Error::other)?;
        let count = cursor.position() as usize;
        // Cancellation of any partial write ends the carrier. Never append a
        // close/control frame to an unfinished payload on that byte stream.
        self.tainted = true;
        self.io.write_all(&bytes[..count]).await?;
        self.io.write_all(payload).await?;
        self.io.flush().await?;
        self.tainted = false;
        Ok(())
    }
    pub async fn shutdown(&mut self) {
        let _ = self.io.shutdown().await;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct Protocol {
    code: u16,
    message: &'static str,
}
fn protocol(code: u16, message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Protocol { code, message })
}

struct Partial {
    frame: Charged<Frame>,
    bytes: usize,
    deadline: Instant,
}
enum Read {
    Data(Charged<Frame>),
    Control(ControlFrame),
    More,
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut read: R,
    input: &Input,
    controls: mpsc::Sender<ControlFrame>,
    frame_timeout: std::time::Duration,
) -> io::Result<()> {
    let context = &input.context;
    let mut closing = context.closing();
    let mut partial: Option<Partial> = None;
    loop {
        let deadline = partial.as_ref().map_or_else(
            || Instant::now() + frame_timeout,
            |partial| partial.deadline,
        );
        let next = tokio::select! { biased;
            _ = async { let _ = closing.wait_for(|closed| *closed).await; } => return Ok(()),
            result = time::timeout_at(deadline, read_frame(&mut read, context, &mut partial, deadline)) =>
                result.map_err(|_| protocol(1002, "partial WebSocket frame/message timed out"))??,
        };
        match next {
            Read::More => {}
            Read::Data(frame) => input.deliver(frame)?,
            Read::Control(frame) => {
                let close = frame.opcode == Control::Close;
                controls
                    .try_send(frame)
                    .map_err(|_| protocol(1008, "WebSocket control queue exhausted"))?;
                if close {
                    return Ok(());
                }
            }
        }
    }
}

async fn read_frame<R: AsyncRead + Unpin>(
    read: &mut R,
    context: &Context,
    partial: &mut Option<Partial>,
    deadline: Instant,
) -> io::Result<Read> {
    let mut bytes = [0; 14];
    read.read_exact(&mut bytes[..2]).await?;
    if bytes[1] & 0x80 == 0 {
        return Err(protocol(1002, "unmasked browser frame"));
    }
    let extra = match bytes[1] & 127 {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    let count = 2 + extra + 4;
    read.read_exact(&mut bytes[2..count]).await?;
    let (header, length) = FrameHeader::parse(&mut Cursor::new(&bytes[..count]))
        .map_err(|_| protocol(1002, "invalid WebSocket opcode/header"))?
        .ok_or_else(|| protocol(1002, "incomplete WebSocket header"))?;
    if header.rsv1
        || header.rsv2
        || header.rsv3
        || (extra == 2 && length < 126)
        || (extra == 8 && (length <= u64::from(u16::MAX) || bytes[2] & 128 != 0))
    {
        return Err(protocol(
            1002,
            "invalid WebSocket header or unsupported extension",
        ));
    }
    let mask = header
        .mask
        .ok_or_else(|| protocol(1002, "unmasked browser frame"))?;
    let budget = &context.budget;
    if let OpCode::Control(opcode) = header.opcode {
        if !matches!(opcode, Control::Close | Control::Ping | Control::Pong)
            || !header.is_final
            || length > 125
        {
            return Err(protocol(1002, "invalid WebSocket control"));
        }
        let size = length as usize;
        let mut body = budget
            .charge(size + 14, true, || vec![0; size])
            .map_err(io::Error::other)?;
        read.read_exact(&mut body.value).await?;
        unmask(&mut body.value, mask);
        if opcode == Control::Close {
            if size == 1 {
                return Err(protocol(1002, "invalid close payload"));
            }
            if size >= 2 {
                let code = u16::from_be_bytes(body[..2].try_into().expect("two-byte close code"));
                if !CloseCode::from(code).is_allowed() {
                    return Err(protocol(1002, "invalid close code"));
                }
                std::str::from_utf8(&body[2..])
                    .map_err(|_| protocol(1007, "invalid close reason"))?;
            }
        }
        return Ok(Read::Control(ControlFrame { opcode, body }));
    }
    let maximum = wire::MAX_FRAME_BYTES - wire::HEADER_BYTES - wire::DELIVERY_BYTES;
    let length = usize::try_from(length)
        .ok()
        .filter(|length| *length <= maximum)
        .ok_or_else(|| protocol(1009, "WebSocket message exceeds limit"))?;
    match header.opcode {
        OpCode::Data(Data::Text) if partial.is_none() => {
            let capacity = if header.is_final {
                length + wire::DELIVERY_BYTES
            } else {
                maximum + wire::DELIVERY_BYTES
            };
            let mut frame = budget
                .charge(capacity + wire::HEADER_BYTES, false, || Frame {
                    kind: Kind::Text,
                    payload: Vec::with_capacity(capacity),
                })
                .map_err(io::Error::other)?;
            frame.value.payload.resize(length + wire::DELIVERY_BYTES, 0);
            read.read_exact(&mut frame.value.payload[4..4 + length])
                .await?;
            unmask(&mut frame.value.payload[4..4 + length], mask);
            if header.is_final {
                validate_text(&frame, length)?;
                Ok(Read::Data(frame))
            } else {
                *partial = Some(Partial {
                    frame,
                    bytes: length,
                    deadline,
                });
                Ok(Read::More)
            }
        }
        OpCode::Data(Data::Continue) if partial.is_some() => {
            let message = partial.as_mut().expect("partial message");
            let start = message.bytes;
            let total = start
                .checked_add(length)
                .filter(|size| *size <= maximum)
                .ok_or_else(|| protocol(1009, "WebSocket message exceeds limit"))?;
            message
                .frame
                .value
                .payload
                .resize(total + wire::DELIVERY_BYTES, 0);
            read.read_exact(&mut message.frame.value.payload[4 + start..4 + total])
                .await?;
            unmask(&mut message.frame.value.payload[4 + start..4 + total], mask);
            message.bytes = total;
            if header.is_final {
                let message = partial.take().expect("completed message");
                validate_text(&message.frame, total)?;
                Ok(Read::Data(message.frame))
            } else {
                Ok(Read::More)
            }
        }
        OpCode::Data(Data::Binary) => Err(protocol(1003, "binary browser input is unsupported")),
        _ => Err(protocol(1002, "invalid WebSocket continuation or opcode")),
    }
}
fn unmask(bytes: &mut [u8], mask: [u8; 4]) {
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte ^= mask[index % 4];
    }
}
fn validate_text(frame: &Frame, length: usize) -> io::Result<()> {
    std::str::from_utf8(&frame.payload[4..4 + length])
        .map(|_| ())
        .map_err(|_| protocol(1007, "invalid WebSocket text"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::{
        budget::{ConnectionBudget, ProcessBudget},
        peer::Peer,
        queue,
    };
    fn peer(process: Arc<ProcessBudget>) -> (Peer, tokio::io::DuplexStream) {
        let (io, hold) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(io);
        let ready = wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        (
            Peer::start(
                read,
                write,
                &ready,
                queue::Context::new(ConnectionBudget::new(
                    process,
                    wire::CONNECTION_BYTES,
                    wire::CONNECTION_CONTROL_BYTES,
                )),
            ),
            hold,
        )
    }
    fn masked(opcode: OpCode, final_frame: bool, payload: &[u8]) -> Vec<u8> {
        let mask = [1, 2, 3, 4];
        let header = FrameHeader {
            opcode,
            is_final: final_frame,
            mask: Some(mask),
            ..Default::default()
        };
        let mut bytes = Vec::with_capacity(14 + payload.len());
        header.format(payload.len() as u64, &mut bytes).unwrap();
        let start = bytes.len();
        bytes.extend_from_slice(payload);
        unmask(&mut bytes[start..], mask);
        bytes
    }
    fn process() -> Arc<ProcessBudget> {
        ProcessBudget::new(wire::PROCESS_BYTES, wire::PROCESS_CONTROL_BYTES)
    }

    #[tokio::test]
    async fn preparation_preserves_input_order_and_leases_until_fixed_host_admission() {
        let process = process();
        let (peer, mut output) = peer(process.clone());
        let (mut browser, input) = tokio::io::duplex(4096);
        let context = peer.sender.context();
        let reader = Reader::preparing(input, context.clone());
        for text in [b"first".as_slice(), b"second"] {
            browser
                .write_all(&masked(OpCode::Data(Data::Text), true, text))
                .await
                .unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Destination::Preparing(early) = &*reader.input.destination.lock().unwrap() {
                    if early.len() == 2 {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let held = process.used();
        assert!(held > 0);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), output.read_u8())
                .await
                .is_err(),
            "application input cannot precede Ready admission"
        );
        reader.admit(peer.sender.clone()).unwrap();
        for expected in [b"first".as_slice(), b"second"] {
            let frame = splitty_transport::codec::read(&mut output, &ConnectionBudget::default())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(frame.application().unwrap().1, expected);
        }
        assert!(
            process.used() >= held,
            "a carrier write has not granted native delivery credit"
        );
        context.close();
        reader.finish().await;
        peer.shutdown().await;
        assert_eq!(process.used(), 0);
    }

    #[tokio::test]
    async fn early_input_flood_closes_setup_and_releases_all_retained_payloads() {
        let process = process();
        let context = Context::new(ConnectionBudget::new(
            process.clone(),
            wire::CONNECTION_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        ));
        let (mut browser, input) = tokio::io::duplex(4096);
        let reader = Reader::preparing(input, context.clone());
        for _ in 0..65 {
            browser
                .write_all(&masked(OpCode::Data(Data::Text), true, b"unsent mutation"))
                .await
                .unwrap();
        }
        let mut closing = context.closing();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            closing.wait_for(|closed| *closed),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reader.close_code(), 1008);
        reader.finish().await;
        assert_eq!(process.used(), 0);
    }

    #[tokio::test]
    async fn large_fragments_reuse_one_counted_allocation_and_controls_preserve_its_deadline() {
        let process = process();
        let (peer, _hold) = peer(process.clone());
        let mut partial = None;
        let deadline = Instant::now() + wire::FRAME_TIMEOUT;
        let bytes = masked(
            OpCode::Data(Data::Text),
            false,
            &vec![b'a'; 10 * 1024 * 1024],
        );
        assert!(matches!(
            read_frame(
                &mut bytes.as_slice(),
                &peer.sender.context(),
                &mut partial,
                deadline
            )
            .await
            .unwrap(),
            Read::More
        ));
        let pointer = partial.as_ref().unwrap().frame.payload.as_ptr();
        assert_eq!(
            partial.as_ref().unwrap().frame.payload.capacity(),
            wire::MAX_FRAME_BYTES - wire::HEADER_BYTES
        );
        assert_eq!(process.used(), wire::MAX_FRAME_BYTES);
        let ping = masked(OpCode::Control(Control::Ping), true, b"nonce");
        let Read::Control(ping) = read_frame(
            &mut ping.as_slice(),
            &peer.sender.context(),
            &mut partial,
            deadline + wire::FRAME_TIMEOUT,
        )
        .await
        .unwrap() else {
            panic!("ping");
        };
        assert_eq!(ping.body.as_slice(), b"nonce");
        drop(ping);
        assert_eq!(partial.as_ref().unwrap().deadline, deadline);
        let bytes = masked(OpCode::Data(Data::Continue), true, &vec![b'b'; 1024 * 1024]);
        let Read::Data(frame) = read_frame(
            &mut bytes.as_slice(),
            &peer.sender.context(),
            &mut partial,
            deadline,
        )
        .await
        .unwrap() else {
            panic!("completed data");
        };
        assert!(partial.is_none());
        assert_eq!(frame.payload.as_ptr(), pointer);
        assert_eq!(
            frame.payload.capacity(),
            wire::MAX_FRAME_BYTES - wire::HEADER_BYTES
        );
        assert_eq!(frame.payload.len(), 11 * 1024 * 1024 + wire::DELIVERY_BYTES);
        let mut writer = Writer::new(tokio::io::sink());
        writer
            .send(
                OpCode::Data(Data::Text),
                &frame.payload[4..frame.payload.len() - 16],
            )
            .await
            .unwrap();
        assert_eq!(
            process.used(),
            wire::MAX_FRAME_BYTES,
            "writer retains no additional payload buffer"
        );
        drop(frame);
        assert_eq!(process.used(), 0);
        peer.shutdown().await;
    }

    #[tokio::test]
    async fn aggregate_exhaustion_and_invalid_frames_fail_before_payload_allocation() {
        let process = ProcessBudget::new(wire::MAX_FRAME_BYTES + 8192, 8192);
        let (first, _a) = peer(process.clone());
        let (second, _b) = peer(process.clone());
        let mut partial = None;
        let input = masked(OpCode::Data(Data::Text), false, b"a");
        read_frame(
            &mut input.as_slice(),
            &first.sender.context(),
            &mut partial,
            Instant::now() + wire::FRAME_TIMEOUT,
        )
        .await
        .unwrap();
        let held = process.used();
        let mut other = None;
        assert!(read_frame(
            &mut input.as_slice(),
            &second.sender.context(),
            &mut other,
            Instant::now() + wire::FRAME_TIMEOUT
        )
        .await
        .is_err());
        assert_eq!(process.used(), held);
        assert!(other.is_none());
        drop(partial);
        assert_eq!(process.used(), 0);
        for bytes in [
            vec![0x81, 0],
            vec![0x83, 0x80, 1, 2, 3, 4],
            masked(OpCode::Data(Data::Continue), true, b"x"),
            masked(OpCode::Control(Control::Ping), false, b"x"),
            masked(OpCode::Control(Control::Close), true, &[1]),
            masked(
                OpCode::Control(Control::Close),
                true,
                &1005u16.to_be_bytes(),
            ),
            masked(OpCode::Data(Data::Text), true, &[255]),
            vec![0x81, 0xff, 0x80, 0, 0, 0, 0, 0, 0, 1, 1, 2, 3, 4],
        ] {
            let mut partial = None;
            assert!(read_frame(
                &mut bytes.as_slice(),
                &first.sender.context(),
                &mut partial,
                Instant::now() + wire::FRAME_TIMEOUT
            )
            .await
            .is_err());
            assert_eq!(process.used(), 0);
        }
        first.shutdown().await;
        second.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn interleaved_control_does_not_keep_a_partial_message_alive_forever() {
        let process = process();
        let (peer, _hold) = peer(process.clone());
        let sender = peer.sender.clone();
        let (mut client, read) = tokio::io::duplex(4096);
        let (controls, mut received) = mpsc::channel(64);
        client
            .write_all(&masked(OpCode::Data(Data::Text), false, b"a"))
            .await
            .unwrap();
        client
            .write_all(&masked(OpCode::Control(Control::Ping), true, b"nonce"))
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            read_loop(
                read,
                &Input {
                    context: sender.context(),
                    destination: Mutex::new(Destination::Admitted(sender)),
                },
                controls,
                std::time::Duration::from_millis(20),
            )
            .await
        });
        assert_eq!(received.recv().await.unwrap().opcode, Control::Ping);
        assert!(task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert_eq!(process.used(), 0);
        peer.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_write_cannot_be_followed_by_another_frame() {
        let (io, _hold) = tokio::io::duplex(1);
        let mut writer = Writer::new(io);
        assert!(time::timeout(
            std::time::Duration::from_millis(1),
            writer.send(OpCode::Data(Data::Text), b"payload")
        )
        .await
        .is_err());
        assert!(writer.tainted);
        assert_eq!(
            writer
                .send(OpCode::Control(Control::Close), &1000u16.to_be_bytes())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
