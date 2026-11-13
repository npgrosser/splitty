//! Bounded, process-local SSH questions for the local service's clients, which
//! read and answer them through its API. Only an askpass child holding the
//! random capability can publish questions on the loopback IPC.
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    io::{self, BufRead, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const LIMIT: usize = 16 * 1024;

/// OpenSSH's host-key confirmation does not consistently set
/// SSH_ASKPASS_PROMPT, including the installed Linux client.
pub fn is_confirmation(prompt: &str, hint: Option<&str>) -> bool {
    hint == Some("confirm")
        || (prompt.contains("Are you sure you want to continue connecting")
            && prompt.contains("key fingerprint"))
}
static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
static BROKER: OnceLock<Option<Broker>> = OnceLock::new();
static CHANGES: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();

fn change_feed() -> &'static tokio::sync::watch::Sender<u64> {
    CHANGES.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// Moves whenever a question is published or settled (answered, cancelled,
/// timed out or abandoned by its helper). Values carry no question.
pub fn changes() -> tokio::sync::watch::Receiver<u64> {
    change_feed().subscribe()
}

fn changed() {
    change_feed().send_modify(|generation| *generation = generation.wrapping_add(1));
}

tokio::task_local! {
    static SESSION: Arc<Session>;
}

/// One hub's live host connections share credentials. Weak entries do not
/// retain a password after the last connection or failed attempt ends.
#[derive(Default)]
pub struct Sessions(Mutex<BTreeMap<String, Weak<Session>>>);

impl Sessions {
    pub fn for_target(&self, target: &str) -> Arc<Session> {
        let mut sessions = self.0.lock().unwrap();
        sessions.retain(|_, session| session.strong_count() != 0);
        if let Some(session) = sessions.get(target).and_then(Weak::upgrade) {
            return session;
        }
        let session = Arc::new(Session {
            id: random_id(),
            target: target.to_owned(),
            credentials: Mutex::new(Credentials::default()),
            invalidated: AtomicBool::new(false),
        });
        sessions.insert(target.to_owned(), Arc::downgrade(&session));
        session
    }
}

/// Kept by the connection owner, never by a process-wide credential cache.
pub struct Session {
    id: String,
    target: String,
    credentials: Mutex<Credentials>,
    invalidated: AtomicBool,
}

impl Session {
    /// Invalidate even when OpenSSH exits after only one password attempt.
    /// Never block the async owner on a user answering another prompt.
    pub fn invalidate(&self) {
        self.invalidated.store(true, Ordering::Release);
        if let Ok(mut credentials) = self.credentials.try_lock() {
            credentials.answers.clear();
        }
    }

    /// The existing connector may create a fresh Ssh for each step. Its
    /// commands inherit this locally assigned scope while the future is polled.
    pub async fn scope<F: Future>(self: &Arc<Self>, future: F) -> F::Output {
        SESSION.scope(self.clone(), future).await
    }
}

#[derive(Default)]
struct Credentials {
    answers: BTreeMap<String, Zeroizing<String>>,
    attempted: BTreeMap<(String, [u8; 32]), Instant>,
    cancelled: BTreeMap<String, Instant>,
}

impl Credentials {
    fn is_cancelled(&mut self, invocation: &str) -> bool {
        self.cancelled
            .retain(|_, at| at.elapsed() < Duration::from_secs(120));
        self.cancelled.contains_key(invocation)
    }

    fn cancel(&mut self, invocation: &str) {
        self.answers.clear();
        if self.cancelled.len() >= 128 {
            self.cancelled.pop_first();
        }
        self.cancelled.insert(invocation.to_owned(), Instant::now());
    }

    fn cached(&mut self, invocation: &str, prompt: &str) -> Option<String> {
        let now = Instant::now();
        self.attempted
            .retain(|_, at| now.duration_since(*at) < Duration::from_secs(120));
        let key = (
            invocation.to_owned(),
            Sha256::digest(prompt.as_bytes()).into(),
        );
        // A second question from the same SSH process means its previous
        // answer was rejected. Never automatically replay that answer again.
        if let Some(at) = self.attempted.get_mut(&key) {
            *at = now;
            self.answers.remove(prompt);
            return None;
        }
        if self.attempted.len() >= 128 {
            return None;
        }
        self.attempted.insert(key, now);
        self.answers.get(prompt).map(|answer| answer.to_string())
    }

    fn remember(&mut self, prompt: &str, answer: Option<&str>) {
        match answer {
            Some(answer) if self.answers.len() < 8 || self.answers.contains_key(prompt) => {
                self.answers
                    .insert(prompt.to_owned(), Zeroizing::new(answer.to_owned()));
            }
            None => self.answers.clear(),
            _ => {}
        }
    }
}

/// Only OpenSSH's password/private-key questions are reusable. In particular,
/// fingerprint consent and arbitrary keyboard-interactive/OTP challenges are not.
fn reusable_question(request: &Request) -> bool {
    let prompt = request.prompt.trim();
    !request.confirm
        && prompt.len() <= 1024
        && (prompt.ends_with("'s password:")
            || (prompt.starts_with("Enter passphrase for key '") && prompt.ends_with("':")))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub target: String,
    pub prompt: String,
    pub confirm: bool,
}
struct Pending {
    prompt: Prompt,
    answer: mpsc::Sender<Option<String>>,
}
struct Broker {
    address: String,
    token: String,
    pending: Arc<Mutex<BTreeMap<String, Pending>>>,
    sessions: Arc<Mutex<BTreeMap<String, Weak<Session>>>>,
}
#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    target: String,
    prompt: String,
    confirm: bool,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    invocation: Option<String>,
}

