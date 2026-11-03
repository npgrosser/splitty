//! Bounded bytes between a document adapter and the native connection task.
//! This layer has no WebView API, listener, address selection or browser auth.
//! The adapter must still authenticate the real caller/document before using
//! its Input handle, and recheck the document before publishing each Chunk.
use super::Attempt;
use splitty_transport::{budget::Lease, queue};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore},
};

pub(crate) const CHUNK_BYTES: usize = 64 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
type CloseWait = Pin<Box<dyn Future<Output = ()> + Send>>;

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "native document closed")
}
fn closing(context: &queue::Context) -> CloseWait {
    let mut closing = context.closing();
    Box::pin(async move {
        let _ = closing.wait_for(|closed| *closed).await;
    })
}

struct State {
    context: queue::Context,
    started: watch::Sender<bool>,
    outgoing: Mutex<Option<Flight>>,
    // Admission follows every actual buffer/handler reference, not just the
    // document's cancellation request or the IO task's return.
    _attempt: Arc<Attempt>,
    _storage: Lease,
}
impl State {
    fn fail(&self, message: &'static str) -> io::Error {
        self.context.close();
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
    fn current(&self) -> io::Result<()> {
        if self.context.is_closed() || !self._attempt.current() {
            Err(closed())
        } else {
            Ok(())
        }
    }
}
struct Flight {
    token: [u8; 16],
    completion: Arc<Completion>,
}
struct Completion(Mutex<CompletionState>);
struct CompletionState {
    copied: bool,
    consumed: bool,
    done: Option<oneshot::Sender<()>>,
}
impl Completion {
    fn mark(&self, copied: bool) {
        let mut state = self.0.lock().expect("IPC completion");
        if copied {
            state.copied = true;
        } else {
            state.consumed = true;
        }
        if state.copied && state.consumed {
            if let Some(done) = state.done.take() {
                let _ = done.send(());
            }
        }
    }
}
struct Copied(Arc<Completion>);
impl Drop for Copied {
    fn drop(&mut self) {
        self.0.mark(true);
    }
}

/// Raw bytes remain private to the native adapter. Flush and the next write
/// wait until this buffer is dropped and the document acknowledges its token.
/// Field order drops the bytes before waking the writer for another buffer.
pub(crate) struct Chunk {
    bytes: Vec<u8>,
    _copied: Copied,
    token: [u8; 16],
    _state: Arc<State>,
}
impl Chunk {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn token(&self) -> [u8; 16] {
        self.token
    }
}
struct Packet {
    bytes: Vec<u8>,
    offset: usize,
    permit: Option<OwnedSemaphorePermit>,
    accepted: Option<oneshot::Sender<()>>,
    _state: Arc<State>,
}
impl Packet {
    fn finish(mut self) {
        drop(std::mem::take(&mut self.bytes));
        self.permit.take();
        if let Some(accepted) = self.accepted.take() {
            let _ = accepted.send(());
        }
    }
}

/// Commands receive a clone of this native-assigned handle, never an address.
/// It accepts one chunk in flight, including while AsyncRead retains a prefix.
#[derive(Clone)]
pub(crate) struct Input {
    state: Arc<State>,
    packets: mpsc::Sender<Packet>,
    permit: Arc<Semaphore>,
}
impl Input {
    pub(crate) fn start(&self) -> io::Result<()> {
        self.state.current()?;
        if !self.state._attempt.live() {
            return Err(self.state.fail("native service not ready"));
        }
        if self.state.started.send_replace(true) {
            return Err(self.state.fail("native document already started"));
        }
        Ok(())
    }
    pub(crate) fn write(&self, bytes: &[u8]) -> io::Result<oneshot::Receiver<()>> {
        self.state.current()?;
        if !*self.state.started.borrow() {
            return Err(self.state.fail("native document not started"));
        }
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err(self.state.fail("invalid IPC chunk size"));
        }
        let permit = self
            .permit
            .clone()
            .try_acquire_owned()
            .map_err(|_| self.state.fail("IPC input already in flight"))?;
        let (accepted, result) = oneshot::channel();
        let packet = Packet {
            bytes: bytes.to_vec(),
            offset: 0,
            permit: Some(permit),
            accepted: Some(accepted),
            _state: self.state.clone(),
        };
        self.packets
            .try_send(packet)
            .map_err(|_| self.state.fail("native input unavailable"))?;
        Ok(result)
    }
    pub(crate) fn consumed(&self, token: [u8; 16]) -> io::Result<()> {
        self.state.current()?;
        let mut pending = self.state.outgoing.lock().expect("IPC outgoing token");
        let Some(flight) = pending.as_ref() else {
            return Err(self.state.fail("no IPC output in flight"));
        };
        if flight.token != token {
            return Err(self.state.fail("invalid IPC output token"));
        }
        let flight = pending.take().expect("matching flight");
        drop(pending);
        flight.completion.mark(false);
        Ok(())
    }
    pub(crate) fn close(&self) {
        self.state.context.close();
    }
}

