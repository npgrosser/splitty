//! The attached session: the session protocol on one side, the terminal on
//! the other. Written over a message stream and a writer, so a scripted server
//! can drive it in tests.

use std::io::Write;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use serde_json::{json, Value};
use splitty_core::protocol::{MSG_ACTIVITY, MSG_ATTACH, MSG_DETACH, MSG_INPUT, MSG_RESIZE};
use splitty_core::pty::replay::ReplayFilter;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use super::input::{DetachKey, Utf8Input};
use super::Outcome;
use crate::cli::CliError;

/// At most one `activity` per this interval while the user types, as the web
/// client sends it; well within the shortest idle time of the size policy.
const ACTIVITY_INTERVAL: Duration = Duration::from_secs(5);

/// What happens in the terminal.
#[derive(Debug)]
pub enum Event {
    Input(Vec<u8>),
    Resized(u16, u16),
    /// The terminal is gone: nobody types or reads anymore.
    HungUp,
    /// SIGTERM: detach as the detach key would.
    Terminated,
}

/// The session to show, and how.
pub struct Attach {
    pub session_id: String,
    pub detach_key: DetachKey,
    /// The terminal's columns and rows.
    pub size: (u16, u16),
    /// The terminal shows the session already (it was attached to its keeper
    /// until the daemon took that over): drop the replay, keep the redraw.
    pub skip_replay: bool,
}

enum Phase {
    /// Before `mux.welcome`.
    Connecting,
    /// From `attach` to `attached`: frames are the buffered output.
    Replay(ReplayFilter),
    Live,
}