fn random_id() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn enable(program: PathBuf) {
    let _ = PROGRAM.set(program);
}

impl Broker {
    fn respond(&self, id: &str, answer: Option<String>) -> bool {
        let pending = self.pending.lock().unwrap().remove(id);
        if pending.is_some() {
            changed();
        }
        pending.is_some_and(|entry| entry.answer.send(answer).is_ok())
    }

    fn start() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?.to_string();
        let token = random_id();
        let pending = Arc::new(Mutex::new(BTreeMap::new()));
        let sessions = Arc::new(Mutex::new(BTreeMap::new()));
        let scoped = sessions.clone();
        let state = pending.clone();
        let capability = token.clone();
        // Bound both unauthenticated readers and published questions.
        let workers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                use std::sync::atomic::Ordering;
                if workers.fetch_add(1, Ordering::SeqCst) >= 16 {
                    workers.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (state, capability, workers, scoped) = (
                    state.clone(),
                    capability.clone(),
                    workers.clone(),
                    scoped.clone(),
                );
                std::thread::spawn(move || {
                    let _ = serve(stream, &capability, &state, &scoped);
                    workers.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Ok(Self {
            address,
            token,
            pending,
            sessions,
        })
    }
}

fn serve(
    stream: TcpStream,
    token: &str,
    pending: &Mutex<BTreeMap<String, Pending>>,
    sessions: &Mutex<BTreeMap<String, Weak<Session>>>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut line = String::new();
    io::BufReader::new(&stream)
        .take((LIMIT + 1) as u64)
        .read_line(&mut line)?;
    if line.len() > LIMIT {
        return Err(io::Error::other("SSH prompt too large"));
    }
    let request: Request = serde_json::from_str(&line)?;
    if request.token != token || request.target.len() > 4096 {
        return Err(io::Error::other("Invalid SSH prompt capability"));
    }
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(110);
    let session = if reusable_question(&request) {
        request
            .session
            .as_ref()
            .and_then(|id| sessions.lock().unwrap().get(id).and_then(Weak::upgrade))
            .filter(|session| session.target == request.target)
    } else {
        None
    };
    let invocation = request
        .invocation
        .as_deref()
        .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Serialize credential questions within this host connection: concurrent
    // streams wait for the first answer, then use it without opening more UI.
    let mut credentials = None;
    if let (Some(session), Some(invocation)) = (&session, invocation) {
        loop {
            if !waiting(&stream, deadline) {
                return send_answer(stream, None);
            }
            match session.credentials.try_lock() {
                Ok(mut state) => {
                    // OpenSSH can ask again after a failed helper. Cancelling
                    // must dismiss this invocation, not open the same dialog
                    // repeatedly. A deliberate retry has a fresh invocation.
                    if state.is_cancelled(invocation) {
                        return send_answer(stream, None);
                    }
                    if session.invalidated.swap(false, Ordering::AcqRel) {
                        state.answers.clear();
                    }
                    if let Some(answer) = state.cached(invocation, &request.prompt) {
                        return send_answer(stream, Some(answer));
                    }
                    credentials = Some(state);
                    break;
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
    }
    let id = random_id();
    let (send, receive) = mpsc::channel();
    pending.lock().unwrap().insert(
        id.clone(),
        Pending {
            prompt: Prompt {
                id: id.clone(),
                target: request.target,
                prompt: request.prompt.clone(),
                confirm: request.confirm,
            },
            answer: send,
        },
    );
    changed();
    let answer = loop {
        match receive.try_recv() {
            Ok(answer) => break answer,
            Err(mpsc::TryRecvError::Disconnected) => break None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !waiting(&stream, deadline) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if pending.lock().unwrap().remove(&id).is_some() {
        changed();
    }
    if let Some(credentials) = &mut credentials {
        if answer.is_none() {
            if let Some(invocation) = invocation {
                credentials.cancel(invocation);
            }
        }
        if session
            .as_ref()
            .is_some_and(|session| session.invalidated.load(Ordering::Acquire))
        {
            credentials.answers.clear();
        } else {
            credentials.remember(&request.prompt, answer.as_deref());
        }
    }
    send_answer(stream, answer)
}

fn waiting(stream: &TcpStream, deadline: Instant) -> bool {
    if Instant::now() >= deadline {
        return false;
    }
    let mut byte = [0];
    match stream.peek(&mut byte) {
        Ok(0) => false,
        Err(error) => error.kind() == io::ErrorKind::WouldBlock,
        _ => true,
    }
}

fn send_answer(mut stream: TcpStream, answer: Option<String>) -> io::Result<()> {
    let answer = answer.map(Zeroizing::new);
    stream.set_nonblocking(false)?;
    serde_json::to_writer(&mut stream, &answer.as_deref().map(String::as_str))?;
    stream.write_all(b"\n")
}

pub fn configure(command: &mut tokio::process::Command, target: &str) -> Option<PathBuf> {
    let program = PROGRAM.get()?;
    let broker = BROKER.get_or_init(|| Broker::start().ok()).as_ref()?;
    command
        .env_remove("SPLITTY_ASKPASS_SESSION")
        .env_remove("SPLITTY_ASKPASS_INVOCATION");
    let _ = SESSION.try_with(|session| {
        if session.target == target {
            let mut sessions = broker.sessions.lock().unwrap();
            sessions.retain(|_, session| session.strong_count() != 0);
            sessions.insert(session.id.clone(), Arc::downgrade(session));
            command
                .env("SPLITTY_ASKPASS_SESSION", &session.id)
                .env("SPLITTY_ASKPASS_INVOCATION", random_id());
        }
    });
    command
        .env("SPLITTY_BROWSER_ASKPASS", "1")
        .env("SPLITTY_ASKPASS_ADDRESS", &broker.address)
        .env("SPLITTY_ASKPASS_TOKEN", &broker.token)
        .env("SPLITTY_ASKPASS_TARGET", target);
    Some(program.clone())
}

pub fn prompts() -> Vec<Prompt> {
    BROKER
        .get()
        .and_then(Option::as_ref)
        .map_or_else(Vec::new, |broker| {
            broker
                .pending
                .lock()
                .unwrap()
                .values()
                .map(|entry| entry.prompt.clone())
                .collect()
        })
}

pub fn respond(id: &str, answer: Option<String>) -> bool {
    if answer
        .as_ref()
        .is_some_and(|answer| answer.len() > LIMIT || answer.contains(['\n', '\r', '\0']))
    {
        return false;
    }
    BROKER
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|broker| broker.respond(id, answer))
}

pub fn respond_document(body: &serde_json::Value) -> bool {
    let Some(id) = body.get("id").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let answer = match body.get("answer") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Null) => None,
        _ => return false,
    };
    respond(id, answer)
}

/// Run before CLI parsing in the server executable. No logs or arguments ever
/// carry the answer: stdout is the pipe OpenSSH created for this child.
pub fn helper() -> io::Result<()> {
    watch_parent();
    let prompt = std::env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("Missing SSH prompt"))?;
    let confirm = is_confirmation(&prompt, std::env::var("SSH_ASKPASS_PROMPT").ok().as_deref());
    let request = Request {
        token: std::env::var("SPLITTY_ASKPASS_TOKEN").map_err(io::Error::other)?,
        target: std::env::var("SPLITTY_ASKPASS_TARGET").map_err(io::Error::other)?,
        prompt,
        confirm,
        session: std::env::var("SPLITTY_ASKPASS_SESSION").ok(),
        invocation: std::env::var("SPLITTY_ASKPASS_INVOCATION").ok(),
    };
    let address: std::net::SocketAddr = std::env::var("SPLITTY_ASKPASS_ADDRESS")
        .map_err(io::Error::other)?
        .parse()
        .map_err(io::Error::other)?;
    if !address.ip().is_loopback() {
        return Err(io::Error::other("Invalid SSH prompt address"));
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(115)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = serde_json::to_vec(&request)?;
    if bytes.len() >= LIMIT {
        return Err(io::Error::other("SSH prompt too large"));
    }
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    let answer: Option<String> =
        serde_json::from_reader(io::BufReader::new(stream).take((LIMIT + 1) as u64))?;
    let answer = answer.ok_or_else(|| io::Error::other("SSH authentication cancelled"))?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{answer}")?;
    stdout.flush()
}

/// On Unix an askpass child must disappear when its SSH parent is cancelled.
/// Windows helpers retain the independent bounded UI timeout.
pub fn watch_parent() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn getppid() -> i32;
        }
        // getppid has no arguments, allocation, or pointer preconditions.
        let parent = unsafe { getppid() };
        std::thread::spawn(move || loop {
            if parent <= 1 || unsafe { getppid() } != parent {
                std::process::exit(1);
            }
            std::thread::sleep(Duration::from_millis(250));
        });
    }
}

/// An askpass helper's side of the broker for tests outside this crate.
#[cfg(feature = "testing")]
pub mod testing {
    use super::*;