pub(crate) struct Channel {
    input: Input,
    chunks: mpsc::Receiver<Chunk>,
}
impl Channel {
    pub(crate) fn input(&self) -> Input {
        self.input.clone()
    }
    pub(crate) async fn next(&mut self) -> io::Result<Option<Chunk>> {
        let state = &self.input.state;
        state.current()?;
        if !*state.started.borrow() {
            let mut started = state.started.subscribe();
            tokio::select! { biased;
                _ = closing(&state.context) => return Ok(None),
                result = tokio::time::timeout(START_TIMEOUT, async { let _ = started.wait_for(|started| *started).await; }) => {
                    if result.is_err() {
                        state.context.close();
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "native document did not start"));
                    }
                }
            }
        }
        tokio::select! { biased;
            _ = closing(&state.context) => Ok(None),
            chunk = self.chunks.recv() => Ok(chunk),
        }
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        self.input.close();
    }
}

struct Writing {
    accepted: oneshot::Receiver<()>,
    deadline: Pin<Box<tokio::time::Sleep>>,
}
pub(crate) struct Io {
    state: Arc<State>,
    packets: mpsc::Receiver<Packet>,
    packet: Option<Packet>,
    chunks: mpsc::Sender<Chunk>,
    writing: Option<Writing>,
    read_closed: CloseWait,
    write_closed: CloseWait,
}

