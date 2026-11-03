//! Reaching a host with the system `ssh`: the validated target, the hardened
//! command, quoting of values for the host's shell, and what a failure means.
//!
//! # Why the target is parsed rather than passed through
//!
//! `ssh` takes its destination positionally, and a value starting with `-` is
//! read as an *option* instead (security review SR-13). Since
//! options include `-oProxyCommand=…`, `-F` and `-J`, a target taken from an
//! untrusted place could make OpenSSH run a local program or load a foreign
//! configuration. Splitty already avoids a shell here, which stops metacharacter
//! injection, but that does nothing about argument confusion. The fix is to
//! model the destination instead of treating it as an opaque string.
//!
//! # Why every value is quoted
//!
//! `ssh` joins everything after the destination into one string that the host's
//! login shell parses again, unlike `wsl.exe --exec`, which passes arguments one
//! by one. A [`CommandLine`] is therefore built from constant words and values
//! that go through [`quote`], and it is sent as exactly one argument after `--`.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};
use std::time::Duration;

use splitty_transport::{
    admission::Admission,
    budget::{Charged, ConnectionBudget, Lease},
    queue,
    worker::Worker,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStderr, Command};
use tokio::task::JoinHandle;

/// Setup output owns its byte reservation beyond process exit, until callers
/// finish parsing or inspecting both captured streams.
pub type CapturedOutput = Charged<Output>;

/// Shared native target ceiling, applied before trimming, copying or diagnostics.
pub const MAX_TARGET_BYTES: usize = 4096;

/// A validated SSH destination.
///
/// Construction is the validation: holding one of these means the value is safe
/// to pass to `ssh` in the destination position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetError {
    #[error("SSH target exceeds 4096 bytes")]
    TooLong,

    #[error("SSH target must not be empty")]
    Empty,

    #[error("SSH target {0:?} starts with a dash, which ssh would read as an option")]
    LooksLikeOption(String),

    #[error("SSH target {0:?} contains whitespace or control characters")]
    IllegalCharacter(String),

    #[error("SSH target {0:?} is not a valid [user@]host[:port]")]
    Malformed(String),
}

impl SshTarget {
    pub fn parse(raw: &str) -> Result<Self, TargetError> {
        if raw.len() > MAX_TARGET_BYTES {
            return Err(TargetError::TooLong);
        }
        let value = raw.trim_matches(|c: char| c == ' ' || c == '\t');

        if value.is_empty() {
            return Err(TargetError::Empty);
        }

        // Any leading dash, including the Unicode lookalikes that a value pasted
        // from a document may carry.
        if value.starts_with([
            '-', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2212}',
        ]) {
            return Err(TargetError::LooksLikeOption(value.to_string()));
        }

        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(TargetError::IllegalCharacter(value.to_string()));
        }

        let (user, host) = match value.rsplit_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, value),
        };

        if let Some(user) = user {
            if user.is_empty() || !user.chars().all(is_user_char) {
                return Err(TargetError::Malformed(value.to_string()));
            }
        }

        if !is_valid_host(host) {
            return Err(TargetError::Malformed(value.to_string()));
        }

        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_user_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// Accepts hostnames, IPv4 literals and bracketed IPv6 literals.
///
/// Bracketed form only for IPv6: a bare `::1` is ambiguous with the `host:port`
/// separator, and accepting both spellings invites a parser disagreement between
/// this check and OpenSSH.
fn is_valid_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }

    if let Some(rest) = host.strip_prefix('[') {
        let Some((addr, tail)) = rest.split_once(']') else {
            return false;
        };
        if addr.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        return tail.is_empty() || is_port_suffix(tail);
    }

    let (name, port) = match host.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };

    if let Some(port) = port {
        if port.parse::<u16>().map(|p| p == 0).unwrap_or(true) {
            return false;
        }
    }

    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

fn is_port_suffix(tail: &str) -> bool {
    tail.strip_prefix(':')
        .and_then(|p| p.parse::<u16>().ok())
        .map(|p| p != 0)
        .unwrap_or(false)
}

