//! The frames of `splitty remote-bridge`: the host daemon's WebSocket
//! messages, carried over the bridge's standard input and output.
//!
//! One frame is one message: a kind byte (1 text, 2 binary), the length as a
//! big-endian `u32`, then the payload. Both ends use this module; [`Bridge`]
//! is the client's end, which runs the bridge on a host through `ssh`.

use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::install::Server;
use crate::ssh::{keep_end, kept_end, Ssh, SshTarget};

/// The largest payload a frame may carry.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

const TEXT: u8 = 1;
const BINARY: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// The bytes of one frame.
pub fn encode(frame: &Frame) -> io::Result<Vec<u8>> {
    let (kind, payload) = match frame {
        Frame::Text(text) => (TEXT, text.as_bytes()),
        Frame::Binary(bytes) => (BINARY, bytes.as_slice()),
    };
    let length = u32::try_from(payload.len())
        .ok()
        .filter(|_| payload.len() <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut bytes = Vec::with_capacity(5 + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// Write one frame and flush it.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    writer.write_all(&encode(frame)?).await?;
    writer.flush().await
}

/// Read one frame; `None` when the input ends cleanly between frames.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut kind = [0u8; 1];
    if reader.read(&mut kind).await? == 0 {
        return Ok(None);
    }
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).await?;
    match kind[0] {
        TEXT => String::from_utf8(payload)
            .map(|text| Some(Frame::Text(text)))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "text frame is not UTF-8")),
        BINARY => Ok(Some(Frame::Binary(payload))),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown frame kind {other}"),
        )),
    }
}

/// `splitty remote-bridge` on a host, run as a child process: frames go to its
/// standard input and come from its standard output, and the end of its error
/// output is kept for a message. The process is killed when the bridge, or its
/// [`BridgeProcess`] after [`Bridge::split`], is dropped.
pub struct Bridge {
    sender: BridgeSender,
    receiver: BridgeReceiver,
    process: BridgeProcess,
}

/// The half of a [`Bridge`] that sends frames to the host. Dropping it ends
/// the bridge's input, which ends the bridge cleanly.
pub struct BridgeSender {
    stdin: ChildStdin,
}

/// The half of a [`Bridge`] that receives the host's frames.
pub struct BridgeReceiver {
    stdout: BufReader<ChildStdout>,
}

/// The process of a [`Bridge`].
pub struct BridgeProcess {
    child: Child,
    stderr: Option<JoinHandle<String>>,
}

/// How a bridge's process ended.
#[derive(Debug)]
pub struct Ended {
    /// Its exit status, unless waiting for it failed.
    pub status: io::Result<ExitStatus>,
    /// The end of its error output, trimmed.
    pub stderr: String,
}

impl Bridge {
    /// `splitty remote-bridge` on `target`, run by `server`, over a
    /// connection that carries nothing but the bridge's standard input and
    /// output ([`Ssh::stdio_only`]). Needs a Tokio runtime.
    pub fn open(target: &SshTarget, server: &Server) -> io::Result<Self> {
        Self::spawn(open_command(&Ssh::stdio_only(target.clone()), server))
    }

    /// Run `command` as a bridge, with its standard streams piped. Needs a
    /// Tokio runtime.
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let not_piped = || io::Error::other("the bridge's standard streams are not piped");
        let stdin = child.stdin.take().ok_or_else(not_piped)?;
        let stdout = child.stdout.take().ok_or_else(not_piped)?;
        let stderr = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(keep_end(stderr)));
        Ok(Self {
            sender: BridgeSender { stdin },
            receiver: BridgeReceiver {
                stdout: BufReader::new(stdout),
            },
            process: BridgeProcess { child, stderr },
        })
    }

    pub async fn send(&mut self, frame: &Frame) -> io::Result<()> {
        self.sender.send(frame).await
    }

    /// The next frame from the host; `None` when the bridge's output ended.
    pub async fn recv(&mut self) -> io::Result<Option<Frame>> {
        self.receiver.recv().await
    }

    pub fn process(&mut self) -> &mut BridgeProcess {
        &mut self.process
    }

    /// Its halves, to send and receive at the same time.
    pub fn split(self) -> (BridgeSender, BridgeReceiver, BridgeProcess) {
        (self.sender, self.receiver, self.process)
    }
}

/// `ssh <options> <target> -- '<server>' remote-bridge`.
fn open_command(ssh: &Ssh, server: &Server) -> Command {
    let line = server.command().word("remote-bridge");
    let mut command = ssh.command();
    command
        .arg(ssh.target().as_str())
        .arg("--")
        .arg(line.as_str());
    command
}

impl BridgeSender {
    pub async fn send(&mut self, frame: &Frame) -> io::Result<()> {
        write_frame(&mut self.stdin, frame).await
    }
}

impl BridgeReceiver {
    /// The next frame; `None` when the output ended between frames. Not
    /// cancel-safe: a frame read halfway is lost.
    pub async fn recv(&mut self) -> io::Result<Option<Frame>> {
        read_frame(&mut self.stdout).await
    }
}

impl BridgeProcess {
    /// The process id, while it runs.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Wait for the process to end.
    pub async fn ended(&mut self) -> Ended {
        let status = self.child.wait().await;
        let stderr = kept_end(self.stderr.take()).await;
        Ended { status, stderr }
    }

    /// Kill the process and wait for it.
    pub async fn kill(&mut self) -> Ended {
        // Fails only when the process has already been waited for.
        let _ = self.child.start_kill();
        self.ended().await
    }