    /// Publish `prompt` for `target` as OpenSSH's askpass child would, and
    /// return the answer once a client settles it (`None`: cancelled).
    pub fn ask(target: &str, prompt: &str) -> mpsc::Receiver<Option<String>> {
        // As a service runtime would; the program itself is never started.
        enable(std::env::current_exe().expect("test executable"));
        let mut command = tokio::process::Command::new("ssh");
        configure(&mut command, target).expect("the question broker runs");
        let variable = |name: &str| {
            command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| value)
                .and_then(|value| value.to_str())
                .expect("broker variable")
                .to_owned()
        };
        let request = Request {
            token: variable("SPLITTY_ASKPASS_TOKEN"),
            target: target.to_owned(),
            prompt: prompt.to_owned(),
            confirm: false,
            session: None,
            invocation: None,
        };
        let mut stream = TcpStream::connect(variable("SPLITTY_ASKPASS_ADDRESS")).unwrap();
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let answer: Option<String> =
                serde_json::from_reader(io::BufReader::new(stream)).unwrap_or(None);
            let _ = send.send(answer);
        });
        receive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_openssh_host_key_questions_work_without_an_askpass_hint() {
        let prompt = "The authenticity of host '[localhost]:2222' can't be established.\nED25519 key fingerprint is SHA256:example.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert!(is_confirmation(prompt, None));
        assert!(is_confirmation(prompt, Some("echo")));
        assert!(is_confirmation(
            &prompt.replace("fingerprint is ", "fingerprint is: "),
            None
        ));
        assert!(is_confirmation("Allow this SSH request?", Some("confirm")));
        assert!(!is_confirmation("demo@localhost's password:", None));
        assert!(!is_confirmation("Enter passphrase for key:", None));
    }

    fn connect(broker: &Broker, token: String) -> TcpStream {
        let request = Request {
            token,
            target: "user@host:2222".into(),
            prompt: "SHA256:actual-key".into(),
            confirm: true,
            session: None,
            invocation: None,
        };
        connect_request(broker, request)
    }

    fn connect_request(broker: &Broker, request: Request) -> TcpStream {
        let mut stream = TcpStream::connect(&broker.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        stream
    }

    fn wait_for(broker: &Broker, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while broker.pending.lock().unwrap().len() != count {
            assert!(
                std::time::Instant::now() < deadline,
                "prompt lifecycle did not settle"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn authenticated_questions_return_one_answer_and_disappear() {
        let broker = Broker::start().unwrap();
        for answer in [
            Some("yes".to_owned()),
            None,
            Some("private password".to_owned()),
        ] {
            let stream = connect(&broker, broker.token.clone());
            wait_for(&broker, 1);
            let entry = broker.pending.lock().unwrap().pop_first().unwrap().1;
            assert_eq!(entry.prompt.target, "user@host:2222");
            assert_eq!(entry.prompt.prompt, "SHA256:actual-key");
            assert!(entry.prompt.confirm);
            entry.answer.send(answer.clone()).unwrap();
            let received: Option<String> = serde_json::from_reader(stream).unwrap();
            assert_eq!(received, answer);
            wait_for(&broker, 0);
        }
    }

    fn wait_changed(changes: &mut tokio::sync::watch::Receiver<u64>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !changes.has_changed().unwrap() {
            assert!(std::time::Instant::now() < deadline, "no change notified");
            std::thread::sleep(Duration::from_millis(10));
        }
        changes.borrow_and_update();
    }

    #[test]
    fn publishing_and_settling_a_question_move_the_change_feed() {
        let broker = Broker::start().unwrap();
        let mut changes = changes();
        changes.borrow_and_update();
        // Answered: the published question and its settlement each notify.
        let stream = connect(&broker, broker.token.clone());
        wait_changed(&mut changes);
        let id = broker
            .pending
            .lock()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        assert!(broker.respond(&id, Some("yes".into())));
        wait_changed(&mut changes);
        let answer: Option<String> = serde_json::from_reader(stream).unwrap();
        assert_eq!(answer.as_deref(), Some("yes"));
        // A helper that goes away settles its question too.
        let stream = connect(&broker, broker.token.clone());
        wait_for(&broker, 1);
        wait_changed(&mut changes);
        drop(stream);
        wait_for(&broker, 0);
        wait_changed(&mut changes);
    }

    #[test]
    fn wrong_capabilities_publish_nothing_and_disconnected_helpers_are_removed() {
        let broker = Broker::start().unwrap();
        let mut rejected = connect(&broker, "wrong capability".into());
        let mut output = Vec::new();
        rejected.read_to_end(&mut output).unwrap();
        assert!(output.is_empty());
        assert!(broker.pending.lock().unwrap().is_empty());
        let accepted = connect(&broker, broker.token.clone());
        wait_for(&broker, 1);
        drop(accepted);
        wait_for(&broker, 0);
    }

    fn password_request(broker: &Broker, session: &Arc<Session>, invocation: &str) -> Request {
        broker
            .sessions
            .lock()
            .unwrap()
            .insert(session.id.clone(), Arc::downgrade(session));
        Request {
            token: broker.token.clone(),
            target: session.target.clone(),
            prompt: "demo@localhost's password: ".into(),
            confirm: false,
            session: Some(session.id.clone()),
            invocation: Some(invocation.to_owned()),
        }
    }

    fn reply(broker: &Broker, answer: Option<&str>) {
        wait_for(broker, 1);
        let pending = broker.pending.lock().unwrap().pop_first().unwrap().1;
        pending.answer.send(answer.map(str::to_owned)).unwrap();
    }

    fn received(stream: TcpStream) -> Option<String> {
        serde_json::from_reader(stream).unwrap()
    }

    #[test]
    fn one_password_serves_later_steps_and_rejected_answers_prompt_again() {
        let broker = Broker::start().unwrap();
        let sessions = Sessions::default();
        let session = sessions.for_target("demo@localhost:2223");
        let first = connect_request(
            &broker,
            password_request(&broker, &session, &"a".repeat(64)),
        );
        reply(&broker, Some("first answer"));
        assert_eq!(received(first).as_deref(), Some("first answer"));

        let second = connect_request(
            &broker,
            password_request(&broker, &session, &"b".repeat(64)),
        );
        assert_eq!(received(second).as_deref(), Some("first answer"));
        assert!(broker.pending.lock().unwrap().is_empty());

        // OpenSSH asking again within the same invocation means rejection.
        let retry = connect_request(
            &broker,
            password_request(&broker, &session, &"b".repeat(64)),
        );
        reply(&broker, Some("corrected answer"));
        assert_eq!(received(retry).as_deref(), Some("corrected answer"));
        let next = connect_request(
            &broker,
            password_request(&broker, &session, &"c".repeat(64)),
        );
        assert_eq!(received(next).as_deref(), Some("corrected answer"));
        session.invalidate();
        let next = connect_request(
            &broker,
            password_request(&broker, &session, &"d".repeat(64)),
        );
        reply(&broker, None);
        assert_eq!(received(next), None);
    }

    #[test]
    fn simultaneous_connections_share_one_question_and_cancel_discards_the_answer() {
        let broker = Broker::start().unwrap();
        let session = Sessions::default().for_target("demo@localhost:2223");
        let first = connect_request(
            &broker,
            password_request(&broker, &session, &"a".repeat(64)),
        );
        wait_for(&broker, 1);
        let second = connect_request(
            &broker,
            password_request(&broker, &session, &"b".repeat(64)),
        );
        reply(&broker, Some("shared answer"));
        assert_eq!(received(first).as_deref(), Some("shared answer"));
        assert_eq!(received(second).as_deref(), Some("shared answer"));
        let retry = connect_request(
            &broker,
            password_request(&broker, &session, &"b".repeat(64)),
        );
        reply(&broker, None);
        assert_eq!(received(retry), None);
        let repeated = connect_request(
            &broker,
            password_request(&broker, &session, &"b".repeat(64)),
        );
        assert_eq!(received(repeated), None);
        assert!(broker.pending.lock().unwrap().is_empty());
        let next = connect_request(
            &broker,
            password_request(&broker, &session, &"c".repeat(64)),
        );
        reply(&broker, Some("new answer"));
        assert_eq!(received(next).as_deref(), Some("new answer"));
    }

    #[test]
    fn credentials_are_separate_per_hub_target_and_connection_lifetime() {
        let broker = Broker::start().unwrap();
        let sessions = Sessions::default();
        let session = sessions.for_target("demo@localhost:2223");
        assert!(Arc::ptr_eq(
            &session,
            &sessions.for_target("demo@localhost:2223")
        ));
        let first = connect_request(&broker, password_request(&broker, &session, &random_id()));
        reply(&broker, Some("private answer"));
        assert_eq!(received(first).as_deref(), Some("private answer"));
        for separate in [
            sessions.for_target("demo@localhost:2222"),
            Sessions::default().for_target("demo@localhost:2223"),
        ] {
            let stream =
                connect_request(&broker, password_request(&broker, &separate, &random_id()));
            reply(&broker, None);
            assert_eq!(received(stream), None);
        }
        // Neither the hub directory nor the broker registry holds passwords alive.
        let weak = Arc::downgrade(&session);
        let old_id = session.id.clone();
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(3);
        while weak.strong_count() != 0 {
            assert!(
                Instant::now() < deadline,
                "credentials survived connection cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let fresh = sessions.for_target("demo@localhost:2223");
        assert_ne!(fresh.id, old_id);
        let stream = connect_request(&broker, password_request(&broker, &fresh, &random_id()));
        reply(&broker, None);
        assert_eq!(received(stream), None);
    }

    #[test]
    fn fingerprint_consent_and_one_time_challenges_are_not_cached() {
        let broker = Broker::start().unwrap();
        let session = Sessions::default().for_target("demo@localhost:2223");
        let mut request = password_request(&broker, &session, &random_id());
        assert!(reusable_question(&request));
        request.prompt = "Enter passphrase for key '/home/demo/.ssh/id_ed25519': ".into();
        assert!(reusable_question(&request));
        for prompt in [
            "Verification code: ",
            "Password: ",
            "Confirm SSH fingerprint?",
        ] {
            request.prompt = prompt.into();
            assert!(!reusable_question(&request));
        }
        request.prompt = "demo@localhost's password:".into();
        request.confirm = true;
        assert!(!reusable_question(&request));
    }
}
