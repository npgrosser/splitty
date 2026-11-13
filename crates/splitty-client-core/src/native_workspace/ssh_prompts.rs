//! The SSH questions of the local service (remote-hosts, SSH Questions Of The
//! Local Service), read and answered over an API stream of their own. A
//! service that announces `ssh-prompts-v1` is read after each
//! `ssh.prompts.changed`, and the stream is opened at the first one; an older
//! service is polled from the start until it refuses a read.
use super::HubSetup;
use crate::api_stream::{decode, ApiStream};
use serde::Deserialize;
use serde_json::{json, Map};
use splitty_core::native_api::Operation;
use std::{collections::VecDeque, future::Future, io, pin::Pin, sync::Arc, time::Duration};
use tokio::time::Instant;

/// At most this many questions are shown, as by the web client.
const MAX_SSH_PROMPTS: usize = 16;
const MAX_TEXT: usize = 16 * 1024;
const MAX_TARGET: usize = 4096;
const MAX_QUEUED_ANSWERS: usize = 16;
const POLL: Duration = Duration::from_secs(1);

/// One question OpenSSH asked the local service: a password or passphrase,
/// or with `confirm` a host key to trust. The text is the service's and SSH's,
/// untrusted, for display only.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SshPrompt {
    pub id: String,
    pub target: String,
    pub prompt: String,
    pub confirm: bool,
}
impl SshPrompt {
    fn valid(&self) -> bool {
        self.id.len() == 64
            && self.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.target.len() <= MAX_TARGET
            && self.prompt.len() <= MAX_TEXT
    }
}

/// An answer the service accepts: within its size limit and on one line.
/// `None` cancels the question.
fn valid_answer(answer: Option<&str>) -> bool {
    answer.is_none_or(|answer| answer.len() <= MAX_TEXT && !answer.contains(['\n', '\r', '\0']))
}

type Outcome = io::Result<Vec<SshPrompt>>;
type Job = Pin<Box<dyn Future<Output = (ApiStream, Outcome)> + Send>>;