/// Show the session until the user detaches, its process ends, the terminal
/// goes or the connection is lost.
pub async fn run<Tx, Rx, RxError>(
    attach: Attach,
    mut to_server: Tx,
    mut from_server: Rx,
    events: &mut mpsc::Receiver<Event>,
    screen: &mut impl Write,
) -> Result<Outcome, CliError>
where
    Tx: Sink<Message> + Unpin,
    Rx: Stream<Item = Result<Message, RxError>> + Unpin,
{
    let id = attach.session_id.as_str();
    let mut size = attach.size;
    let mut phase = Phase::Connecting;
    let mut text = Utf8Input::default();
    let mut last_activity: Option<Instant> = None;

    let outcome = loop {
        tokio::select! {
            message = from_server.next() => match message {
                Some(Ok(Message::Text(message))) => {
                    let Ok(event) = serde_json::from_str::<Value>(message.as_str()) else {
                        continue;
                    };
                    let mine = event["sessionId"].as_str() == Some(id);
                    match event["type"].as_str().unwrap_or_default() {
                        "mux.welcome" if matches!(phase, Phase::Connecting) => {
                            // Attaching is deliberate: this terminal takes the
                            // PTY size, as the size badge's takeover would.
                            send(&mut to_server, json!({"type": MSG_ACTIVITY, "takeover": true})).await?;
                            last_activity = Some(Instant::now());
                            send(&mut to_server, json!({"type": MSG_ATTACH, "id": id})).await?;
                            phase = Phase::Replay(ReplayFilter::new());
                        }
                        "attached" if mine => {
                            if let Phase::Replay(filter) = &mut phase {
                                let rest = filter.finish();
                                if !attach.skip_replay {
                                    show(screen, &rest)?;
                                }
                                phase = Phase::Live;
                                send(&mut to_server, resize(id, size, true)).await?;
                            }
                        }
                        "closed" if mine => {
                            break match (event["signal"].as_str(), event["exitCode"].as_i64()) {
                                (Some(signal), _) => Outcome::Signaled(signal.to_owned()),
                                (None, Some(code)) => Outcome::Exited(code),
                                (None, None) => Outcome::Closed,
                            };
                        }
                        "error" if mine => {
                            let error = event["error"].as_str().unwrap_or("unknown error");
                            return Err(CliError::Failed(error.to_owned()));
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Binary(frame))) => {
                    let Some((session, bytes)) = crate::ws::codec::decode_binary_frame(&frame) else {
                        continue;
                    };
                    if session != id {
                        continue;
                    }
                    match &mut phase {
                        Phase::Connecting => {}
                        Phase::Replay(_) if attach.skip_replay => {}
                        Phase::Replay(filter) => show(screen, &filter.push(bytes))?,
                        Phase::Live => show(screen, bytes)?,
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(lost()),
                Some(Ok(_)) => {}
            },
            event = events.recv() => match event {
                Some(Event::Input(bytes)) => {
                    let (typed, detached) = match attach.detach_key.find(&bytes) {
                        Some(at) => (&bytes[..at], true),
                        None => (&bytes[..], false),
                    };
                    let data = text.push(typed);
                    if !data.is_empty() {
                        send(&mut to_server, json!({"type": MSG_INPUT, "id": id, "data": data})).await?;
                        if last_activity.is_none_or(|at| at.elapsed() >= ACTIVITY_INTERVAL) {
                            send(&mut to_server, json!({"type": MSG_ACTIVITY})).await?;
                            last_activity = Some(Instant::now());
                        }
                    }
                    if detached {
                        break Outcome::Detached;
                    }
                }
                Some(Event::Resized(cols, rows)) => {
                    size = (cols, rows);
                    if matches!(phase, Phase::Live) {
                        send(&mut to_server, resize(id, size, false)).await?;
                    }
                }
                Some(Event::Terminated) => break Outcome::Detached,
                // Nothing goes to the session for a terminal that is gone.
                Some(Event::HungUp) | None => {
                    let _ = to_server.close().await;
                    return Ok(Outcome::HungUp);
                }
            },
        }
    };
    if outcome == Outcome::Detached {
        send(&mut to_server, json!({"type": MSG_DETACH, "id": id})).await?;
    }
    let _ = to_server.close().await;
    Ok(outcome)
}

fn resize(id: &str, (cols, rows): (u16, u16), redraw: bool) -> Value {
    let mut message = json!({"type": MSG_RESIZE, "id": id, "cols": cols, "rows": rows});
    if redraw {
        message["redraw"] = json!(true);
    }
    message
}

async fn send<Tx>(to_server: &mut Tx, message: Value) -> Result<(), CliError>
where
    Tx: Sink<Message> + Unpin,
{
    to_server
        .send(Message::text(message.to_string()))
        .await
        .map_err(|_| lost())
}

fn lost() -> CliError {
    CliError::Failed("lost the connection to the server".into())
}

/// Output for the terminal; a write that fails ends the attach.
fn show(screen: &mut impl Write, bytes: &[u8]) -> Result<(), CliError> {
    if bytes.is_empty() {
        return Ok(());
    }
    screen
        .write_all(bytes)
        .and_then(|()| screen.flush())
        .map_err(|e| CliError::Failed(format!("cannot write to the terminal: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::codec::encode_binary_frame;
    use futures::channel::mpsc as channel;

    const SID: &str = "sess-1";

    /// What the client showed, readable while it runs.
    #[derive(Clone, Default)]
    struct Screen(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Screen {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("screen").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Screen {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().expect("screen").clone()
        }
    }

    /// A scripted server and terminal around one attach.
    struct Harness {
        to_client: channel::UnboundedSender<Result<Message, String>>,
        from_client: channel::UnboundedReceiver<Message>,
        events: mpsc::Sender<Event>,
        screen: Screen,
        task: tokio::task::JoinHandle<Result<Outcome, CliError>>,
    }

    impl Harness {
        fn start(detach_key: DetachKey) -> Self {
            Self::start_with(detach_key, false)
        }

        fn start_with(detach_key: DetachKey, skip_replay: bool) -> Self {
            let (to_client, from_server) = channel::unbounded();
            let (to_server, from_client) = channel::unbounded();
            let (events, mut receiver) = mpsc::channel(16);
            let screen = Screen::default();
            let mut shown = screen.clone();
            let task = tokio::spawn(async move {
                let attach = Attach {
                    session_id: SID.into(),
                    detach_key,
                    size: (80, 24),
                    skip_replay,
                };
                run(attach, to_server, from_server, &mut receiver, &mut shown).await
            });
            Self {
                to_client,
                from_client,
                events,
                screen,
                task,
            }
        }

        fn server_sends(&self, event: Value) {
            self.to_client
                .unbounded_send(Ok(Message::text(event.to_string())))
                .expect("client listens");
        }

        fn server_sends_output(&self, session: &str, bytes: &[u8]) {
            self.to_client
                .unbounded_send(Ok(Message::binary(encode_binary_frame(session, bytes))))
                .expect("client listens");
        }

        async fn user(&self, event: Event) {
            self.events.send(event).await.expect("client listens");
        }

        async fn next_sent(&mut self) -> Value {
            let message = self.from_client.next().await.expect("a message");
            serde_json::from_str(message.to_text().expect("text")).expect("JSON")
        }

        /// Wait until the terminal shows `expected`.
        async fn shows(&self, expected: &[u8]) {
            for _ in 0..500 {
                if self.screen.bytes() == expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            panic!(
                "the terminal shows {:?}, not {:?}",
                String::from_utf8_lossy(&self.screen.bytes()),
                String::from_utf8_lossy(expected)
            );
        }

        /// Welcome, the replay and `attached`; returns the messages sent so far.
        async fn attach(&mut self, replay: &[u8]) -> Vec<Value> {
            self.server_sends(json!({"type": "mux.welcome", "clientId": "c", "screens": []}));
            let first = vec![self.next_sent().await, self.next_sent().await];
            self.server_sends(json!({"type": "size", "sessionId": SID, "cols": 120, "rows": 40}));
            self.server_sends_output(SID, replay);
            self.server_sends(json!({"type": "attached", "sessionId": SID}));
            let mut sent = first;
            sent.push(self.next_sent().await);
            sent
        }

        /// The outcome, what the terminal showed and the messages sent after
        /// the last one the test read.
        async fn finish(self) -> (Result<Outcome, CliError>, Vec<u8>, Vec<Value>) {
            let Self {
                to_client,
                from_client,
                events,
                screen,
                task,
            } = self;
            let outcome = task.await.expect("client task");
            drop((to_client, events));
            let rest = from_client
                .map(|m| serde_json::from_str(m.to_text().expect("text")).expect("JSON"))
                .collect()
                .await;
            (outcome, screen.bytes(), rest)
        }
    }

    #[tokio::test]
    async fn the_terminal_takes_the_size_then_detaches_and_the_session_keeps_running() {
        let mut client = Harness::start(DetachKey::default());
        let sent = client.attach(b"$ ").await;
        assert_eq!(sent[0], json!({"type": "activity", "takeover": true}));
        assert_eq!(sent[1], json!({"type": "attach", "id": SID}));
        assert_eq!(
            sent[2],
            json!({"type": "resize", "id": SID, "cols": 80, "rows": 24, "redraw": true})
        );

        client.user(Event::Resized(100, 30)).await;
        assert_eq!(
            client.next_sent().await,
            json!({"type": "resize", "id": SID, "cols": 100, "rows": 30})
        );

        client.server_sends_output("someone-else", b"not mine");
        client.server_sends_output(SID, b"hello");
        client.shows(b"$ hello").await;
        client.user(Event::Input(b"ls\r\x1c".to_vec())).await;
        let (outcome, screen, rest) = client.finish().await;
        assert_eq!(outcome.expect("detached"), Outcome::Detached);
        assert_eq!(screen, b"$ hello");
        assert_eq!(
            rest,
            [
                json!({"type": "input", "id": SID, "data": "ls\r"}),
                json!({"type": "detach", "id": SID}),
            ],
            "a detach, never a close"
        );
    }

    #[tokio::test]
    async fn the_replay_asks_the_terminal_nothing_and_live_output_is_unchanged() {
        let mut client = Harness::start(DetachKey::default());
        client
            .attach(b"old\x1b]11;?\x07\x1b]52;c;eA==\x07 prompt")
            .await;
        client.server_sends_output(SID, b"\x1b]11;?\x07");
        client.server_sends(json!({"type": "closed", "sessionId": SID, "exitCode": 0}));
        let (outcome, screen, _) = client.finish().await;
        assert_eq!(outcome.expect("exited"), Outcome::Exited(0));
        assert_eq!(screen, b"old prompt\x1b]11;?\x07");
    }

    #[tokio::test(start_paused = true)]
    async fn typing_counts_as_activity_at_most_every_five_seconds() {
        let mut client = Harness::start(DetachKey::default());
        client.attach(b"").await;
        client.user(Event::Input(b"a".to_vec())).await;
        assert_eq!(client.next_sent().await["type"], "input");
        tokio::time::advance(ACTIVITY_INTERVAL).await;
        client.user(Event::Input(b"b".to_vec())).await;
        assert_eq!(client.next_sent().await["type"], "input");
        assert_eq!(client.next_sent().await, json!({"type": "activity"}));
        client.user(Event::Input(b"c".to_vec())).await;
        assert_eq!(client.next_sent().await["type"], "input");
        client.user(Event::Terminated).await;
        assert_eq!(client.next_sent().await["type"], "detach");
        let (outcome, _, rest) = client.finish().await;
        assert_eq!(outcome.expect("detached"), Outcome::Detached);
        assert!(rest.is_empty(), "no activity within five seconds: {rest:?}");
    }

    #[tokio::test]
    async fn an_ended_process_is_reported_with_its_code_signal_or_neither() {
        for (closed, expected) in [
            (json!({"exitCode": 3}), Outcome::Exited(3)),
            (
                json!({"exitCode": 137, "signal": "SIGKILL"}),
                Outcome::Signaled("SIGKILL".into()),
            ),
            (json!({}), Outcome::Closed),
        ] {
            let mut client = Harness::start(DetachKey::default());
            client.attach(b"").await;
            client.server_sends_output(SID, b"last words");
            let mut event = closed.clone();
            event["type"] = json!("closed");
            event["sessionId"] = json!(SID);
            client.server_sends(event);
            let (outcome, screen, _) = client.finish().await;
            assert_eq!(outcome.expect("ended"), expected, "{closed}");
            assert_eq!(screen, b"last words");
        }
    }

    #[tokio::test]
    async fn a_lost_connection_and_an_unknown_session_are_errors() {
        let mut client = Harness::start(DetachKey::default());
        client.attach(b"").await;
        client.to_client.close_channel();
        let (outcome, _, _) = client.finish().await;
        assert!(
            matches!(outcome, Err(CliError::Failed(m)) if m == "lost the connection to the server")
        );

        let mut client = Harness::start(DetachKey::default());
        client.server_sends(json!({"type": "mux.welcome"}));
        client.next_sent().await;
        client.next_sent().await;
        client
            .server_sends(json!({"type": "error", "sessionId": SID, "error": "session not found"}));
        let (outcome, _, _) = client.finish().await;
        assert!(matches!(outcome, Err(CliError::Failed(m)) if m == "session not found"));
    }

    #[tokio::test]
    async fn a_handover_from_a_keeper_shows_no_replay_but_redraws() {
        let mut client = Harness::start_with(DetachKey::default(), true);
        let sent = client.attach(b"already on screen").await;
        assert_eq!(
            sent[2],
            json!({"type": "resize", "id": SID, "cols": 80, "rows": 24, "redraw": true})
        );
        client.server_sends_output(SID, b"live");
        client.shows(b"live").await;
        client.user(Event::Input(b"\x1c".to_vec())).await;
        let (outcome, screen, _) = client.finish().await;
        assert_eq!(outcome.expect("detached"), Outcome::Detached);
        assert_eq!(screen, b"live", "the replay is not shown again");
    }

    #[tokio::test]
    async fn a_hangup_sends_nothing_to_the_session() {
        let mut client = Harness::start(DetachKey::default());
        client.attach(b"").await;
        client.user(Event::HungUp).await;
        let (outcome, _, rest) = client.finish().await;
        assert_eq!(outcome.expect("hung up"), Outcome::HungUp);
        assert!(rest.is_empty(), "{rest:?}");
    }

    #[tokio::test]
    async fn another_detach_key_lets_ctrl_backslash_through() {
        let mut client = Harness::start("ctrl-]".parse().expect("key"));
        client.attach(b"").await;
        client.user(Event::Input(b"\x1c".to_vec())).await;
        assert_eq!(
            client.next_sent().await,
            json!({"type": "input", "id": SID, "data": "\u{1c}"})
        );
        client.user(Event::Input(b"\x1d".to_vec())).await;
        let (outcome, _, rest) = client.finish().await;
        assert_eq!(outcome.expect("detached"), Outcome::Detached);
        assert_eq!(rest, [json!({"type": "detach", "id": SID})]);
    }
}
