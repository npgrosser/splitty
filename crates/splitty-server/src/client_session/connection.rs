//! A connection owns exactly one session identity and its cleanup scope.
use super::{handler, sender::ClientSender, ClientState};
use splitty_transport::{admission, queue, wire};
use std::sync::Arc;

#[derive(Debug)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

pub struct Connection {
    pub input: Input,
    pub output: queue::Receiver<Message>,
    sender: Arc<ClientSender>,
    worker: Option<tokio::task::JoinHandle<()>>,
}

pub fn connect(state: Arc<ClientState>) -> std::io::Result<Connection> {
    connect_with(state, queue::Context::default())
}

pub fn connect_with(
    state: Arc<ClientState>,
    context: queue::Context,
) -> std::io::Result<Connection> {
    let assignment = admission::Assignment::new(state.admission.clone(), admission::Domain::new());
    let stream = admission::Admission::global()
        .begin(assignment)
        .and_then(|pending| pending.admit())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e))?;
    Ok(connect_admitted(state, context, stream, true))
}

pub fn connect_admitted(
    state: Arc<ClientState>,
    context: queue::Context,
    stream: admission::Stream,
    local_hub: bool,
) -> Connection {
    let (input, incoming) = context.channel(queue::QUEUE_ITEMS);
    let (sender, output) = ClientSender::with_context(context.clone());
    let sender = Arc::new(sender);
    let task_sender = sender.clone();
    let worker = tokio::spawn(async move {
        let _stream = stream;
        handler::run(incoming, task_sender, state, local_hub).await;
    });
    Connection {
        input: Input { tx: input, context },
        output,
        sender,
        worker: Some(worker),
    }
}

#[derive(Clone)]
pub struct Input {
    tx: queue::Sender<Message>,
    context: queue::Context,
}
impl Input {
    /// Move an already charged carrier buffer into the same connection queue.
    pub(crate) fn forward(
        &self,
        message: splitty_transport::budget::Charged<Message>,
    ) -> std::io::Result<()> {
        self.tx.forward(message)
    }
    pub fn send_text(&self, text: &str) -> std::io::Result<()> {
        let size = text
            .len()
            .saturating_add(wire::HEADER_BYTES + wire::DELIVERY_BYTES);
        if size > wire::MAX_FRAME_BYTES {
            self.context.close();
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session frame exceeds limit",
            ));
        }
        self.tx.send(size, false, || Message::Text(text.to_owned()))
    }
    pub fn send(&self, message: Message) -> std::io::Result<()> {
        let (length, control) = match &message {
            Message::Text(text) => (text.len(), false),
            Message::Binary(bytes) => (bytes.len(), false),
            Message::Ping(bytes) | Message::Pong(bytes) => (bytes.len(), true),
            Message::Close => (0, true),
        };
        let size = length.saturating_add(wire::HEADER_BYTES + wire::DELIVERY_BYTES);
        if size > wire::MAX_FRAME_BYTES || (control && size > wire::MAX_CONTROL_BYTES) {
            self.context.close();
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session frame exceeds limit",
            ));
        }
        self.tx.send(size, control, || message)
    }
}

impl Connection {
    pub fn closing(&self) -> tokio::sync::watch::Receiver<bool> {
        self.sender.closing()
    }

    pub async fn shutdown(mut self) {
        self.sender.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.sender.close();
    }
}
