//! An admitted full-duplex carrier has one persistent reader and one ordered
//! writer. Control delivery has independent bounded queue capacity. A select
//! never restarts a partially consumed frame on the same byte stream.
use crate::{
    budget::Charged,
    codec::{self, Frame},
    flow, queue,
    wire::{self, Delivery, Kind, Ready},
};
use serde::Serialize;
use std::{
    future::Future,
    io,
    sync::{Arc, Mutex},
    task::Poll,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    task::JoinHandle,
    time::{self, Instant},
};

const CONTROLS: usize = 64;
enum Outbound {
    Frame(Frame),
    Confirmed(Frame, tokio::sync::oneshot::Sender<()>),
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Clone)]
pub struct Sender {
    context: queue::Context,
    data: queue::Sender<Outbound>,
    control: queue::Sender<Outbound>,
    receipts: queue::Sender<Delivery>,
    generation: u32,
    limits: wire::Limits,
}
impl Sender {
    pub fn context(&self) -> queue::Context {
        self.context.clone()
    }
    pub fn generation(&self) -> u32 {
        self.generation
    }
    pub fn close(&self) {
        self.context.close();
    }
    pub fn control<T: Serialize>(&self, kind: Kind, value: &T) -> io::Result<()> {
        if kind.is_data() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected control kind",
            ));
        }
        let size = codec::json_size(value, kind.payload_limit())? + wire::HEADER_BYTES;
        self.control
            .try_build(size, true, || Frame::json(kind, value).map(Outbound::Frame))
    }
    pub fn data(&self, kind: Kind, bytes: &[u8]) -> io::Result<()> {
        if !kind.is_data() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected data kind",
            ));
        }
        let size = bytes
            .len()
            .checked_add(wire::HEADER_BYTES + wire::DELIVERY_BYTES)
            .filter(|size| {
                *size <= self.limits.frame_bytes as usize
                    && (kind != Kind::Request || bytes.len() <= self.limits.api_bytes as usize)
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds ceiling"))?;
        self.data.try_build(size, false, || {
            Frame::data(kind, Delivery::new(self.generation), bytes).map(Outbound::Frame)
        })
    }
    /// One bounded producer may await actual remote consumption. This is used
    /// by owned-pipe substreams to keep one chunk per stream in flight, without
    /// blocking other streams or mistaking a local flush for delivery credit.
    pub fn data_confirmed(
        &self,
        kind: Kind,
        bytes: &[u8],
    ) -> io::Result<tokio::sync::oneshot::Receiver<()>> {
        if !kind.is_data() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected data kind",
            ));
        }
        let size = bytes
            .len()
            .checked_add(wire::HEADER_BYTES + wire::DELIVERY_BYTES)
            .filter(|size| {
                *size <= self.limits.frame_bytes as usize
                    && (kind != Kind::Request || bytes.len() <= self.limits.api_bytes as usize)
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds ceiling"))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.data.try_build(size, false, || {
            Ok(Outbound::Confirmed(
                Frame::data(kind, Delivery::new(self.generation), bytes)?,
                tx,
            ))
        })?;
        Ok(rx)
    }
    /// Transfer a counted native-layout body from an adapter without copying
    /// its payload or releasing its lease before delivery. The queue rejects
    /// charges belonging to a different connection.
    pub fn data_frame(&self, mut frame: Charged<Frame>) -> io::Result<()> {
        if !frame.kind.is_data()
            || frame.payload.len() < wire::DELIVERY_BYTES
            || frame.size() > self.limits.frame_bytes as usize
            || (frame.kind == Kind::Request
                && frame.payload.len() - wire::DELIVERY_BYTES > self.limits.api_bytes as usize)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid owned application frame",
            ));
        }
        let delivery = Delivery::new(self.generation);
        frame.value.payload[..4].copy_from_slice(&delivery.generation.to_be_bytes());
        let end = frame.payload.len();
        frame.value.payload[end - 16..].copy_from_slice(&delivery.receipt);
        frame.validate()?;
        self.data.forward(frame.map(Outbound::Frame))
    }
    pub fn json<T: Serialize>(&self, kind: Kind, value: &T) -> io::Result<()> {
        if !kind.is_data() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected data kind",
            ));
        }
        let limit = (kind.payload_limit() - wire::DELIVERY_BYTES)
            .min(self.limits.frame_bytes as usize - wire::HEADER_BYTES - wire::DELIVERY_BYTES);
        let limit = if kind == Kind::Request {
            limit.min(self.limits.api_bytes as usize)
        } else {
            limit
        };
        let size = codec::json_size(value, limit)? + wire::HEADER_BYTES + wire::DELIVERY_BYTES;
        self.data.try_build(size, false, || {
            Frame::data_json(kind, Delivery::new(self.generation), value).map(Outbound::Frame)
        })
    }

    /// A local writer barrier, ordered after prior application frames. It does
    /// not acknowledge remote execution and is never encoded on the wire.
    pub fn flush(&self) -> io::Result<tokio::sync::oneshot::Receiver<()>> {
        let mut receiver = None;
        self.data
            .try_build(std::mem::size_of::<Outbound>(), false, || {
                let (tx, rx) = tokio::sync::oneshot::channel();
                receiver = Some(rx);
                Ok(Outbound::Flush(tx))
            })?;
        Ok(receiver.expect("admitted flush barrier"))
    }
}