/// Options applied to every `ssh` invocation.
///
/// `ExitOnForwardFailure` is the important one: without it OpenSSH keeps running
/// after a forward fails to bind, so the caller sees a live process and concludes
/// the tunnel is up. `BatchMode` stops ssh blocking on an interactive prompt in
/// a context where nobody is watching for one; an unknown host key therefore
/// fails instead of being accepted (see [`classify`]).
pub fn hardening_options() -> Vec<String> {
    [
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Options for a connection that carries only a command and its standard
/// input and output, whatever the user's configuration says for the host: no
/// terminal (`RequestTTY` would mangle binary data), no agent or X11
/// forwarding, and none of the forwards the configuration names. A tunnel
/// cannot use them: `ClearAllForwardings` clears its own `-L` as well.
pub fn stdio_only_options() -> Vec<String> {
    [
        "-T",
        "-a",
        "-x",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "ControlPersist=no",
        "-o",
        "ForkAfterAuthentication=no",
        "-o",
        "StdinNull=no",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Options for a connection that carries only the port forwards named on its
/// command line, whatever the user's configuration says for the host: no
/// command (`-N`), no terminal, no agent or X11 forwarding. The forwards the
/// configuration names cannot be switched off here, since
/// `ClearAllForwardings` clears the command line's as well; a caller checks
/// them with [`Ssh::configured_forwards`] instead.
pub fn forward_only_options() -> Vec<String> {
    ["-N", "-T", "-a", "-x"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Options for a connection that carries the one remote forward named on
/// its command line and the command that serves it on the host, with its
/// standard input and output: no terminal, no agent or X11 forwarding. As
/// for [`forward_only_options`], the configuration's forwards cannot be
/// switched off without clearing the command line's; a caller checks them
/// with [`Ssh::configured_forwards`].
pub fn relay_only_options() -> Vec<String> {
    ["-T", "-a", "-x"].iter().map(|s| s.to_string()).collect()
}

/// The keywords of `ssh -G` that name a forward.
const FORWARD_KEYWORDS: [&str; 3] = ["localforward", "remoteforward", "dynamicforward"];

/// The forwards in the output of `ssh -G`, one line each, such as
/// `remoteforward 9999 [localhost]:9999`.
fn forwards_in_config(config: &str) -> Vec<String> {
    config
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.split_whitespace().next().is_some_and(|keyword| {
                FORWARD_KEYWORDS
                    .iter()
                    .any(|forward| keyword.eq_ignore_ascii_case(forward))
            })
        })
        .map(str::to_owned)
        .collect()
}

/// `value` as one word for a POSIX shell: in single quotes, with each `'`
/// written as `'\''`. fish reads the same form as concatenated strings.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A version as it may appear in a path on the host.
pub fn is_version(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// A SHA-256 digest in lowercase hex.
pub fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One command line for the host's login shell: constant words, and values
/// that are always quoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine(String);

impl CommandLine {
    /// A line that starts with a constant program name.
    pub fn new(program: &'static str) -> Self {
        Self(program.to_owned())
    }

    /// A line that starts with a program given as a value, such as a path the
    /// host reported.
    pub fn program(path: &str) -> Self {
        Self(quote(path))
    }

    /// Append a constant word, such as a subcommand or a flag.
    pub fn word(mut self, word: &'static str) -> Self {
        self.0.push(' ');
        self.0.push_str(word);
        self
    }

    /// Append a value, quoted.
    pub fn value(mut self, value: &str) -> Self {
        self.0.push(' ');
        self.0.push_str(&quote(value));
        self
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Runs command lines on a host.
pub trait Executor: Sync {
    /// The host as the user named it, for messages.
    fn host(&self) -> &str;

    /// Setup and derived diagnostics share the connector owner's byte budget.
    fn budget(&self) -> ConnectionBudget {
        ConnectionBudget::default()
    }

    /// Run `line`, feeding `input` to its standard input when given.
    fn run(
        &self,
        line: &CommandLine,
        input: Option<Vec<u8>>,
    ) -> impl Future<Output = io::Result<CapturedOutput>> + Send;
}

/// The `ssh` client this machine runs: on Windows the system's OpenSSH
/// (`%SystemRoot%\System32\OpenSSH\ssh.exe`) when it is installed, otherwise
/// `ssh` from `PATH`.
pub fn ssh_program() -> PathBuf {
    #[cfg(windows)]
    {
        windows_ssh(std::env::var_os("SystemRoot").as_deref(), Path::is_file)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("ssh")
    }
}

/// The order of [`ssh_program`] on Windows, with the system root and the file
/// check passed in. A GUI app's `PATH` may lack the OpenSSH directory, or put
/// another `ssh` (Git's, an MSYS one) first, which knows neither the Windows
/// agent nor the user's Windows configuration.
#[cfg_attr(
    not(any(windows, test)),
    expect(dead_code, reason = "the resolution applies only on Windows")
)]
fn windows_ssh(system_root: Option<&std::ffi::OsStr>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    if let Some(root) = system_root.filter(|root| !root.is_empty()) {
        let system = Path::new(root)
            .join("System32")
            .join("OpenSSH")
            .join("ssh.exe");
        if exists(&system) {
            return system;
        }
    }
    PathBuf::from("ssh")
}

/// `CREATE_NO_WINDOW`: a console program started by a GUI app would otherwise
/// open a console window of its own.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The host, reached with the system `ssh` and the user's configuration.
pub struct Ssh {
    target: SshTarget,
    program: PathBuf,
    /// Options after the hardening ones.
    options: Vec<String>,
    /// Native owner cancellation and byte accounting across every setup step.
    context: Option<queue::Context>,
}

impl Ssh {
    #[cfg(test)]
    pub(crate) fn with_test_program(mut self, program: PathBuf) -> Self {
        self.program = program;
        self
    }
    pub fn new(target: SshTarget) -> Self {
        Self {
            target,
            program: ssh_program(),
            options: Vec::new(),
            context: None,
        }
    }

    /// The native owner supplies this before starting preparation. It never
    /// comes from a remote host or renderer-selected address. This context
    /// belongs to one connection attempt; a retry supplies a fresh context.
    pub fn with_context(mut self, context: queue::Context) -> Self {
        self.context = Some(context);
        self
    }
    pub(crate) fn context(&self) -> queue::Context {
        self.context.clone().unwrap_or_default()
    }
    pub(crate) fn owner_context(&self) -> Option<queue::Context> {
        self.context.clone()
    }

    /// The host, for commands and their standard input and output only
    /// ([`stdio_only_options`]).
    pub fn stdio_only(target: SshTarget) -> Self {
        Self {
            options: stdio_only_options(),
            ..Self::new(target)
        }
    }

    /// Preserve the selected executable/target while assigning the owned
    /// command-only policy; a connector cannot inherit forwarding options.
    pub fn into_stdio_only(mut self) -> Self {
        self.options = stdio_only_options();
        self
    }

    /// The host, for port forwards named on the command line only
    /// ([`forward_only_options`]).
    pub fn forward_only(target: SshTarget) -> Self {
        Self {
            options: forward_only_options(),
            ..Self::new(target)
        }
    }

    /// The host, for one remote forward named on the command line and the
    /// command that serves it ([`relay_only_options`]).
    pub fn relay_only(target: SshTarget) -> Self {
        Self {
            options: relay_only_options(),
            ..Self::new(target)
        }
    }

    /// The forwards the user's configuration adds for the host
    /// (`LocalForward`, `RemoteForward`, `DynamicForward`), as `ssh -G`
    /// resolves it without connecting: one line each, such as
    /// `remoteforward 9999 [localhost]:9999`.
    pub async fn configured_forwards(&self) -> Result<Vec<String>, SshError> {
        let mut command = self.command();
        command.arg("-G").arg(self.target.as_str());
        let context = self.context();
        let output = run_context(&mut command, None, &Admission::global(), &context)
            .await
            .map_err(SshError::Spawn)?;
        if !output.status.success() {
            let _decoded = context
                .budget
                .reserve(3 * output.stderr.len(), false)
                .map_err(|error| {
                    SshError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error))
                })?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(classify_budgeted(
                self.target.as_str(),
                output.status,
                &stderr,
                &context.budget,
            ));
        }
        Ok(forwards_in_config(&String::from_utf8_lossy(&output.stdout)))
    }

    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    /// `ssh` with the hardening options, killed when dropped, and on Windows
    /// without a console window.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(hardening_options())
            .args(&self.options)
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
}

impl Executor for Ssh {
    fn host(&self) -> &str {
        self.target.as_str()
    }

    fn budget(&self) -> ConnectionBudget {
        self.context().budget
    }

    async fn run(&self, line: &CommandLine, input: Option<Vec<u8>>) -> io::Result<CapturedOutput> {
        let mut command = self.command();
        command
            .arg(self.target.as_str())
            .arg("--")
            .arg(line.as_str());
        run_context(&mut command, input, &Admission::global(), &self.context()).await
    }
}

/// Run `command`, writing `input` to its standard input while its output is
/// collected, so neither side can block the other.
pub async fn run_with_input(
    mut command: Command,
    input: Option<Vec<u8>>,
) -> io::Result<CapturedOutput> {
    run_admitted(&mut command, input, &Admission::global()).await
}

const SETUP_STDOUT: usize = 1024 * 1024;
const SETUP_TIMEOUT: Duration = Duration::from_secs(120);

async fn run_admitted(
    command: &mut Command,
    input: Option<Vec<u8>>,
    admission: &Admission,
) -> io::Result<CapturedOutput> {
    run_budgeted(command, input, admission, &ConnectionBudget::default()).await
}

async fn run_budgeted(
    command: &mut Command,
    input: Option<Vec<u8>>,
    admission: &Admission,
    budget: &ConnectionBudget,
) -> io::Result<CapturedOutput> {
    run_context(
        command,
        input,
        admission,
        &queue::Context::new(budget.clone()),
    )
    .await
}

async fn run_context(
    command: &mut Command,
    input: Option<Vec<u8>>,
    admission: &Admission,
    context: &queue::Context,
) -> io::Result<CapturedOutput> {
    if context.is_closed() {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "SSH owner closed",
        ));
    }
    let budget = &context.budget;
    // Byte admission precedes both process creation and output allocation.
    // Include stderr's read scratch until collection finishes; the retained
    // output then keeps only its actual Vec capacities and struct storage.
    let storage = budget
        .reserve(
            SETUP_STDOUT + 1 + STDERR_KEPT + 4096 + std::mem::size_of::<Output>(),
            false,
        )
        .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Worker::spawn(command, admission)?;
    let stdin = child.stdin();
    let stdout = child
        .stdout()
        .ok_or_else(|| io::Error::other("missing SSH stdout pipe"))?;
    let stderr = child
        .stderr()
        .ok_or_else(|| io::Error::other("missing SSH stderr pipe"))?;
    let write = async move {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
        }
        Ok::<(), io::Error>(())
    };
    let collect = async {
        tokio::try_join!(
            async { Ok::<_, io::Error>(write.await) },
            setup_output(stdout),
            diagnostic_tail(stderr),
            child.wait()
        )
    };
    let mut closing = context.closing();
    let result = tokio::select! { biased;
        _ = closing.wait_for(|closed| *closed) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "SSH owner closed")),
        result = tokio::time::timeout(SETUP_TIMEOUT, collect) => result
            .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "SSH setup exceeded its deadline"))),
    };
    let (written, stdout, stderr, status) = match result {
        Ok(output) => output,
        Err(error) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
            // If exit cannot yet be observed, Worker's reaper retains the slot.
            return Err(error);
        }
    };
    let retained = stdout.capacity() + stderr.capacity() + std::mem::size_of::<Output>();
    let output = storage.shrink(retained).charge(Output {
        status,
        stdout,
        stderr,
    });
    // A command that stops reading early fails on its own; its status and
    // error output say more than the broken pipe.
    if let Err(error) = written {
        if error.kind() != io::ErrorKind::BrokenPipe || output.status.success() {
            return Err(error);
        }
    }
    Ok(output)
}