pub(super) struct RuntimeSshPrompts<'a> {
    service: &'a crate::owned_service::OwnedService,
    /// The service does not notify: read every [`POLL`].
    polled: bool,
    setup: Option<HubSetup<'a>>,
    setup_context: Option<splitty_transport::queue::Context>,
    stream: Option<ApiStream>,
    job: Option<Job>,
    answers: VecDeque<(String, Option<String>)>,
    read_due: bool,
    next_poll: Instant,
    /// The stream could not be opened or a read failed; nothing is shown.
    stopped: bool,
    known: Arc<Vec<SshPrompt>>,
}
impl<'a> RuntimeSshPrompts<'a> {
    /// `notified`: the service announced `ssh-prompts-v1`.
    pub(super) fn new(service: &'a crate::owned_service::OwnedService, notified: bool) -> Self {
        let mut prompts = Self {
            service,
            polled: !notified,
            setup: None,
            setup_context: None,
            stream: None,
            job: None,
            answers: VecDeque::new(),
            read_due: !notified,
            next_poll: Instant::now(),
            stopped: false,
            known: Arc::default(),
        };
        if prompts.polled {
            prompts.open();
        }
        prompts
    }
    fn open(&mut self) {
        if self.stream.is_none() && self.job.is_none() && self.setup.is_none() && !self.stopped {
            let context = splitty_transport::queue::Context::default();
            self.setup = Some(Box::pin(
                self.service.api_stream_with_context(context.clone()),
            ));
            self.setup_context = Some(context);
        }
    }
    /// `ssh.prompts.changed` arrived on the local connection.
    pub(super) fn changed(&mut self) {
        self.read_due = true;
        self.open();
    }
    /// Answer one question; the list is read again afterwards.
    pub(super) fn answer(&mut self, id: String, answer: Option<String>) -> io::Result<()> {
        if !valid_answer(answer.as_deref()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The answer must be one line of at most 16 KiB",
            ));
        }
        if self.stopped {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SSH questions are not available",
            ));
        }
        if self.answers.len() >= MAX_QUEUED_ANSWERS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Too many SSH answers are waiting",
            ));
        }
        self.answers.retain(|(queued, _)| *queued != id);
        self.answers.push_back((id, answer));
        self.open();
        Ok(())
    }
    /// The questions whenever they differ from the last list published.
    pub(super) async fn next_update(&mut self) -> Arc<Vec<SshPrompt>> {
        loop {
            if self.stopped {
                return std::future::pending().await;
            }
            if self.job.is_none() && self.stream.is_none() {
                let Some(setup) = &mut self.setup else {
                    return std::future::pending().await;
                };
                let result = setup.await;
                self.setup = None;
                self.setup_context = None;
                match result {
                    Ok(stream) => self.stream = Some(stream),
                    Err(_) => {
                        if let Some(list) = self.stop() {
                            return list;
                        }
                        continue;
                    }
                }
            }
            if self.job.is_none() {
                let answer = self.answers.pop_front();
                if answer.is_none() && !self.read_due {
                    if !self.polled {
                        return std::future::pending().await;
                    }
                    tokio::time::sleep_until(self.next_poll).await;
                }
                self.read_due = false;
                let mut stream = self.stream.take().expect("prompt stream retained");
                self.job = Some(Box::pin(async move {
                    if let Some((id, answer)) = answer {
                        // A question that went meanwhile is not accepted;
                        // the read below shows what is still asked.
                        let _ = stream
                            .request(
                                Operation::SshAnswer,
                                Map::from_iter([(
                                    "body".into(),
                                    json!({"id": id, "answer": answer}),
                                )]),
                            )
                            .await;
                    }
                    let outcome = read(&mut stream).await;
                    (stream, outcome)
                }));
            }
            let (stream, outcome) = self.job.as_mut().expect("job started").await;
            self.job = None;
            self.stream = Some(stream);
            self.next_poll = Instant::now() + POLL;
            match outcome {
                Ok(prompts) if *self.known != prompts => {
                    self.known = Arc::new(prompts);
                    return self.known.clone();
                }
                Ok(_) => {}
                Err(_) => {
                    if let Some(list) = self.stop() {
                        return list;
                    }
                }
            }
        }
    }
    /// Stop reading; an empty list when questions were shown.
    fn stop(&mut self) -> Option<Arc<Vec<SshPrompt>>> {
        self.stopped = true;
        self.answers.clear();
        (!self.known.is_empty()).then(|| {
            self.known = Arc::default();
            self.known.clone()
        })
    }
    pub(super) async fn finish(mut self) -> io::Result<()> {
        if let Some(context) = self.setup_context.take() {
            context.close();
        }
        if let Some(setup) = self.setup.take() {
            self.stream = setup.await.ok();
        }
        if let Some(job) = self.job.take() {
            self.stream = Some(job.await.0);
        }
        match self.stream.take() {
            Some(stream) => stream.finish().await,
            None => Ok(()),
        }
    }
}