    /// End a bridge whose [`BridgeSender`] is dropped: it gets `grace` to
    /// finish on its own, then it is killed.
    pub async fn finish(&mut self, grace: Duration) -> Ended {
        match timeout(grace, self.child.wait()).await {
            Ok(_) => self.ended().await,
            Err(_) => self.kill().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_in_order() {
        let frames = [
            Frame::Text(r#"{"type":"mux.resync"}"#.into()),
            Frame::Binary(vec![0, 1, 2, 255]),
            Frame::Text(String::new()),
            Frame::Binary(Vec::new()),
        ];
        let mut bytes = Vec::new();
        for frame in &frames {
            write_frame(&mut bytes, frame).await.expect("write");
        }
        assert_eq!(&bytes[..5], [1, 0, 0, 0, 21]);
        let mut reader = bytes.as_slice();
        for frame in &frames {
            assert_eq!(
                read_frame(&mut reader).await.expect("read").as_ref(),
                Some(frame)
            );
        }
        assert_eq!(read_frame(&mut reader).await.expect("end"), None);
    }

    #[tokio::test]
    async fn oversized_unknown_and_truncated_frames_are_errors() {
        let mut oversized = vec![2u8];
        oversized.extend_from_slice(&((MAX_FRAME + 1) as u32).to_be_bytes());
        assert!(read_frame(&mut oversized.as_slice()).await.is_err());
        assert!(encode(&Frame::Binary(vec![0; MAX_FRAME + 1])).is_err());

        let unknown = [9u8, 0, 0, 0, 0];
        assert!(read_frame(&mut unknown.as_slice()).await.is_err());

        let truncated = [1u8, 0, 0, 0, 5, b'a'];
        assert!(read_frame(&mut truncated.as_slice()).await.is_err());

        let bad_text = [1u8, 0, 0, 0, 1, 0xff];
        assert!(read_frame(&mut bad_text.as_slice()).await.is_err());
    }

    /// The bridge runs the installed server by its quoted path, over a
    /// connection that forwards nothing.
    #[test]
    fn the_bridge_runs_the_hosts_server_over_a_stdio_only_connection() {
        let ssh = Ssh::stdio_only(SshTarget::parse("user@vm").expect("target"));
        let server = Server::Installed {
            path: "/home/it's me/.local/share/splitty/remote/0.1.0/splitty".into(),
            config_dir: None,
        };
        let args: Vec<String> = open_command(&ssh, &server)
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let mut expected = crate::ssh::hardening_options();
        expected.extend(crate::ssh::stdio_only_options());
        expected.extend([
            "user@vm".to_owned(),
            "--".to_owned(),
            r"'/home/it'\''s me/.local/share/splitty/remote/0.1.0/splitty' remote-bridge"
                .to_owned(),
        ]);
        assert_eq!(args, expected);
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    /// `cat` echoes every frame: what goes in comes back, in order, and the end
    /// of the input ends the bridge cleanly.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_bridge_carries_frames_both_ways_and_ends_with_its_input() {
        let mut bridge = Bridge::spawn(sh("exec cat")).expect("spawn");
        let frames = [
            Frame::Text(r#"{"type":"mux.resync"}"#.into()),
            Frame::Binary(vec![0, 1, 2, 255]),
        ];
        for frame in &frames {
            bridge.send(frame).await.expect("send");
        }
        for frame in &frames {
            assert_eq!(bridge.recv().await.expect("recv").as_ref(), Some(frame));
        }

        let (sender, mut receiver, mut process) = bridge.split();
        drop(sender);
        assert_eq!(receiver.recv().await.expect("end"), None);
        let ended = process.finish(Duration::from_secs(10)).await;
        assert!(ended.status.expect("status").success());
    }

    /// What the host's bridge says when it fails is kept for the message.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_bridge_leaves_its_status_and_error_output() {
        let mut bridge = Bridge::spawn(sh(
            "echo 'No Splitty daemon is running on this host.' >&2; exit 1",
        ))
        .expect("spawn");
        assert_eq!(bridge.recv().await.expect("end"), None);
        let ended = bridge.process().ended().await;
        assert_eq!(ended.status.expect("status").code(), Some(1));
        assert_eq!(ended.stderr, "No Splitty daemon is running on this host.");
    }

    /// Only the end of a long error output is kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_end_of_a_long_error_output_is_kept() {
        let mut bridge = Bridge::spawn(sh(
            "i=0; while [ $i -lt 2000 ]; do echo 'debug1: noise' >&2; i=$((i+1)); done; \
             echo 'the reason' >&2",
        ))
        .expect("spawn");
        let ended = bridge.process().ended().await;
        assert!(
            ended.stderr.len() <= crate::ssh::STDERR_KEPT,
            "{}",
            ended.stderr.len()
        );
        assert!(ended.stderr.ends_with("the reason"), "{}", ended.stderr);
    }

    /// A bridge that does not end with its input is killed after the grace
    /// period.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_bridge_that_ignores_its_input_is_killed() {
        let bridge = Bridge::spawn(sh("exec sleep 30")).expect("spawn");
        let (sender, _receiver, mut process) = bridge.split();
        drop(sender);
        let started = std::time::Instant::now();
        let ended = process.finish(Duration::from_millis(200)).await;
        assert!(!ended.status.expect("status").success(), "killed");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(process.id(), None, "the process is gone");
    }
}