async fn setup_output(reader: impl tokio::io::AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    // Fixed maximum allocation, including the byte which proves overflow.
    let mut output = Vec::with_capacity(SETUP_STDOUT + 1);
    reader
        .take(u64::try_from(SETUP_STDOUT + 1).map_err(io::Error::other)?)
        .read_to_end(&mut output)
        .await?;
    if output.len() > SETUP_STDOUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH setup output exceeds 1 MiB",
        ));
    }
    Ok(output)
}

async fn diagnostic_tail(mut reader: impl tokio::io::AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut kept = Vec::with_capacity(STDERR_KEPT);
    let mut buffer = [0u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(kept);
        }
        let excess = (kept.len() + read).saturating_sub(STDERR_KEPT);
        kept.drain(..excess);
        kept.extend_from_slice(&buffer[..read]);
    }
}

/// How much of a long-running `ssh`'s error output is kept, from its end.
pub(crate) const STDERR_KEPT: usize = 8 * 1024;

/// A retained SSH diagnostic keeps its capacity reservation after the reader
/// exits, until the consumer drops it or converts it into another error type.
pub(crate) type Diagnostic = Charged<String>;

/// Reserve before spawning the child/drain. Covers the tail, read scratch,
/// lossy UTF-8 growth (old and new capacity) and the trimmed returned string.
pub(crate) fn diagnostic_budget(budget: &ConnectionBudget) -> io::Result<Lease> {
    budget
        .reserve(10 * STDERR_KEPT + 4096, false)
        .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))
}