/// The valid questions the service lists, at most [`MAX_SSH_PROMPTS`].
async fn read(stream: &mut ApiStream) -> io::Result<Vec<SshPrompt>> {
    let reply = stream.request(Operation::SshPrompts, Map::new()).await?;
    let listed = decode::<Vec<SshPrompt>>(stream, &reply)?;
    Ok(listed
        .iter()
        .filter(|prompt| prompt.valid())
        .take(MAX_SSH_PROMPTS)
        .cloned()
        .collect())
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn only_one_line_answers_within_the_limit_are_sent() {
        assert!(valid_answer(None));
        assert!(valid_answer(Some("private password")));
        assert!(valid_answer(Some(&"x".repeat(MAX_TEXT))));
        assert!(!valid_answer(Some(&"x".repeat(MAX_TEXT + 1))));
        for broken in ["two\nlines", "carriage\rreturn", "nul\0byte"] {
            assert!(!valid_answer(Some(broken)), "{broken:?}");
        }
    }

    #[test]
    fn listed_questions_need_a_broker_identifier_and_bounded_text() {
        let prompt = |id: &str, target: &str, text: &str| SshPrompt {
            id: id.into(),
            target: target.into(),
            prompt: text.into(),
            confirm: false,
        };
        assert!(prompt(&"a".repeat(64), "demo@host", "Password:").valid());
        assert!(!prompt("short", "demo@host", "Password:").valid());
        assert!(!prompt(&"g".repeat(64), "demo@host", "Password:").valid());
        assert!(!prompt(&"a".repeat(64), &"h".repeat(MAX_TARGET + 1), "Password:").valid());
        assert!(!prompt(&"a".repeat(64), "demo@host", &"p".repeat(MAX_TEXT + 1)).valid());
    }

    async fn next(prompts: &mut RuntimeSshPrompts<'_>) -> Arc<Vec<SshPrompt>> {
        tokio::time::timeout(Duration::from_secs(10), prompts.next_update())
            .await
            .expect("question list")
    }

    async fn next_list(
        updates: &mut tokio::sync::mpsc::Receiver<super::super::Update>,
    ) -> Arc<Vec<SshPrompt>> {
        loop {
            let update = tokio::time::timeout(Duration::from_secs(10), updates.recv())
                .await
                .expect("question update")
                .expect("worker running");
            if let super::super::Update::SshPrompts(list) = update {
                return list;
            }
        }
    }

    // remote-hosts, A password question reaches the native window: the
    // service notifies, the worker reads the question and publishes it, and
    // its answer command reaches the waiting helper.
    #[tokio::test]
    async fn a_question_of_the_local_service_reaches_the_window_and_its_answer_the_helper() {
        use super::super::{Command, CommandKind, Update, Worker};
        let (commands, receiver) = tokio::sync::mpsc::channel(16);
        let (sender, mut updates) = tokio::sync::mpsc::channel(64);
        let worker = Worker::start_isolated(receiver, sender);
        let owner = loop {
            let update = tokio::time::timeout(Duration::from_secs(10), updates.recv())
                .await
                .expect("workspace")
                .expect("worker running");
            if let Update::Workspace(model) = update {
                break model.lifecycle_token();
            }
        };
        let target = format!("notified-{}@client-core", uuid::Uuid::new_v4().simple());
        let answer = splitty_remote::askpass::testing::ask(&target, "demo@host's password:");
        let prompt = loop {
            let list = next_list(&mut updates).await;
            if let Some(prompt) = list.iter().find(|prompt| prompt.target == target) {
                break prompt.clone();
            }
        };
        assert_eq!(prompt.prompt, "demo@host's password:");
        assert!(!prompt.confirm);
        commands
            .send(Command::new(
                owner,
                CommandKind::AnswerSshPrompt {
                    id: prompt.id.clone(),
                    answer: Some("private answer".into()),
                },
            ))
            .await
            .unwrap();
        assert_eq!(
            answer
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .as_deref(),
            Some("private answer")
        );
        loop {
            let list = next_list(&mut updates).await;
            if list.iter().all(|listed| listed.id != prompt.id) {
                break;
            }
        }
        worker.shutdown().unwrap();
    }

    // remote-hosts, Older local service: without the capability the questions
    // are polled, read and answered all the same.
    #[tokio::test]
    async fn a_service_without_notifications_is_polled_and_answered() {
        let service = crate::owned_service::OwnedService::from(crate::EmbeddedService::isolated());
        let mut prompts = RuntimeSshPrompts::new(&service, false);
        let target = format!("polled-{}@client-core", uuid::Uuid::new_v4().simple());
        let answer = splitty_remote::askpass::testing::ask(&target, "demo@host's password:");
        let id = loop {
            let list = next(&mut prompts).await;
            if let Some(prompt) = list.iter().find(|prompt| prompt.target == target) {
                break prompt.id.clone();
            }
        };
        prompts.answer(id.clone(), None).unwrap();
        loop {
            let list = next(&mut prompts).await;
            if list.iter().all(|prompt| prompt.id != id) {
                break;
            }
        }
        assert_eq!(answer.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        prompts.finish().await.unwrap();
        service.close().await.unwrap();
    }
}