pub(crate) fn pair(attempt: Arc<Attempt>) -> io::Result<(Channel, Io)> {
    if !attempt.current() {
        return Err(closed());
    }
    let context = attempt.context.clone();
    // One maximum-sized chunk per direction. Reserve before any bytes arrive;
    // receipt/cancel input therefore remains possible when data fills a queue.
    let storage = context
        .budget
        .reserve(2 * CHUNK_BYTES, false)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    let state = Arc::new(State {
        context,
        started: watch::channel(false).0,
        outgoing: Mutex::new(None),
        _attempt: attempt,
        _storage: storage,
    });
    let (packets, incoming) = mpsc::channel(1);
    let (chunks, outgoing) = mpsc::channel(1);
    let input = Input {
        state: state.clone(),
        packets,
        permit: Arc::new(Semaphore::new(1)),
    };
    let io = Io {
        read_closed: closing(&state.context),
        write_closed: closing(&state.context),
        state,
        packets: incoming,
        packet: None,
        chunks,
        writing: None,
    };
    Ok((
        Channel {
            input,
            chunks: outgoing,
        },
        io,
    ))
}
impl Io {
    fn poll_written(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.state.context.is_closed() || self.write_closed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        let Some(writing) = self.writing.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        match Pin::new(&mut writing.accepted).poll(cx) {
            Poll::Ready(Ok(())) => {
                self.writing.take();
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => {
                self.state.context.close();
                Poll::Ready(Err(closed()))
            }
            Poll::Pending => {
                if writing.deadline.as_mut().poll(cx).is_ready() {
                    self.state.context.close();
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "native IPC consumer stalled",
                    )));
                }
                Poll::Pending
            }
        }
    }
}
impl AsyncRead for Io {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.state.context.is_closed() || self.read_closed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if let Some(packet) = self.packet.as_mut() {
                let length = buffer.remaining().min(packet.bytes.len() - packet.offset);
                buffer.put_slice(&packet.bytes[packet.offset..packet.offset + length]);
                packet.offset += length;
                if packet.offset == packet.bytes.len() {
                    self.packet.take().expect("consumed input").finish();
                }
                return Poll::Ready(Ok(()));
            }
            match self.packets.poll_recv(cx) {
                Poll::Ready(Some(packet)) => self.packet = Some(packet),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
impl AsyncWrite for Io {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.writing.is_some() {
            match self.poll_written(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
        }
        if self.state.context.is_closed() || self.write_closed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(closed()));
        }
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let length = bytes.len().min(CHUNK_BYTES);
        let token = rand::random();
        let (done, accepted) = oneshot::channel();
        let completion = Arc::new(Completion(Mutex::new(CompletionState {
            copied: false,
            consumed: false,
            done: Some(done),
        })));
        *self.state.outgoing.lock().expect("IPC outgoing token") = Some(Flight {
            token,
            completion: completion.clone(),
        });
        let chunk = Chunk {
            bytes: bytes[..length].to_vec(),
            _copied: Copied(completion),
            token,
            _state: self.state.clone(),
        };
        if self.chunks.try_send(chunk).is_err() {
            self.state.context.close();
            return Poll::Ready(Err(closed()));
        }
        self.writing = Some(Writing {
            accepted,
            deadline: Box::pin(tokio::time::sleep(DELIVERY_TIMEOUT)),
        });
        // AsyncWrite reports bytes as soon as it has copied/accepted them.
        // Returning Pending here would violate cancel-safe write(): a later
        // poll might use a different buffer. The next write and flush wait
        // for this chunk's actual release and consumption.
        Poll::Ready(Ok(length))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_written(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(result) => {
                self.state.context.close();
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
impl Drop for Io {
    fn drop(&mut self) {
        self.state.context.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_ipc::{BackendId, Broker, Policy};
    use splitty_transport::wire;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixture(ready: bool) -> (Broker, Arc<Attempt>, Channel, Io) {
        let broker = Broker::default();
        broker
            .register(
                "main",
                Policy::Main {
                    backend: BackendId::new(),
                },
            )
            .unwrap();
        let proof = broker.committed("main").unwrap();
        let attempt = Arc::new(broker.open("main", &proof, 1, None, false).unwrap());
        if ready {
            attempt
                .ready(
                    &wire::Ready {
                        generation: 1,
                        instance: "a".repeat(32),
                        mux_protocol: 1,
                        purpose: wire::Purpose::Direct,
                        limits: wire::Limits::default(),
                    },
                    1,
                )
                .unwrap();
        }
        let (channel, io) = pair(attempt.clone()).unwrap();
        (broker, attempt, channel, io)
    }

    #[tokio::test]
    async fn input_is_byte_exact_and_acceptance_waits_for_the_whole_chunk() {
        let (broker, attempt, channel, mut io) = fixture(true);
        let budget = attempt.context.budget.clone();
        let input = channel.input();
        input.start().unwrap();
        let bytes: Vec<u8> = (0..=255).cycle().take(CHUNK_BYTES).collect();
        let mut accepted = input.write(&bytes).unwrap();
        let mut first = [0; 7];
        io.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, &bytes[..7]);
        assert_eq!(
            accepted.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        let mut rest = vec![0; CHUNK_BYTES - 7];
        io.read_exact(&mut rest).await.unwrap();
        accepted.await.unwrap();
        assert_eq!(rest, bytes[7..]);
        // Ingress is pre-admitted; filling the remaining data budget cannot
        // prevent a receipt/cancel chunk from reaching its parser.
        let held = budget
            .reserve(
                wire::CONNECTION_BYTES - wire::CONNECTION_CONTROL_BYTES - 2 * CHUNK_BYTES,
                false,
            )
            .unwrap();
        let accepted = input.write(b"cancel").unwrap();
        let mut cancel = [0; 6];
        io.read_exact(&mut cancel).await.unwrap();
        accepted.await.unwrap();
        assert_eq!(&cancel, b"cancel");
        drop(held);
        assert_eq!(budget.used(), 2 * CHUNK_BYTES);
        drop((io, channel, input, attempt));
        assert_eq!(budget.used(), 0);
        assert_eq!(broker.counts(), (0, 0));
    }

    #[tokio::test]
    async fn partial_input_still_occupies_the_only_ingress_slot() {
        let (_broker, attempt, channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        let accepted = input.write(b"two").unwrap();
        let mut byte = [0];
        io.read_exact(&mut byte).await.unwrap();
        assert_eq!(
            input.write(b"extra").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(attempt.context.is_closed());
        drop(io);
        assert!(accepted.await.is_err());
    }

    #[tokio::test]
    async fn output_chunks_wait_for_both_consumption_and_actual_buffer_release() {
        let (broker, attempt, mut channel, mut io) = fixture(true);
        let budget = attempt.context.budget.clone();
        let input = channel.input();
        input.start().unwrap();
        let bytes: Vec<u8> = (0..=255).cycle().take(1024 * 1024 + 17).collect();
        let expected = bytes.clone();
        let writer = tokio::spawn(async move {
            io.write_all(&bytes).await.unwrap();
            io.flush().await.unwrap();
            io
        });
        let mut received = Vec::new();
        while received.len() < expected.len() {
            let chunk = channel.next().await.unwrap().unwrap();
            assert!(chunk.bytes().len() <= CHUNK_BYTES);
            received.extend_from_slice(chunk.bytes());
            input.consumed(chunk.token()).unwrap();
            tokio::task::yield_now().await;
            assert!(
                !writer.is_finished(),
                "early ack cannot release a still-owned output buffer"
            );
            drop(chunk);
        }
        let io = writer.await.unwrap();
        assert_eq!(received, expected);
        assert_eq!(budget.used(), 2 * CHUNK_BYTES);
        drop((io, input, channel, attempt));
        assert_eq!(budget.used(), 0);
        assert_eq!(broker.counts(), (0, 0));
    }

    #[tokio::test]
    async fn a_dropped_output_without_its_token_does_not_complete_the_write() {
        let (_broker, attempt, mut channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        let writer = tokio::spawn(async move {
            io.write_all(b"data").await?;
            io.flush().await
        });
        let chunk = channel.next().await.unwrap().unwrap();
        let token = chunk.token();
        drop(chunk);
        tokio::task::yield_now().await;
        assert!(!writer.is_finished());
        let mut wrong = token;
        wrong[0] ^= 1;
        assert_eq!(
            input.consumed(wrong).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(attempt.context.is_closed());
        assert_eq!(
            writer.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn navigation_revokes_io_but_retains_quota_until_outstanding_buffers_are_gone() {
        let (broker, attempt, mut channel, mut io) = fixture(true);
        let budget = attempt.context.budget.clone();
        let input = channel.input();
        input.start().unwrap();
        let writer = tokio::spawn(async move {
            io.write_all(b"old document").await?;
            io.flush().await
        });
        let chunk = channel.next().await.unwrap().unwrap();
        broker.navigating("main");
        assert_eq!(
            writer.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(input.consumed(chunk.token()).is_err());
        assert!(input.write(b"stale").is_err());
        drop((input, channel, attempt));
        assert_eq!(broker.counts(), (1, 0));
        assert_eq!(budget.used(), 2 * CHUNK_BYTES);
        drop(chunk);
        assert_eq!(broker.counts(), (0, 0));
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn frozen_output_times_out_without_stalling_another_connection() {
        let (_broker, attempt, mut channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        let writer = tokio::spawn(async move {
            io.write_all(b"frozen").await?;
            io.flush().await
        });
        let chunk = channel.next().await.unwrap().unwrap();
        tokio::time::advance(DELIVERY_TIMEOUT).await;
        assert_eq!(
            writer.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(attempt.context.is_closed());
        let (_other_broker, other, healthy, mut healthy_io) = fixture(true);
        let healthy_input = healthy.input();
        healthy_input.start().unwrap();
        let accepted = healthy_input.write(b"healthy").unwrap();
        let mut bytes = [0; 7];
        healthy_io.read_exact(&mut bytes).await.unwrap();
        accepted.await.unwrap();
        assert_eq!(&bytes, b"healthy");
        assert!(other.current());
        drop(chunk);
    }

    #[tokio::test(start_paused = true)]
    async fn readiness_start_deadline_and_duplicate_ack_are_enforced() {
        let (_broker, _attempt, channel, _io) = fixture(false);
        assert!(channel.input().start().is_err());
        let (_broker, attempt, mut channel, _io) = fixture(true);
        assert_eq!(
            channel.next().await.err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(attempt.context.is_closed());
        let (_broker, attempt, mut channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        let writer = tokio::spawn(async move {
            io.write_all(b"reply").await?;
            io.flush().await?;
            Ok::<_, io::Error>(io)
        });
        let chunk = channel.next().await.unwrap().unwrap();
        let token = chunk.token();
        drop(chunk);
        input.consumed(token).unwrap();
        let _io = writer.await.unwrap().unwrap();
        assert_eq!(
            input.consumed(token).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(attempt.context.is_closed());
    }
    #[tokio::test]
    async fn cancelled_pending_write_does_not_consume_a_later_buffer() {
        let (_broker, _attempt, mut channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        let first =
            std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut io).poll_write(cx, b"first")))
                .await;
        assert!(
            matches!(first, Poll::Ready(Ok(5))),
            "copied bytes must be reported immediately"
        );
        let abandoned =
            std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut io).poll_write(cx, b"abandoned")))
                .await;
        assert!(abandoned.is_pending());
        let chunk = channel.next().await.unwrap().unwrap();
        assert_eq!(chunk.bytes(), b"first");
        let token = chunk.token();
        drop(chunk);
        input.consumed(token).unwrap();
        assert_eq!(
            io.write(b"x").await.unwrap(),
            1,
            "the new call writes its own, shorter buffer"
        );
        let chunk = channel.next().await.unwrap().unwrap();
        assert_eq!(chunk.bytes(), b"x");
        let token = chunk.token();
        drop(chunk);
        input.consumed(token).unwrap();
        io.flush().await.unwrap();
    }

    #[tokio::test]
    async fn closed_io_remains_terminal_across_repeated_polls() {
        let (_broker, _attempt, channel, mut io) = fixture(true);
        let input = channel.input();
        input.start().unwrap();
        input.close();
        for _ in 0..3 {
            assert_eq!(
                io.read(&mut [0]).await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert_eq!(
                io.write(b"data").await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert_eq!(
                io.flush().await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert_eq!(
                io.shutdown().await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
        }
    }
}