/// Drain a long-running child's stderr without an unbounded tail or waiter.
pub(crate) async fn keep_end(stderr: ChildStderr, storage: Lease) -> Diagnostic {
    let kept = diagnostic_tail(stderr).await.unwrap_or_default();
    let decoded = String::from_utf8_lossy(&kept);
    let message = decoded.trim().to_owned();
    drop(decoded);
    drop(kept);
    storage
        .shrink(message.capacity() + std::mem::size_of::<String>())
        .charge(message)
}

/// How long the error output may stay open after the process ended: a
/// `ProxyCommand` that `ssh` started can hold it a moment longer.
const STDERR_WAIT: Duration = Duration::from_secs(2);

/// What the [`keep_end`] task of a process that ended kept, if it ends in
/// time.
pub(crate) fn kept_end(task: Option<JoinHandle<Diagnostic>>) -> impl Future<Output = Diagnostic> {
    struct Drain(JoinHandle<Diagnostic>);
    impl Drop for Drain {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    // Wrap before constructing the future, so cancellation before its first
    // poll also owns the task. A bare JoinHandle would detach on drop.
    let task = task.map(Drain);
    async move {
        match task {
            Some(mut task) => match tokio::time::timeout(STDERR_WAIT, &mut task.0).await {
                Ok(result) => result.unwrap_or_default(),
                Err(_) => {
                    task.0.abort();
                    let _ = (&mut task.0).await;
                    Diagnostic::default()
                }
            },
            None => Diagnostic::default(),
        }
    }
}

/// Why a command on the host failed.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("ssh could not be started: {0}")]
    Spawn(io::Error),

    /// The host's shell did not find the program (status 127).
    #[error("the command was not found on {host}")]
    NotFound { host: String },

    #[error("{message}")]
    Failed {
        message: String,
        status: Option<i32>,
        stderr: String,
        /// What the user can do about it, when `ssh` said why it failed.
        hint: Option<String>,
        /// Reservation for owned diagnostic copies, retained with this error.
        allocation: Option<Box<Lease>>,
    },
}

impl SshError {
    /// What the user can do about the failure, if anything is known.
    pub fn hint(&self) -> Option<&str> {
        match self {
            SshError::Failed { hint, .. } => hint.as_deref(),
            SshError::Spawn(_) | SshError::NotFound { .. } => None,
        }
    }
}

/// The message `ssh` printed when it refused an unknown or changed host key.
pub fn host_key_failed(stderr: &str) -> bool {
    stderr.contains("Host key verification failed")
        || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
}

/// The message `ssh` printed when the host accepted none of its credentials.
pub fn authentication_failed(stderr: &str) -> bool {
    stderr.contains("Permission denied (") || stderr.contains("Too many authentication failures")
}

/// Turn a failed command into the error the user can act on.
///
/// 127 is what a POSIX shell (and fish) returns for a command it cannot find;
/// ssh passes the remote command's status through and uses 255 for its own
/// failures.
pub fn classify(host: &str, status: ExitStatus, stderr: &str) -> SshError {
    classify_budgeted(host, status, stderr, &ConnectionBudget::default())
}

/// Reserve before converting a captured diagnostic into retained error strings.
/// The original capture remains independently charged until its owner drops it.
pub fn classify_budgeted(
    host: &str,
    status: ExitStatus,
    stderr: &str,
    budget: &ConnectionBudget,
) -> SshError {
    let stderr = stderr.trim();
    let bytes = host
        .len()
        .checked_add(stderr.len())
        .and_then(|bytes| bytes.checked_mul(8))
        .and_then(|bytes| bytes.checked_add(4096));
    let storage = match bytes.and_then(|bytes| budget.reserve(bytes, false).ok()) {
        Some(storage) => storage,
        None => {
            return SshError::Spawn(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSH diagnostic capacity exhausted",
            ))
        }
    };
    // NotFound only copies the caller-owned host. Failed keeps its converted
    // message, stderr and optional hint after the original capture is gone.
    if status.code() == Some(127) {
        return SshError::NotFound {
            host: host.to_owned(),
        };
    }
    let message = if stderr.is_empty() {
        format!("ssh to {host} failed ({status})")
    } else {
        format!("ssh to {host} failed: {stderr}")
    };
    let hint = host_key_failed(stderr).then(|| {
        format!(
            "The host key of {host} is unknown or has changed. Run `ssh {host}` once in a \
             terminal and check the fingerprint before you connect again."
        )
    });
    let stderr = stderr.to_owned();
    let retained = message.capacity()
        + stderr.capacity()
        + hint.as_ref().map_or(0, String::capacity)
        + std::mem::size_of::<SshError>()
        + std::mem::size_of::<Lease>();
    SshError::Failed {
        message,
        status: status.code(),
        stderr,
        hint,
        allocation: Some(Box::new(storage.shrink(retained))),
    }
}