/// Dequeue retains the charge. Dropping a consumed message sends its credit;
/// holding it in a request future also holds the sender's delivery allowance.
pub struct Incoming {
    pub frame: Frame,
    receipt: Option<Delivery>,
    sender: Sender,
}
impl Incoming {
    /// Consume this hop while handing its existing payload to another peer.
    /// Use with `Charged::map` and `Sender::data_frame` so the same connection
    /// lease follows the bytes into the next queue. Drop returns this hop's
    /// credit; the next sender assigns its own generation and receipt.
    pub fn into_frame(mut self) -> Frame {
        let kind = self.frame.kind;
        std::mem::replace(
            &mut self.frame,
            Frame {
                kind,
                payload: Vec::new(),
            },
        )
    }
}
impl Drop for Incoming {
    fn drop(&mut self) {
        if let Some(receipt) = self.receipt {
            let _ = self
                .sender
                .receipts
                .send(std::mem::size_of::<Delivery>(), true, || receipt);
        }
    }
}

pub struct Peer {
    pub sender: Sender,
    incoming: queue::Receiver<Incoming>,
    cancel: queue::Receiver<Incoming>,
    cancel_ended: bool,
    closed_reason: Arc<Mutex<Option<Arc<Charged<wire::Closed>>>>>,
    tasks: Vec<JoinHandle<io::Result<()>>>,
}
impl Peer {
    /// Call only after negotiation and application-stream admission. The
    /// context also covers session subscriptions and the surrounding IPC broker.
    pub fn start<R, W>(reader: R, writer: W, ready: &Ready, context: queue::Context) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, out_rx) = context.channel(queue::QUEUE_ITEMS);
        let (control, ctl_rx) = context.channel(CONTROLS);
        let (receipts, receipt_rx) = context.channel(queue::QUEUE_ITEMS);
        let (incoming, in_rx) = context.channel(queue::QUEUE_ITEMS);
        let (cancel, cancel_rx) = context.channel(CONTROLS);
        let sender = Sender {
            context: context.clone(),
            data: outgoing,
            control,
            receipts,
            generation: ready.generation,
            limits: ready.limits.clone(),
        };
        let ledger = Arc::new(Mutex::new(flow::Credits::new(ready.generation)));
        let read_sender = sender.clone();
        let read_ledger = ledger.clone();
        let closed_reason = Arc::new(Mutex::new(None));
        let read_reason = closed_reason.clone();
        let (read_ended, mut ended) = tokio::sync::watch::channel(false);
        let read = tokio::spawn(async move {
            let result = read_loop(
                reader,
                &read_sender,
                read_ledger,
                incoming,
                cancel,
                &read_reason,
            )
            .await;
            // A frame can become ready in the same poll that owner cancellation
            // closes its queue. The lease is dropped by forward; this is a
            // normal end only when cancellation was already in effect.
            let result = match result {
                Err(error)
                    if error.kind() == io::ErrorKind::BrokenPipe
                        && read_sender.context.is_closed() =>
                {
                    Ok(())
                }
                result => result,
            };
            if result.is_err() {
                read_sender.close();
            }
            // EOF closes ingress, not authority: accepted frames still belong
            // to the consumer. Stop writes without revoking those frames.
            read_ended.send_replace(true);
            result
        });
        let write_context = context;
        let write = tokio::spawn(async move {
            let result = tokio::select! { biased;
                _ = ended.wait_for(|done| *done) => Ok(()),
                result = write_loop(writer, out_rx, ctl_rx, receipt_rx, ledger, &write_context) => result,
            };
            if result.is_err() || !*ended.borrow() {
                write_context.close();
            }
            result
        });
        Self {
            sender,
            incoming: in_rx,
            cancel: cancel_rx,
            cancel_ended: false,
            closed_reason,
            tasks: vec![read, write],
        }
    }
    /// The peer's actual terminal `Closed` frame, if one was received. Earlier
    /// accepted application frames may still be queued; drain `recv` to EOF.
    /// Clones share the control-byte charge until the final clone is dropped.
    pub fn closed_reason(&self) -> Option<Arc<Charged<wire::Closed>>> {
        self.closed_reason.lock().expect("closed reason").clone()
    }
    pub async fn recv(&mut self) -> Option<Charged<Incoming>> {
        loop {
            tokio::select! { biased;
                value = self.cancel.recv(), if !self.cancel_ended => match value {
                    Some(value) => return Some(value),
                    None => self.cancel_ended = true,
                },
                value = self.incoming.recv() => {
                    if value.is_none() {
                        self.sender.close();
                    }
                    return value;
                },
            }
        }
    }
    pub async fn finish(mut self) -> io::Result<()> {
        self.sender.close();
        let mut error = None;
        for task in self.tasks.drain(..) {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => {
                    error.get_or_insert(reason);
                }
                Err(reason) => {
                    error.get_or_insert(io::Error::other(reason));
                }
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub async fn shutdown(self) {
        let _ = self.finish().await;
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.sender.close();
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    sender: &Sender,
    ledger: Arc<Mutex<flow::Credits>>,
    incoming: queue::Sender<Incoming>,
    cancel: queue::Sender<Incoming>,
    closed_reason: &Mutex<Option<Arc<Charged<wire::Closed>>>>,
) -> io::Result<()> {
    let mut closing = sender.context.closing();
    let mut ping = time::interval_at(Instant::now() + wire::PING_INTERVAL, wire::PING_INTERVAL);
    ping.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut outstanding: Option<u32> = None;
    // Liveness is time since the last matched pong (or connection start),
    // checked at 15-second ticks, not a fresh 30-second grace for every ping.
    let mut last_pong = Instant::now();
    loop {
        let read = codec::read_limited(
            &mut reader,
            &sender.context.budget,
            sender.limits.frame_bytes as usize,
        );
        tokio::pin!(read);
        let frame = loop {
            tokio::select! { biased;
                _ = closing.wait_for(|closed| *closed) => return Ok(()),
                _ = ping.tick() => {
                    if last_pong.elapsed() >= wire::LIVENESS_TIMEOUT {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "native peer did not answer ping"));
                    }
                    if outstanding.is_none() {
                        let nonce = rand::random(); outstanding = Some(nonce);
                        sender.control(Kind::Ping, &wire::Ping { nonce })?;
                    }
                }
                // Keep this exact future across control ticks. Liveness is
                // polled first so a continuous input flood cannot starve it.
                frame = &mut read => break frame?,
            }
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        match frame.value.kind {
            Kind::Credit => {
                ledger.lock().expect("credit ledger").acknowledge(
                    &*frame
                        .value
                        .decode_control::<wire::Credit>(&sender.context.budget)?,
                )?;
            }
            Kind::Ping => {
                sender.control(
                    Kind::Pong,
                    &*frame
                        .value
                        .decode_control::<wire::Ping>(&sender.context.budget)?,
                )?;
            }
            Kind::Pong => {
                let pong: Charged<wire::Ping> =
                    frame.value.decode_control(&sender.context.budget)?;
                if outstanding == Some(pong.nonce) {
                    outstanding = None;
                    last_pong = Instant::now();
                }
            }
            Kind::Cancel => {
                let message: Charged<wire::Cancel> =
                    frame.value.decode_control(&sender.context.budget)?;
                if message.generation == sender.generation {
                    cancel.forward(frame.map(|frame| Incoming {
                        frame,
                        receipt: None,
                        sender: sender.clone(),
                    }))?;
                }
            }
            Kind::Text | Kind::Binary | Kind::Request | Kind::Reply => {
                let delivery = frame.value.application()?.0;
                if delivery.generation != sender.generation {
                    continue;
                }
                incoming.forward(frame.map(|frame| Incoming {
                    frame,
                    receipt: Some(delivery),
                    sender: sender.clone(),
                }))?;
            }
            Kind::Closed => {
                let reason = frame
                    .value
                    .decode_control::<wire::Closed>(&sender.context.budget)?;
                *closed_reason.lock().expect("closed reason") = Some(Arc::new(reason));
                return Ok(());
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected control after service admission",
                ))
            }
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut data: queue::Receiver<Outbound>,
    mut control: queue::Receiver<Outbound>,
    mut receipts: queue::Receiver<Delivery>,
    ledger: Arc<Mutex<flow::Credits>>,
    context: &queue::Context,
) -> io::Result<()> {
    let mut closing = context.closing();
    loop {
        enum Next {
            Frame(Charged<Outbound>),
            Receipt(Charged<Delivery>),
        }
        let next = tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => return Ok(()),
            value = control.recv() => value.map(Next::Frame),
            value = receipts.recv() => value.map(Next::Receipt),
            value = data.recv() => value.map(Next::Frame),
        };
        let Some(next) = next else {
            return Ok(());
        };
        // Keep compact receipt storage until its encoded frame is actually
        // written. Only this writer allocates one encoded credit at a time.
        let mut receipt = None;
        let frame = match next {
            Next::Frame(frame) => frame,
            Next::Receipt(delivery) => {
                let scratch = context
                    .budget
                    .reserve(32, true)
                    .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
                let credit = flow::credit(*delivery);
                let frame = Frame::control(Kind::Credit, &credit, &context.budget)?;
                drop(credit);
                drop(scratch);
                receipt = Some(delivery);
                frame.map(Outbound::Frame)
            }
        };
        let _receipt = receipt;
        if context.is_closed() {
            return Ok(());
        }
        let (outbound, lease) = frame.into_parts();
        let (frame, confirmed) = match outbound {
            Outbound::Frame(frame) => (frame, None),
            Outbound::Confirmed(frame, confirmed) => (frame, Some(confirmed)),
            Outbound::Flush(done) => {
                let _ = done.send(());
                continue;
            }
        };
        let _writing = if frame.kind.is_data() {
            ledger.lock().expect("credit ledger").track_confirmed(
                frame.application()?.0,
                lease,
                confirmed,
            )?
        } else {
            Arc::new(lease)
        };
        let write = codec::write(&mut writer, &frame);
        tokio::pin!(write);
        // The parent may be closed while a partial write is pending, before
        // its watcher gets scheduled. Recheck on every resumed IO poll, not
        // only once before beginning the frame.
        let write = std::future::poll_fn(|cx| {
            if context.is_closed() {
                Poll::Ready(Ok(()))
            } else {
                write.as_mut().poll(cx)
            }
        });
        tokio::select! { biased;
            _ = closing.wait_for(|closed| *closed) => return Ok(()),
            result = time::timeout(wire::LIVENESS_TIMEOUT, write) => {
                result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "native write stalled"))??;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{ConnectionBudget, ProcessBudget};
    #[tokio::test]
    async fn owner_close_during_final_payload_read_is_clean_cancellation() {
        use std::{
            pin::Pin,
            task::{Context as TaskContext, Poll},
        };
        use tokio::io::{AsyncReadExt, ReadBuf};

        struct CloseAtPayload {
            encoded: Vec<u8>,
            offset: usize,
            context: queue::Context,
        }
        impl AsyncRead for CloseAtPayload {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut TaskContext<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let this = self.get_mut();
                if this.offset >= wire::HEADER_BYTES {
                    this.context.close();
                }
                let length = buf.remaining().min(this.encoded.len() - this.offset);
                buf.put_slice(&this.encoded[this.offset..this.offset + length]);
                this.offset += length;
                Poll::Ready(Ok(()))
            }
        }

        let (mut writer, mut encoded_reader) = tokio::io::duplex(128);
        codec::write(
            &mut writer,
            &Frame::data(Kind::Binary, Delivery::new(3), b"x").unwrap(),
        )
        .await
        .unwrap();
        drop(writer);
        let mut encoded = Vec::new();
        encoded_reader.read_to_end(&mut encoded).await.unwrap();
        let context = queue::Context::default();
        let reader = CloseAtPayload {
            encoded,
            offset: 0,
            context: context.clone(),
        };
        let mut closing = context.closing();
        let peer = Peer::start(reader, tokio::io::sink(), &ready(), context.clone());
        time::timeout(wire::HANDSHAKE_TIMEOUT, closing.wait_for(|closed| *closed))
            .await
            .unwrap()
            .unwrap();
        assert!(
            peer.finish().await.is_ok(),
            "owner cancellation cannot turn a valid frame into a transfer error"
        );
        assert_eq!(
            context.budget.used(),
            0,
            "the discarded frame releases its lease"
        );
    }

    #[tokio::test]
    async fn consumed_burst_does_not_exhaust_ordinary_control_slots() {
        let context = queue::Context::default();
        let (reader, mut remote) = tokio::io::duplex(64 * 1024);
        let (writer, mut blocked_reader) = tokio::io::duplex(1);
        let mut peer = Peer::start(reader, writer, &ready(), context.clone());
        let origin = ConnectionBudget::default();
        let mut credits = flow::Credits::new(3);
        for _ in 0..queue::QUEUE_ITEMS {
            let delivery = Delivery::new(3);
            drop(
                credits
                    .track(delivery, origin.reserve(1, false).unwrap())
                    .unwrap(),
            );
            codec::write(
                &mut remote,
                &Frame::data(Kind::Binary, delivery, b"x").unwrap(),
            )
            .await
            .unwrap();
            drop(peer.recv().await.expect("received frame"));
        }
        assert!(
            !context.is_closed(),
            "receipt burst must not close the connection"
        );
        peer.sender
            .control(Kind::Ping, &wire::Ping { nonce: 17 })
            .unwrap();
        peer.sender
            .control(
                Kind::Cancel,
                &wire::Cancel {
                    generation: 3,
                    id: 8,
                },
            )
            .unwrap();
        let mut kinds = Vec::new();
        for _ in 0..queue::QUEUE_ITEMS + 2 {
            let frame = time::timeout(
                wire::HANDSHAKE_TIMEOUT,
                codec::read(&mut blocked_reader, &context.budget),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            kinds.push(frame.kind);
            if frame.kind == Kind::Credit {
                let credit = frame
                    .decode_control::<wire::Credit>(&context.budget)
                    .unwrap();
                assert!(credits.acknowledge(&credit).unwrap());
            }
        }
        assert_eq!(&kinds[..3], &[Kind::Credit, Kind::Ping, Kind::Cancel]);
        assert!(credits.is_empty());
        assert_eq!(
            origin.used(),
            0,
            "each exact receipt releases its original lease"
        );
        assert!(!context.is_closed());
        peer.shutdown().await;
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn receipt_lane_overflow_closes_and_releases_every_charge() {
        let context = queue::Context::default();
        let (reader, mut remote) = tokio::io::duplex(64 * 1024);
        let (writer, _blocked_reader) = tokio::io::duplex(1);
        let mut peer = Peer::start(reader, writer, &ready(), context.clone());
        for _ in 0..queue::QUEUE_ITEMS + 2 {
            codec::write(
                &mut remote,
                &Frame::data(Kind::Binary, Delivery::new(3), b"x").unwrap(),
            )
            .await
            .unwrap();
            if let Some(message) = peer.recv().await {
                drop(message);
            } else {
                break;
            }
        }
        assert!(context.is_closed(), "receipt backlog remains bounded");
        peer.shutdown().await;
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn cancellation_releases_an_undrained_receipt_backlog() {
        let context = queue::Context::default();
        let (reader, mut remote) = tokio::io::duplex(64 * 1024);
        let (writer, _blocked_reader) = tokio::io::duplex(1);
        let mut peer = Peer::start(reader, writer, &ready(), context.clone());
        for _ in 0..128 {
            codec::write(
                &mut remote,
                &Frame::data(Kind::Binary, Delivery::new(3), b"x").unwrap(),
            )
            .await
            .unwrap();
            drop(peer.recv().await.unwrap());
        }
        assert!(!context.is_closed());
        assert!(context.budget.control.used() > 0);
        peer.shutdown().await;
        assert_eq!(context.budget.used(), 0);
    }
    fn ready() -> Ready {
        Ready {
            generation: 3,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        }
    }
    fn pair() -> (Peer, Peer, Arc<ProcessBudget>) {
        let process = ProcessBudget::new(
            8192 + 2 * wire::CONNECTION_CONTROL_BYTES,
            2 * wire::CONNECTION_CONTROL_BYTES,
        );
        let a = queue::Context::new(ConnectionBudget::new(
            process.clone(),
            1536 + wire::CONNECTION_CONTROL_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        ));
        let b = queue::Context::new(ConnectionBudget::new(
            process.clone(),
            1536 + wire::CONNECTION_CONTROL_BYTES,
            wire::CONNECTION_CONTROL_BYTES,
        ));
        let (a_io, b_io) = tokio::io::duplex(1024);
        let (ar, aw) = tokio::io::split(a_io);
        let (br, bw) = tokio::io::split(b_io);
        (
            Peer::start(ar, aw, &ready(), a),
            Peer::start(br, bw, &ready(), b),
            process,
        )
    }
    #[tokio::test]
    async fn parent_revocation_prevents_a_pending_write_from_resuming_before_its_watcher() {
        use std::{
            pin::Pin,
            sync::atomic::{AtomicBool, AtomicUsize, Ordering},
            task::{Context, Waker},
        };
        #[derive(Default)]
        struct Gate {
            ready: AtomicBool,
            bytes: AtomicUsize,
            waker: Mutex<Option<Waker>>,
            pending: tokio::sync::Notify,
        }
        struct Writer(Arc<Gate>);
        impl AsyncWrite for Writer {
            fn poll_write(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bytes: &[u8],
            ) -> Poll<io::Result<usize>> {
                if !self.0.ready.load(Ordering::SeqCst) {
                    *self.0.waker.lock().unwrap() = Some(cx.waker().clone());
                    self.0.pending.notify_one();
                    return Poll::Pending;
                }
                self.0.bytes.fetch_add(bytes.len(), Ordering::SeqCst);
                Poll::Ready(Ok(bytes.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }
        let owner = queue::Context::default();
        let child = owner.child();
        let (reader, _other_end) = tokio::io::duplex(1024);
        let gate = Arc::new(Gate::default());
        let peer = Peer::start(reader, Writer(gate.clone()), &ready(), child.clone());
        peer.sender
            .data(Kind::Text, b"must not be written")
            .unwrap();
        time::timeout(wire::HANDSHAKE_TIMEOUT, gate.pending.notified())
            .await
            .unwrap();
        owner.close();
        gate.ready.store(true, Ordering::SeqCst);
        gate.waker.lock().unwrap().take().unwrap().wake();
        // No forwarding watcher exists in this fixture. Only the already
        // pending writer is woken; it must notice owner revocation itself.
        let mut closed = child.closing();
        time::timeout(wire::HANDSHAKE_TIMEOUT, closed.wait_for(|closed| *closed))
            .await
            .unwrap()
            .unwrap();
        peer.shutdown().await;
        assert_eq!(gate.bytes.load(Ordering::SeqCst), 0);
        assert_eq!(owner.budget.used(), 0);
    }

    #[tokio::test]
    async fn a_relay_transfers_the_payload_and_charge_until_downstream_consumes_it() {
        let (source, mut relay_in, process) = pair();
        let shared = relay_in.sender.context();
        let (left, right) = tokio::io::duplex(1024);
        let (lr, lw) = tokio::io::split(left);
        let (rr, rw) = tokio::io::split(right);
        let relay_out = Peer::start(lr, lw, &ready(), shared.clone());
        let mut sink = Peer::start(rr, rw, &ready(), queue::Context::default());
        let consumed = source
            .sender
            .data_confirmed(Kind::Binary, &[0, 255, 128])
            .unwrap();
        let incoming = relay_in.recv().await.unwrap();
        let pointer = incoming.frame.payload.as_ptr();
        let before = shared.budget.data.used();
        let frame = incoming.map(Incoming::into_frame);
        assert_eq!(frame.payload.as_ptr(), pointer);
        assert_eq!(shared.budget.data.used(), before);
        relay_out.sender.data_frame(frame).unwrap();
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, consumed)
            .await
            .unwrap()
            .unwrap();
        let received = sink.recv().await.unwrap();
        assert_eq!(received.frame.application().unwrap().1, &[0, 255, 128]);
        assert_eq!(
            shared.budget.data.used(),
            before,
            "downstream still owns its delivery allowance"
        );
        drop(received);
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, async {
            while shared.budget.data.used() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        source.shutdown().await;
        relay_in.shutdown().await;
        relay_out.shutdown().await;
        sink.shutdown().await;
        assert_eq!(process.used(), 0);
    }

    #[tokio::test]
    async fn oversized_control_parse_closes_only_its_carrier_and_releases_reservations() {
        let (server, mut raw) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(server);
        let context = queue::Context::default();
        let mut peer = Peer::start(read, write, &ready(), context.clone());
        let payload = format!("{{\"nonce\":[{}0]}}", "0,".repeat(1500)).into_bytes();
        codec::write(
            &mut raw,
            &Frame {
                kind: Kind::Ping,
                payload,
            },
        )
        .await
        .unwrap();
        assert!(peer.recv().await.is_none());
        assert_eq!(
            peer.finish().await.unwrap_err().kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(context.budget.used(), 0);
        assert!(
            context.budget.control.high_water() <= wire::MAX_CONTROL_BYTES,
            "the denied parser never acquired its oversized reservation"
        );
        let (healthy, mut receiver, process) = pair();
        healthy
            .sender
            .control(
                Kind::Cancel,
                &wire::Cancel {
                    generation: 3,
                    id: 7,
                },
            )
            .unwrap();
        let cancel = receiver.recv().await.unwrap();
        assert_eq!(cancel.frame.decode::<wire::Cancel>().unwrap().id, 7);
        drop(cancel);
        healthy.shutdown().await;
        receiver.shutdown().await;
        assert_eq!(process.used(), 0);
    }

    #[tokio::test]
    async fn confirmed_data_waits_for_the_recipient_and_fails_when_the_carrier_ends() {
        let (a, mut b, process) = pair();
        let mut consumed = a
            .sender
            .data_confirmed(Kind::Binary, &[0, 255, 128])
            .unwrap();
        let held = b.recv().await.unwrap();
        assert!(matches!(
            consumed.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        drop(held);
        tokio::time::timeout(wire::HANDSHAKE_TIMEOUT, consumed)
            .await
            .unwrap()
            .unwrap();
        let unconsumed = a.sender.data_confirmed(Kind::Binary, &[1]).unwrap();
        let held = b.recv().await.unwrap();
        a.shutdown().await;
        assert!(unconsumed.await.is_err());
        drop(held);
        b.shutdown().await;
        assert_eq!(process.used(), 0);
    }
    #[tokio::test]
    async fn writer_barriers_preserve_data_order_without_granting_delivery_credit() {
        let (a, mut b, process) = pair();
        a.sender.data(Kind::Binary, &[0, 255, 128]).unwrap();
        a.sender.flush().unwrap().await.unwrap();
        let held = b.recv().await.unwrap();
        assert_eq!(held.frame.application().unwrap().1, &[0, 255, 128]);
        assert!(
            a.sender.context.budget.data.used() > 0,
            "a local write is not remote consumption"
        );
        drop(held);
        a.shutdown().await;
        b.shutdown().await;
        assert_eq!(process.used(), 0);
    }
    #[tokio::test(start_paused = true)]
    async fn cancellation_and_ping_work_while_data_credit_is_exhausted() {
        let (a, mut b, process) = pair();
        a.sender.data(Kind::Binary, &vec![0; 1490]).unwrap();
        let held = b.recv().await.unwrap();
        assert_eq!(a.sender.context.budget.data.used(), 1515);
        a.sender
            .control(
                Kind::Cancel,
                &wire::Cancel {
                    generation: 3,
                    id: 9,
                },
            )
            .unwrap();
        let cancel = b.recv().await.unwrap();
        assert_eq!(cancel.frame.decode::<wire::Cancel>().unwrap().id, 9);
        drop(cancel);
        for _ in 0..4 {
            time::advance(wire::PING_INTERVAL).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert!(!a.sender.context.is_closed());
            assert!(!b.sender.context.is_closed());
        }
        assert!(process.used() <= 8192 + 2 * wire::CONNECTION_CONTROL_BYTES);
        drop(held);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(a.sender.context.budget.data.used(), 0);
        a.shutdown().await;
        b.shutdown().await;
        assert_eq!(process.used(), 0);
    }
    #[tokio::test]
    async fn application_order_binary_bytes_and_consumption_credit_survive_fragmentation() {
        let (a, mut b, process) = pair();
        a.sender
            .data(Kind::Text, br#"{"type":"mux.welcome"}"#)
            .unwrap();
        a.sender.data(Kind::Binary, &[0, 255, 128, 0]).unwrap();
        let welcome = b.recv().await.unwrap();
        assert_eq!(welcome.frame.kind, Kind::Text);
        drop(welcome);
        let binary = b.recv().await.unwrap();
        assert_eq!(binary.frame.application().unwrap().1, &[0, 255, 128, 0]);
        drop(binary);
        a.shutdown().await;
        b.shutdown().await;
        assert_eq!(process.used(), 0);
    }
    #[tokio::test(start_paused = true)]
    async fn an_unresponsive_peer_closes_with_a_bounded_write_and_liveness_deadline() {
        let (io, _silent) = tokio::io::duplex(1);
        let (read, write) = tokio::io::split(io);
        let context = queue::Context::default();
        let mut peer = Peer::start(read, write, &ready(), context.clone());
        peer.sender.data(Kind::Text, b"unread").unwrap();
        assert!(peer.recv().await.is_none());
        assert!(context.is_closed());
        peer.shutdown().await;
        assert_eq!(context.budget.used(), 0);
    }
    #[tokio::test]
    async fn late_frames_from_an_old_generation_never_reach_the_replacement_client() {
        let (server, mut raw) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(server);
        let context = queue::Context::default();
        let mut peer = Peer::start(read, write, &ready(), context.clone());
        codec::write(
            &mut raw,
            &Frame::data(Kind::Reply, Delivery::new(2), b"old").unwrap(),
        )
        .await
        .unwrap();
        codec::write(
            &mut raw,
            &Frame::data(Kind::Reply, Delivery::new(3), b"current").unwrap(),
        )
        .await
        .unwrap();
        let current = peer.recv().await.unwrap();
        assert_eq!(current.frame.application().unwrap().1, b"current");
        drop(current);
        peer.shutdown().await;
        assert_eq!(context.budget.used(), 0);
    }

    #[tokio::test]
    async fn orderly_end_drains_accepted_frames_but_revocation_and_bad_frames_discard() {
        use tokio::io::AsyncWriteExt;
        for ending in ["eof", "closed", "bad_closed", "revoked", "malformed"] {
            let (server, mut raw) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(server);
            let context = queue::Context::default();
            let mut peer = Peer::start(read, write, &ready(), context.clone());
            for kind in [Kind::Reply, Kind::Text] {
                codec::write(
                    &mut raw,
                    &Frame::data(kind, Delivery::new(3), b"final").unwrap(),
                )
                .await
                .unwrap();
            }
            codec::write(
                &mut raw,
                &Frame::json(
                    Kind::Cancel,
                    &wire::Cancel {
                        generation: 3,
                        id: 1,
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();
            if ending == "closed" {
                codec::write(
                    &mut raw,
                    &Frame::json(
                        Kind::Closed,
                        &wire::Closed {
                            code: wire::CloseCode::Closed,
                            message: "done".into(),
                            retryable: false,
                        },
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            } else if ending == "bad_closed" {
                codec::write(
                    &mut raw,
                    &Frame {
                        kind: Kind::Closed,
                        payload: b"{bad json".to_vec(),
                    },
                )
                .await
                .unwrap();
            } else {
                if ending == "malformed" {
                    raw.write_all(&[255]).await.unwrap();
                }
                raw.shutdown().await.unwrap();
            }
            // Observe actual reader completion before consuming, so this tests
            // buffered terminal frames rather than racing the consumer against EOF.
            let result = peer.tasks.remove(0).await.unwrap();
            assert_eq!(
                result.is_err(),
                matches!(ending, "malformed" | "bad_closed")
            );
            let reason = peer.closed_reason();
            if ending == "closed" {
                let closed = reason.as_ref().expect("received terminal reason");
                assert_eq!(closed.code, wire::CloseCode::Closed);
                assert_eq!(closed.message, "done");
                assert!(!closed.retryable);
                assert!(context.budget.control.used() > 0);
            } else {
                assert!(reason.is_none(), "no fabricated terminal reason");
            }
            if ending == "revoked" {
                context.close();
            }
            if matches!(ending, "revoked" | "malformed" | "bad_closed") {
                assert!(
                    peer.recv().await.is_none(),
                    "hard revocation discards queued input"
                );
            } else {
                assert!(!context.is_closed());
                for expected in [Kind::Cancel, Kind::Reply, Kind::Text] {
                    let frame = peer.recv().await.expect("accepted final frame");
                    assert_eq!(frame.frame.kind, expected);
                }
                assert!(peer.recv().await.is_none());
                assert_eq!(peer.closed_reason().is_some(), ending == "closed");
            }
            let retained = reason.as_ref().map(Arc::clone);
            peer.shutdown().await;
            if ending == "closed" {
                assert!(context.budget.control.used() > 0);
                drop(reason);
                assert!(context.budget.control.used() > 0);
            }
            drop(retained);
            assert_eq!(
                context.budget.used(),
                0,
                "all retained final frames released"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn liveness_counts_from_start_and_only_matched_pongs_extend_it() {
        for matched in [false, true] {
            let (server, mut raw) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(server);
            let context = queue::Context::default();
            let mut peer = Peer::start(read, write, &ready(), context.clone());
            let budget = ConnectionBudget::default();
            let frame = codec::read(&mut raw, &budget).await.unwrap().unwrap();
            let mut ping: wire::Ping = frame.value.decode().unwrap();
            drop(frame);
            if !matched {
                ping.nonce = ping.nonce.wrapping_add(1);
            }
            codec::write(&mut raw, &Frame::json(Kind::Pong, &ping).unwrap())
                .await
                .unwrap();
            // A following Ping/Pong exchange is a barrier for parsing the pong.
            codec::write(
                &mut raw,
                &Frame::json(Kind::Ping, &wire::Ping { nonce: 42 }).unwrap(),
            )
            .await
            .unwrap();
            let pong = codec::read(&mut raw, &budget).await.unwrap().unwrap();
            assert_eq!(pong.value.kind, Kind::Pong);
            drop(pong);
            time::advance(wire::PING_INTERVAL).await;
            if matched {
                let next = codec::read(&mut raw, &budget).await.unwrap().unwrap();
                assert_eq!(next.value.kind, Kind::Ping);
                drop(next);
                assert!(!context.is_closed());
                time::advance(wire::PING_INTERVAL).await;
            }
            let error = peer.tasks.remove(0).await.unwrap().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            peer.shutdown().await;
            assert_eq!(context.budget.used(), 0);
        }
    }

    #[tokio::test]
    async fn finishing_distinguishes_bad_framing_from_a_clean_eof() {
        use tokio::io::AsyncWriteExt;
        for malformed in [false, true] {
            let (server, mut raw) = tokio::io::duplex(64);
            let (read, write) = tokio::io::split(server);
            let context = queue::Context::default();
            let mut peer = Peer::start(read, write, &ready(), context.clone());
            if malformed {
                raw.write_all(&[255, 0, 0, 0, 0]).await.unwrap();
            } else {
                raw.shutdown().await.unwrap();
            }
            assert!(peer.recv().await.is_none());
            assert_eq!(peer.finish().await.is_err(), malformed);
            assert_eq!(context.budget.used(), 0);
        }
    }
}
