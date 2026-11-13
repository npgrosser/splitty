//! One ordered, byte-bounded output queue per service client.
use super::Message;
use splitty_core::protocol::{self, ServerEvent};
use splitty_transport::{codec, queue, wire};
use std::io;

pub struct ClientSender {
    tx: queue::Sender<Message>,
    context: queue::Context,
}

impl ClientSender {
    pub fn channel() -> (Self, queue::Receiver<Message>) {
        Self::with_context(queue::Context::default())
    }
    pub fn with_context(context: queue::Context) -> (Self, queue::Receiver<Message>) {
        let (tx, rx) = context.channel(queue::QUEUE_ITEMS);
        (Self { tx, context }, rx)
    }
    pub fn context(&self) -> queue::Context {
        self.context.clone()
    }
    pub fn closing(&self) -> tokio::sync::watch::Receiver<bool> {
        self.context.closing()
    }
    pub fn close(&self) {
        self.context.close();
    }
    /// Notifications already own a counted buffer in this same connection.
    pub(super) fn forward(
        &self,
        message: splitty_transport::budget::Charged<Message>,
    ) -> io::Result<()> {
        self.tx.forward(message)
    }
    fn data(&self, length: usize, make: impl FnOnce() -> io::Result<Message>) -> io::Result<()> {
        let size = length
            .checked_add(wire::HEADER_BYTES + wire::DELIVERY_BYTES)
            .filter(|n| *n <= wire::MAX_FRAME_BYTES)
            .ok_or_else(|| {
                self.close();
                io::Error::new(io::ErrorKind::InvalidData, "session frame exceeds limit")
            })?;
        self.tx
            .try_build(size, false, make)
            .inspect_err(|_| self.close())
    }
    pub fn try_send_text(&self, text: &str) -> io::Result<()> {
        self.data(text.len(), || Ok(Message::Text(text.to_owned())))
    }
    pub async fn send_text(&self, text: &str) -> io::Result<()> {
        self.try_send_text(text)
    }
    pub async fn send_binary_data(&self, data: Vec<u8>) -> io::Result<()> {
        self.data(data.len(), || Ok(Message::Binary(data)))
    }
    pub async fn send_event(&self, event: &ServerEvent) -> io::Result<()> {
        self.event(event)
    }
    pub fn try_send_json<T: serde::Serialize>(&self, value: &T) -> io::Result<()> {
        let len = codec::json_size(
            value,
            wire::MAX_FRAME_BYTES - wire::HEADER_BYTES - wire::DELIVERY_BYTES,
        )
        .inspect_err(|_| self.close())?;
        self.data(len, || {
            // serde_json::to_string may grow past the charged length. Allocate
            // the measured capacity once and retain its bytes in the queue.
            String::from_utf8(codec::json_bytes(value, len)?)
                .map(Message::Text)
                .map_err(io::Error::other)
        })
    }
    fn event(&self, event: &ServerEvent) -> io::Result<()> {
        self.try_send_json(event)
    }
}

impl protocol::Sender for ClientSender {
    fn delivery_context(&self) -> queue::Context {
        self.context()
    }
    fn close(&self) {
        ClientSender::close(self);
    }
    fn send(&self, event: &ServerEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.event(event)?;
        Ok(())
    }
    fn send_binary(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.data(session_id.len() + 2 + data.len(), || {
            Ok(Message::Binary(super::codec::encode_binary_frame(
                session_id, data,
            )))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::{ConnectionBudget, ProcessBudget};

    #[tokio::test]
    async fn escaped_json_has_exact_charged_capacity_until_consumed() {
        let process = ProcessBudget::new(8192, 128);
        let budget = ConnectionBudget::new(process.clone(), 8192, 128);
        let (sender, mut output) = ClientSender::with_context(queue::Context::new(budget.clone()));
        let event = serde_json::json!({"type":"test", "text":"\"\\\n界".repeat(100)});
        sender.try_send_json(&event).unwrap();
        let frame = output.recv().await.unwrap();
        let Message::Text(text) = &frame.value else {
            panic!("JSON text");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text).unwrap(),
            event
        );
        assert_eq!(text.capacity(), text.len());
        assert_eq!(
            budget.used(),
            text.capacity() + wire::HEADER_BYTES + wire::DELIVERY_BYTES
        );
        drop(frame);
        assert_eq!(process.used(), 0);
    }

    #[tokio::test]
    async fn notification_fanout_disconnects_a_full_recipient_and_transfers_healthy_buffers() {
        use splitty_core::protocol::notify::NotificationBus;
        use std::sync::Arc;
        let process = ProcessBudget::new(2048, 128);
        let slow = queue::Context::new(ConnectionBudget::new(process.clone(), 512, 64));
        let healthy = queue::Context::new(ConnectionBudget::new(process.clone(), 512, 64));
        let (slow_sender, mut slow_rx) = ClientSender::with_context(slow.clone());
        let (healthy_sender, mut healthy_rx) = ClientSender::with_context(healthy.clone());
        let (output, mut output_rx) = ClientSender::with_context(healthy.clone());
        let bus = NotificationBus::default();
        let _slow = bus.subscribe(Arc::new(slow_sender));
        let _healthy = bus.subscribe(Arc::new(healthy_sender));
        let event = ServerEvent::mux_nudge("x".repeat(120), None);
        for _ in 0..10 {
            bus.send(&event);
            let message = healthy_rx.recv().await.expect("healthy subscriber");
            let Message::Text(text) = &message.value else {
                panic!("text notification");
            };
            let pointer = text.as_ptr();
            let charged = healthy.budget.used();
            output.forward(message).unwrap();
            assert_eq!(
                healthy.budget.used(),
                charged,
                "forward neither copies nor releases bytes"
            );
            let delivered = output_rx.recv().await.unwrap();
            let Message::Text(text) = &delivered.value else {
                panic!("text delivery");
            };
            assert_eq!(text.as_ptr(), pointer);
            drop(delivered);
            assert_eq!(healthy.budget.used(), 0);
        }
        assert!(slow.is_closed());
        assert!(!healthy.is_closed());
        assert!(slow_rx.recv().await.is_none());
        assert!(slow.budget.data.high_water() <= 448);
        assert!(process.data.high_water() <= 1920);
        assert_eq!(process.used(), 0);
    }
}