/// Run `line` and return its output when it succeeded.
pub async fn run_checked<E: Executor>(
    executor: &E,
    line: &CommandLine,
    input: Option<Vec<u8>>,
) -> Result<CapturedOutput, SshError> {
    let output = executor.run(line, input).await.map_err(SshError::Spawn)?;
    if output.status.success() {
        Ok(output)
    } else {
        let budget = executor.budget();
        let _decoded = budget
            .reserve(3 * output.stderr.len(), false)
            .map_err(|error| SshError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error)))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(classify_budgeted(
            executor.host(),
            output.status,
            &stderr,
            &budget,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_a_stderr_wait_aborts_the_drain_even_before_its_first_poll() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        for poll in [false, true] {
            let dropped = Arc::new(AtomicBool::new(false));
            let marker = dropped.clone();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let drain = tokio::spawn(async move {
                let _guard = Dropped(marker);
                entered.send(()).unwrap();
                std::future::pending::<Diagnostic>().await
            });
            ready.await.unwrap();
            if poll {
                let waiting = tokio::spawn(kept_end(Some(drain)));
                tokio::task::yield_now().await;
                waiting.abort();
                let _ = waiting.await;
            } else {
                drop(kept_end(Some(drain)));
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while !dropped.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancelled stderr reader is dropped");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_capture_is_bounded_and_failed_commands_release_only_reaped_workers() {
        let admission = Admission::default();
        let process = splitty_transport::budget::ProcessBudget::new(4 * SETUP_STDOUT, 0);
        let budget = ConnectionBudget::new(process.clone(), 4 * SETUP_STDOUT, 0);
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 1048577 /dev/zero; exec sleep 30"]);
        let result = run_budgeted(&mut command, None, &admission, &budget).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(admission.counts().workers, 0);
        assert_eq!(budget.used(), 0);

        let mut command = Command::new("sh");
        command.args([
            "-c",
            "head -c 32768 /dev/zero >&2; printf reason >&2; printf ready; exit 7",
        ]);
        let output = run_budgeted(&mut command, None, &admission, &budget)
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"ready");
        assert_eq!(output.stderr.len(), STDERR_KEPT);
        assert!(output.stderr.ends_with(b"reason"));
        assert_eq!(admission.counts().workers, 0);
        assert_eq!(
            budget.used(),
            output.stdout.capacity() + output.stderr.capacity() + std::mem::size_of::<Output>()
        );
        assert_eq!(
            process.used(),
            budget.used(),
            "output remains counted after worker exit"
        );
        drop(output);
        assert_eq!(process.used(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn saturated_setup_refuses_before_spawn_and_cancelled_setup_is_reaped() {
        let admission = std::sync::Arc::new(Admission::default());
        let mut held: Vec<_> = (0..32)
            .map(|_| {
                let mut command = Command::new("sleep");
                command.arg("30");
                Worker::spawn(&mut command, &admission).unwrap()
            })
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let mut command = Command::new("touch");
        command.arg(&marker);
        assert_eq!(
            run_admitted(&mut command, None, &admission)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!marker.exists());
        for worker in &mut held {
            worker.start_kill().unwrap();
        }
        for worker in &mut held {
            worker.wait().await.unwrap();
        }
        drop(held);
        let workers = admission.clone();
        let path = marker.clone();
        let task = tokio::spawn(async move {
            let mut command = Command::new("sh");
            command
                .args(["-c", "printf started > \"$1\"; exec sleep 30", "sh"])
                .arg(path);
            run_admitted(&mut command, None, &workers).await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(admission.counts().workers, 1);
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while admission.counts().workers != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled worker is observed before its slot returns");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owner_cancellation_reaps_setup_before_returning_and_prevents_later_spawns() {
        let context = queue::Context::default();
        let admission = Admission::default();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pid");
        let mut command = Command::new("sh");
        command
            .args(["-c", "echo $$ > \"$1\"; exec sleep 30", "sh"])
            .arg(&marker);
        let cancel = async {
            let pid = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(text) = tokio::fs::read_to_string(&marker).await {
                        if let Ok(pid) = text.trim().parse::<u32>() {
                            break pid;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(admission.counts().workers, 1);
            assert!(context.budget.used() > SETUP_STDOUT);
            context.close();
            pid
        };
        let (result, pid) = tokio::join!(
            run_context(&mut command, None, &admission, &context),
            cancel
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(admission.counts().workers, 0);
        assert_eq!(context.budget.used(), 0);
        assert!(!Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap()
            .success());
        let not_started = dir.path().join("unexpected");
        let mut command = Command::new("touch");
        command.arg(&not_started);
        assert_eq!(
            run_context(&mut command, None, &admission, &context)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(!not_started.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_byte_admission_precedes_spawn_and_cancellation_releases_capture() {
        let admission = std::sync::Arc::new(Admission::default());
        let process = splitty_transport::budget::ProcessBudget::new(4 * SETUP_STDOUT, 0);
        let budget = ConnectionBudget::new(process.clone(), 4 * SETUP_STDOUT, 0);
        let held = process.data.acquire(4 * SETUP_STDOUT).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let mut command = Command::new("touch");
        command.arg(&marker);
        assert_eq!(
            run_budgeted(&mut command, None, &admission, &budget)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!marker.exists());
        assert_eq!(admission.counts().workers, 0);
        assert_eq!(budget.used(), 0);
        drop(held);
        let task_budget = budget.clone();
        let task_admission = admission.clone();
        let path = marker.clone();
        let task = tokio::spawn(async move {
            let mut command = Command::new("sh");
            command
                .args(["-c", "printf started > \"$1\"; exec sleep 30", "sh"])
                .arg(path);
            run_budgeted(&mut command, None, &task_admission, &task_budget).await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(budget.used() > SETUP_STDOUT);
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while admission.counts().workers != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(process.used(), 0);
    }

    /// SR-13: a target that ssh would read as an option must be refused before
    /// the process starts. `-oProxyCommand=` is the dangerous case — it makes
    /// OpenSSH execute a local program.
    #[test]
    fn option_like_targets_are_rejected() {
        for raw in [
            "-oProxyCommand=/bin/sh -c evil",
            "-F/tmp/evil_config",
            "-J jump.example",
            "-luser",
            "--",
            // Unicode dashes: a value pasted from a document or chat.
            "\u{2013}oProxyCommand=x",
            "\u{2212}F/tmp/x",
        ] {
            let result = SshTarget::parse(raw);
            assert!(result.is_err(), "{raw:?} must be rejected, got {result:?}");
        }
    }

    #[test]
    fn whitespace_and_control_characters_are_rejected() {
        for raw in [
            "host name",
            "host\tname",
            "host\nname",
            "host\0name",
            "host\rname",
        ] {
            assert!(SshTarget::parse(raw).is_err(), "{raw:?} must be rejected");
        }
    }

    #[test]
    fn ordinary_destinations_are_accepted() {
        for raw in [
            "host",
            "host.example.com",
            "user@host",
            "user@host.example.com",
            "user.name@host-1.example.com",
            "192.0.2.1",
            "user@192.0.2.1",
            "[2001:db8::1]",
            "user@[2001:db8::1]",
            "[2001:db8::1]:2222",
            "host.example.com:2222",
        ] {
            assert!(SshTarget::parse(raw).is_ok(), "{raw:?} should be accepted");
        }
    }

    #[test]
    fn target_byte_ceiling_preserves_long_valid_users() {
        let target = format!("{}@h", "u".repeat(MAX_TARGET_BYTES - 2));
        assert_eq!(SshTarget::parse(&target).unwrap().as_str(), target);
        assert_eq!(
            SshTarget::parse(&format!("u{target}")),
            Err(TargetError::TooLong)
        );
    }

    #[test]
    fn oversized_targets_are_rejected_before_normalization_or_diagnostics() {
        for raw in [
            format!("{}@host", "u".repeat(1024 * 1024)),
            format!("-{}", "x".repeat(1024 * 1024)),
            format!("host\n{}", "x".repeat(1024 * 1024)),
            format!("{}host", " ".repeat(MAX_TARGET_BYTES)),
            "界".repeat(MAX_TARGET_BYTES / 3 + 1),
        ] {
            let error = SshTarget::parse(&raw).unwrap_err();
            assert_eq!(error, TargetError::TooLong);
            assert_eq!(error.to_string(), "SSH target exceeds 4096 bytes");
            assert_eq!(format!("{error:?}"), "TooLong");
        }
    }

    #[test]
    fn malformed_destinations_are_rejected() {
        for raw in [
            "",
            "@host",
            "user@",
            "user@@host",
            "host:0",
            "host:99999",
            "host:notaport",
            "[not-an-ipv6]",
            "[2001:db8::1",
            "-",
            "host..example",
            "host-.example",
        ] {
            assert!(SshTarget::parse(raw).is_err(), "{raw:?} must be rejected");
        }
    }

    /// A user part containing a dash must not be mistaken for an option: only a
    /// *leading* dash on the whole value is dangerous.
    #[test]
    fn dashes_inside_the_target_are_fine() {
        assert!(SshTarget::parse("my-user@my-host.example.com").is_ok());
    }

    #[test]
    fn hardening_options_pin_the_dangerous_defaults() {
        let options = hardening_options().join(" ");
        assert!(
            options.contains("ExitOnForwardFailure=yes"),
            "without this ssh survives a failed forward and looks healthy",
        );
        assert!(options.contains("BatchMode=yes"));
    }

    fn args(ssh: &Ssh) -> Vec<String> {
        ssh.command()
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// A stdio-only connection forwards nothing, whatever the user's
    /// configuration says; a plain one keeps the configuration's choices.
    #[test]
    fn stdio_only_connections_forward_nothing() {
        let options = stdio_only_options();
        for option in ["-T", "-a", "-x", "ClearAllForwardings=yes"] {
            assert!(options.iter().any(|o| o == option), "{option}");
        }
        let target = SshTarget::parse("vm").expect("target");
        let stdio_only = args(&Ssh::stdio_only(target.clone()));
        assert!(
            stdio_only.starts_with(&hardening_options()),
            "{stdio_only:?}"
        );
        assert!(stdio_only.ends_with(&options), "{stdio_only:?}");
        assert_eq!(args(&Ssh::new(target)), hardening_options());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_stdio_overrides_background_masters_and_configured_forwarding() {
        if std::process::Command::new("ssh")
            .arg("-V")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config");
        std::fs::write(&config, "Host host\n  HostName 192.0.2.1\n  ControlMaster auto\n  ControlPath /tmp/unused-splitty-test-master\n  ControlPersist yes\n  ForkAfterAuthentication yes\n  StdinNull yes\n  ForwardAgent yes\n  ForwardX11 yes\n  RequestTTY force\n  LocalForward 5432 localhost:5432\n  RemoteForward 8080 localhost:8080\n  DynamicForward 1080\n").unwrap();
        let ssh = Ssh::stdio_only(SshTarget::parse("host").unwrap());
        let mut command = ssh.command();
        command.arg("-F").arg(config).args(["-G", "host"]);
        let result = run_with_input(command, None).await.unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = std::str::from_utf8(&result.stdout).unwrap();
        let values: std::collections::HashMap<_, _> = text
            .lines()
            .filter_map(|line| line.split_once(' '))
            .collect();
        for key in [
            "controlmaster",
            "controlpersist",
            "forkafterauthentication",
            "stdinnull",
            "forwardagent",
            "forwardx11",
            "requesttty",
        ] {
            assert!(
                values
                    .get(key)
                    .is_some_and(|value| matches!(*value, "no" | "false")),
                "{key}: {:?}",
                values.get(key)
            );
        }
        assert!(values
            .get("controlpath")
            .is_none_or(|value| *value == "none"));
        assert!(forwards_in_config(text).is_empty());
    }

    /// A forward-only connection runs no command and adds no terminal,
    /// agent or X11 forwarding, but keeps the forwards of its command line:
    /// no `ClearAllForwardings`.
    #[test]
    fn forward_only_connections_carry_only_their_forwards() {
        let target = SshTarget::parse("vm").expect("target");
        let forward_only = args(&Ssh::forward_only(target));
        assert!(
            forward_only.starts_with(&hardening_options()),
            "{forward_only:?}"
        );
        assert!(
            forward_only.ends_with(&forward_only_options()),
            "{forward_only:?}"
        );
        for option in ["-N", "-T", "-a", "-x"] {
            assert!(forward_only.iter().any(|o| o == option), "{option}");
        }
        assert!(
            !forward_only
                .iter()
                .any(|o| o.contains("ClearAllForwardings")),
            "{forward_only:?}"
        );
    }

    #[test]
    fn the_forwards_of_a_resolved_configuration_are_found() {
        let config =
            "user me\nhostname 10.0.0.5\nclearallforwardings no\nexitonforwardfailure yes\n\
                      gatewayports yes\nforwardagent no\ndynamicforward 1080\n\
                      localforward 5432 [localhost]:5432\nremoteforward 9999 [localhost]:9999\n\
                      forwardx11 no\n";
        assert_eq!(
            forwards_in_config(config),
            [
                "dynamicforward 1080",
                "localforward 5432 [localhost]:5432",
                "remoteforward 9999 [localhost]:9999",
            ]
        );
        assert_eq!(
            forwards_in_config("user me\nforwardagent no\nclearallforwardings no\n"),
            Vec::<String>::new()
        );
    }

    /// What the system's `ssh -G` reports for a host with forwards in its
    /// configuration, and for one without. Needs only the `ssh` client.
    #[cfg(unix)]
    #[tokio::test]
    async fn configured_forwards_come_from_the_resolved_configuration() {
        if std::process::Command::new("ssh")
            .arg("-V")
            .output()
            .is_err()
        {
            eprintln!("skipping: no ssh client");
            return;
        }
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            "Host tunnels\n  HostName 192.0.2.1\n  RemoteForward 9999 localhost:9999\n  \
             LocalForward 5432 localhost:5432\n\nHost plain\n  HostName 192.0.2.2\n  \
             GatewayPorts yes\n",
        )
        .expect("config");
        let with_config = |host: &str| Ssh {
            options: vec![
                "-F".to_owned(),
                config.display().to_string(),
                "-N".to_owned(),
                "-T".to_owned(),
            ],
            ..Ssh::new(SshTarget::parse(host).expect("target"))
        };

        let forwards = with_config("tunnels")
            .configured_forwards()
            .await
            .expect("ssh -G");
        assert_eq!(forwards.len(), 2, "{forwards:?}");
        assert!(
            forwards.iter().any(|f| f.starts_with("remoteforward 9999")),
            "{forwards:?}"
        );
        assert!(
            forwards.iter().any(|f| f.starts_with("localforward 5432")),
            "{forwards:?}"
        );
        assert_eq!(
            with_config("plain")
                .configured_forwards()
                .await
                .expect("ssh -G"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn refused_credentials_are_recognized() {
        assert!(authentication_failed(
            "user@vm: Permission denied (publickey,password)."
        ));
        assert!(authentication_failed(
            "Received disconnect from 192.0.2.1 port 22:2: Too many authentication failures"
        ));
        assert!(!authentication_failed(
            "ssh: connect to host vm port 22: Connection refused"
        ));
        assert!(!authentication_failed("Host key verification failed."));
    }

    #[cfg(unix)]
    /// What the host's shell makes of a quoted value: exactly the value.
    fn through_sh(value: &str) -> String {
        let line = CommandLine::new("printf").word("%s").value(value);
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(line.as_str())
            .output()
            .expect("sh");
        assert!(output.status.success(), "{line:?}");
        String::from_utf8(output.stdout).expect("utf-8")
    }

    #[cfg(unix)]
    #[test]
    fn quoted_values_reach_the_shell_unchanged() {
        for value in [
            "plain",
            "with space",
            "it's",
            "'",
            "''",
            "$(touch /tmp/splitty-quote-pwned)",
            "`id`",
            "a\nb",
            "line\nbreak",
            "-n",
            "--version",
            "*",
            "~",
            "a;b|c&d>e<f",
            "\\",
            "",
        ] {
            assert_eq!(through_sh(value), value, "{value:?}");
        }
        assert!(!std::path::Path::new("/tmp/splitty-quote-pwned").exists());
    }

    #[test]
    fn versions_and_checksums_are_allowlisted() {
        for good in ["0.1.0", "1.2.3-beta.1", "0.1.0+local"] {
            assert!(is_version(good), "{good}");
        }
        for bad in [
            "",
            "-1",
            ".1",
            "0.1.0;id",
            "0.1.0 x",
            "0.1/0",
            &"9".repeat(65),
        ] {
            assert!(!is_version(bad), "{bad:?}");
        }
        assert!(is_sha256(&"a1".repeat(32)));
        assert!(!is_sha256(&"A1".repeat(32)));
        assert!(!is_sha256(&"a1".repeat(31)));
        assert!(!is_sha256(&"g1".repeat(32)));
    }

    #[cfg(unix)]
    fn exit(code: i32) -> ExitStatus {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exit {code}"))
            .status()
            .expect("sh")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checked_setup_conversion_retains_its_owner_budget_after_capture_drop() {
        struct FailedSetup(ConnectionBudget);
        impl Executor for FailedSetup {
            fn host(&self) -> &str {
                "vm"
            }
            fn budget(&self) -> ConnectionBudget {
                self.0.clone()
            }
            async fn run(
                &self,
                _line: &CommandLine,
                _input: Option<Vec<u8>>,
            ) -> io::Result<CapturedOutput> {
                let mut command = Command::new("sh");
                command.args([
                    "-c",
                    "printf '\\377Host key verification failed.\\200' >&2; exit 255",
                ]);
                run_budgeted(&mut command, None, &Admission::default(), &self.0).await
            }
        }
        let budget = ConnectionBudget::default();
        let error = run_checked(
            &FailedSetup(budget.clone()),
            &CommandLine::new("ignored"),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains('\u{fffd}'));
        assert!(error.hint().is_some());
        assert!(
            budget.used() > 0,
            "derived error owns its converted bytes after raw output drops"
        );
        drop(error);
        assert_eq!(budget.used(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn converted_diagnostic_is_admitted_and_retained_in_the_owner_budget() {
        let budget = ConnectionBudget::default();
        for host in ["vm".into(), "u".repeat(4096)] {
            for diagnostic in [
                "Connection refused".into(),
                "Host key verification failed. 界".repeat(1024),
            ] {
                let error = classify_budgeted(&host, exit(255), &diagnostic, &budget);
                match &error {
                    SshError::Failed {
                        message,
                        stderr,
                        hint,
                        allocation,
                        ..
                    } => {
                        let payload = message.capacity()
                            + stderr.capacity()
                            + hint.as_ref().map_or(0, String::capacity);
                        assert!(allocation.is_some());
                        assert!(
                            budget.used() >= payload,
                            "converted copies outlive the source capture"
                        );
                        assert!(message.contains(&diagnostic));
                    }
                    other => panic!("unexpected classification: {other}"),
                }
                drop(error);
                assert_eq!(budget.used(), 0);
            }
        }
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES,
                false,
            )
            .unwrap();
        assert!(
            matches!(classify_budgeted("vm", exit(255), "Host key verification failed", &budget), SshError::Spawn(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_command_is_told_apart_from_ssh_errors() {
        let missing = classify("vm", exit(127), "sh: 1: splitty: not found");
        assert!(matches!(missing, SshError::NotFound { .. }), "{missing:?}");

        let refused = classify(
            "vm",
            exit(255),
            "ssh: connect to host vm port 22: Connection refused",
        );
        assert!(
            matches!(
                refused,
                SshError::Failed {
                    status: Some(255),
                    ..
                }
            ),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("Connection refused"));
        assert!(!refused.to_string().contains("fingerprint"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_host_key_check_says_how_to_verify_the_key() {
        let error = classify("user@vm", exit(255), "Host key verification failed.");
        let hint = error.hint().expect("a hint");
        assert!(hint.contains("Run `ssh user@vm` once"), "{hint}");
        assert!(hint.contains("fingerprint"), "{hint}");
        assert!(!error.to_string().contains("fingerprint"), "{error}");

        let refused = classify("user@vm", exit(255), "Connection refused");
        assert_eq!(refused.hint(), None);
    }

    /// The system's OpenSSH first, then whatever `ssh` the `PATH` finds.
    #[test]
    fn windows_runs_the_system_openssh_when_it_is_installed() {
        let root = tempfile::tempdir().expect("system root");
        let system = root.path().join("System32").join("OpenSSH").join("ssh.exe");
        let root_name = root.path().as_os_str();

        assert_eq!(
            windows_ssh(Some(root_name), Path::is_file),
            Path::new("ssh")
        );
        std::fs::create_dir_all(system.parent().expect("parent")).expect("mkdir");
        std::fs::write(&system, b"").expect("ssh.exe");
        assert_eq!(windows_ssh(Some(root_name), Path::is_file), system);

        assert_eq!(windows_ssh(None, |_| true), Path::new("ssh"));
        assert_eq!(
            windows_ssh(Some(std::ffi::OsStr::new("")), |_| true),
            Path::new("ssh")
        );
    }
}
