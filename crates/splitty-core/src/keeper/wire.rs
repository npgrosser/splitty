//! The keeper protocol's frames: a 4-byte big-endian length, a kind byte and
//! the payload. Control frames carry JSON; `input` and `output` carry raw
//! bytes behind a sequence number or an offset. One decoder is the keeper's
//! whole parsing surface, so it bounds what it reads.
//!
//! Version 1 is meant to last: a daemon controls keepers that older versions
//! started. Changes stay additive and are announced as capabilities in
//! [`Hello`]; unknown kinds and fields are skipped.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// The protocol version of this build's keepers and controllers.
pub const PROTOCOL: u32 = 1;

/// A frame above this ends the connection.
pub const MAX_FRAME: usize = 1 << 20;

/// What a daemon sends a keeper it has just started: everything the program
/// needs, so nothing of it shows on the keeper's command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSpec {
    pub session_id: String,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub screen_id: Option<String>,
    pub argv: Vec<String>,
    /// The program's whole environment; the keeper's own is not inherited.
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// A managed job: input fails at once on a full terminal, and `close`
    /// stops the program instead of hanging it up.
    pub managed: bool,
    pub buffer_size: usize,
    /// The socket to serve, chosen by the daemon in its keeper directory.
    pub socket: String,
}

/// The keeper's first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub session_id: String,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub screen_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Milliseconds since the Unix epoch.
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    /// The offsets of the oldest buffered byte and of the next one.
    #[serde(default)]
    pub buffer_start: u64,
    #[serde(default)]
    pub buffer_end: u64,
    /// The program's name, for a listing; its working directory a listing
    /// looks up by the PID.
    #[serde(default)]
    pub program: String,
    /// The keeper's own executable: the `splitty` that a daemon for its
    /// session can be started with.
    #[serde(default)]
    pub executable: String,
    /// Whether a controller holds the keeper.
    #[serde(default)]
    pub controlled: bool,
    /// The program's result, once it has ended.
    #[serde(default)]
    pub exited: Option<Exit>,
}

/// How a program ended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exit {
    #[serde(default)]
    pub exit_code: Option<u32>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl From<crate::pty::ProcessResult> for Exit {
    fn from(result: crate::pty::ProcessResult) -> Self {
        Self {
            exit_code: result.exit_code,
            signal: result.signal,
            error: result.error,
        }
    }
}

impl From<Exit> for crate::pty::ProcessResult {
    fn from(exit: Exit) -> Self {
        Self {
            exit_code: exit.exit_code,
            signal: exit.signal,
            error: exit.error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    // Controller to keeper.
    Spawn(Box<SpawnSpec>),
    /// Become the keeper's controller, resuming at `from` (the oldest
    /// buffered byte when absent).
    Claim {
        takeover: bool,
        from: Option<u64>,
    },
    Input {
        seq: u32,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Hang up an ordinary session or stop a managed one, and end without
    /// waiting for an acknowledgement.
    Close,
    /// Stop a managed program with escalation.
    Stop,
    ExitAck,
    /// Let go of the keeper; it keeps running.
    Release,

    // Keeper to controller.
    Hello(Box<Hello>),
    /// The keeper started nothing. `program` tells that the program itself
    /// could not start, an error to report; otherwise the keeper could not
    /// set itself up, and the daemon may run the session itself.
    SpawnFailed {
        error: String,
        program: bool,
    },
    /// The claim succeeded: output follows from `from`; `lost` when the
    /// requested offset was older than the buffer.
    Claimed {
        from: u64,
        lost: bool,
    },
    Busy,
    TakenOver,
    Output {
        offset: u64,
        data: Vec<u8>,
    },
    InputResult {
        seq: u32,
        error: Option<String>,
    },
    /// The terminal's foreground process group changed.
    Foreground {
        pid: Option<u32>,
    },
    Exited(Exit),
    /// The terminal closed: no output follows.
    OutputEnded,

    /// A kind this build does not know, skipped.
    Unknown(u8),
}

mod kind {
    pub const SPAWN: u8 = 0x80;
    pub const CLAIM: u8 = 0x81;
    pub const INPUT: u8 = 0x82;
    pub const RESIZE: u8 = 0x83;
    pub const CLOSE: u8 = 0x84;
    pub const STOP: u8 = 0x85;
    pub const EXIT_ACK: u8 = 0x86;
    pub const RELEASE: u8 = 0x87;

    pub const HELLO: u8 = 0x01;
    pub const SPAWN_FAILED: u8 = 0x02;
    pub const CLAIMED: u8 = 0x03;
    pub const BUSY: u8 = 0x04;
    pub const TAKEN_OVER: u8 = 0x05;
    pub const OUTPUT: u8 = 0x06;
    pub const INPUT_RESULT: u8 = 0x07;
    pub const FOREGROUND: u8 = 0x08;
    pub const EXITED: u8 = 0x09;
    pub const OUTPUT_ENDED: u8 = 0x0a;
}

#[derive(Serialize, Deserialize)]
struct ClaimBody {
    takeover: bool,
    #[serde(default)]
    from: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct SizeBody {
    cols: u16,
    rows: u16,
}

#[derive(Serialize, Deserialize)]
struct SpawnFailedBody {
    error: String,
    #[serde(default)]
    program: bool,
}

#[derive(Serialize, Deserialize)]
struct ClaimedBody {
    from: u64,
    lost: bool,
}

#[derive(Serialize, Deserialize)]
struct InputResultBody {
    seq: u32,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ForegroundBody {
    #[serde(default)]
    pid: Option<u32>,
}

impl Frame {
    /// The frame's kind byte and payload.
    fn encode(&self) -> io::Result<(u8, Vec<u8>)> {
        fn json<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
            serde_json::to_vec(value).map_err(io::Error::other)
        }
        Ok(match self {
            Self::Spawn(spec) => (kind::SPAWN, json(spec.as_ref())?),
            Self::Claim { takeover, from } => (
                kind::CLAIM,
                json(&ClaimBody {
                    takeover: *takeover,
                    from: *from,
                })?,
            ),
            Self::Input { seq, data } => (kind::INPUT, [&seq.to_be_bytes()[..], data].concat()),
            Self::Resize { cols, rows } => (
                kind::RESIZE,
                json(&SizeBody {
                    cols: *cols,
                    rows: *rows,
                })?,
            ),
            Self::Close => (kind::CLOSE, Vec::new()),
            Self::Stop => (kind::STOP, Vec::new()),
            Self::ExitAck => (kind::EXIT_ACK, Vec::new()),
            Self::Release => (kind::RELEASE, Vec::new()),
            Self::Hello(hello) => (kind::HELLO, json(hello.as_ref())?),
            Self::SpawnFailed { error, program } => (
                kind::SPAWN_FAILED,
                json(&SpawnFailedBody {
                    error: error.clone(),
                    program: *program,
                })?,
            ),
            Self::Claimed { from, lost } => (
                kind::CLAIMED,
                json(&ClaimedBody {
                    from: *from,
                    lost: *lost,
                })?,
            ),
            Self::Busy => (kind::BUSY, Vec::new()),
            Self::TakenOver => (kind::TAKEN_OVER, Vec::new()),
            Self::Output { offset, data } => {
                (kind::OUTPUT, [&offset.to_be_bytes()[..], data].concat())
            }
            Self::InputResult { seq, error } => (
                kind::INPUT_RESULT,
                json(&InputResultBody {
                    seq: *seq,
                    error: error.clone(),
                })?,
            ),
            Self::Foreground { pid } => (kind::FOREGROUND, json(&ForegroundBody { pid: *pid })?),
            Self::Exited(exit) => (kind::EXITED, json(exit)?),
            Self::OutputEnded => (kind::OUTPUT_ENDED, Vec::new()),
            Self::Unknown(kind) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("cannot send a frame of unknown kind {kind:#04x}"),
                ))
            }
        })
    }

    fn decode(kind: u8, payload: &[u8]) -> io::Result<Self> {
        fn json<T: serde::de::DeserializeOwned>(payload: &[u8]) -> io::Result<T> {
            serde_json::from_slice(payload).map_err(|e| invalid(e.to_string()))
        }
        fn split<const N: usize>(payload: &[u8]) -> io::Result<([u8; N], Vec<u8>)> {
            if payload.len() < N {
                return Err(invalid("frame too short".into()));
            }
            let (head, rest) = payload.split_at(N);
            let mut array = [0u8; N];
            array.copy_from_slice(head);
            Ok((array, rest.to_vec()))
        }
        Ok(match kind {
            kind::SPAWN => Self::Spawn(Box::new(json(payload)?)),
            kind::CLAIM => {
                let body: ClaimBody = json(payload)?;
                Self::Claim {
                    takeover: body.takeover,
                    from: body.from,
                }
            }
            kind::INPUT => {
                let (seq, data) = split::<4>(payload)?;
                Self::Input {
                    seq: u32::from_be_bytes(seq),
                    data,
                }
            }
            kind::RESIZE => {
                let body: SizeBody = json(payload)?;
                Self::Resize {
                    cols: body.cols,
                    rows: body.rows,
                }
            }
            kind::CLOSE => Self::Close,
            kind::STOP => Self::Stop,
            kind::EXIT_ACK => Self::ExitAck,
            kind::RELEASE => Self::Release,
            kind::HELLO => Self::Hello(Box::new(json(payload)?)),
            kind::SPAWN_FAILED => {
                let body: SpawnFailedBody = json(payload)?;
                Self::SpawnFailed {
                    error: body.error,
                    program: body.program,
                }
            }
            kind::CLAIMED => {
                let body: ClaimedBody = json(payload)?;
                Self::Claimed {
                    from: body.from,
                    lost: body.lost,
                }
            }
            kind::BUSY => Self::Busy,
            kind::TAKEN_OVER => Self::TakenOver,
            kind::OUTPUT => {
                let (offset, data) = split::<8>(payload)?;
                Self::Output {
                    offset: u64::from_be_bytes(offset),
                    data,
                }
            }
            kind::INPUT_RESULT => {
                let body: InputResultBody = json(payload)?;
                Self::InputResult {
                    seq: body.seq,
                    error: body.error,
                }
            }
            kind::FOREGROUND => Self::Foreground {
                pid: json::<ForegroundBody>(payload)?.pid,
            },
            kind::EXITED => Self::Exited(json(payload)?),
            kind::OUTPUT_ENDED => Self::OutputEnded,
            other => Self::Unknown(other),
        })
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Write one frame and flush it.
pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    let (kind, payload) = frame.encode()?;
    let length = payload.len() + 1;
    if length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("frame of {length} bytes exceeds the protocol's bound"),
        ));
    }
    let length = u32::try_from(length).map_err(|_| invalid("frame too long".into()))?;
    let mut bytes = Vec::with_capacity(payload.len() + 5);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.push(kind);
    bytes.extend_from_slice(&payload);
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Read one frame; `None` at a clean end of the stream between frames.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut length = [0u8; 4];
    // A stream that ends before a frame ends cleanly; one that ends within
    // the length does not.
    loop {
        match reader.read(&mut length[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut length[1..])?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| invalid("frame too long".into()))?;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid(format!("frame length {length} out of bounds")));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Frame::decode(body[0], &body[1..]).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            session_id: "s1".into(),
            pane_id: Some("p1".into()),
            screen_id: None,
            argv: vec!["/bin/sh".into(), "-l".into()],
            env: vec![("TERM".into(), "xterm-256color".into())],
            cwd: Some("/tmp".into()),
            cols: 80,
            rows: 24,
            managed: false,
            buffer_size: 1 << 20,
            socket: "/run/keepers/s1.sock".into(),
        }
    }

    #[test]
    fn every_kind_round_trips() {
        let frames = vec![
            Frame::Spawn(Box::new(spec())),
            Frame::Claim {
                takeover: true,
                from: Some(42),
            },
            Frame::Claim {
                takeover: false,
                from: None,
            },
            Frame::Input {
                seq: 7,
                data: b"ls\r\x00\xff".to_vec(),
            },
            Frame::Resize {
                cols: 100,
                rows: 30,
            },
            Frame::Close,
            Frame::Stop,
            Frame::ExitAck,
            Frame::Release,
            Frame::Hello(Box::new(Hello {
                protocol: PROTOCOL,
                capabilities: vec!["x".into()],
                session_id: "s1".into(),
                pane_id: Some("p1".into()),
                screen_id: Some("sc".into()),
                pid: Some(4242),
                started_at: 1,
                managed: true,
                cols: 80,
                rows: 24,
                buffer_start: 3,
                buffer_end: 9,
                program: "zsh".into(),
                executable: "/usr/local/bin/splitty".into(),
                controlled: true,
                exited: Some(Exit {
                    exit_code: Some(3),
                    signal: None,
                    error: None,
                }),
            })),
            Frame::SpawnFailed {
                error: "no such program".into(),
                program: true,
            },
            Frame::Claimed {
                from: 5,
                lost: true,
            },
            Frame::Busy,
            Frame::TakenOver,
            Frame::Output {
                offset: u64::MAX - 1,
                data: vec![0x1b, b'[', b'm'],
            },
            Frame::InputResult {
                seq: 9,
                error: Some("full".into()),
            },
            Frame::Foreground { pid: Some(77) },
            Frame::Exited(Exit {
                exit_code: None,
                signal: Some("SIGKILL".into()),
                error: None,
            }),
            Frame::OutputEnded,
        ];
        let mut bytes = Vec::new();
        for frame in &frames {
            write_frame(&mut bytes, frame).expect("encode");
        }
        let mut reader = bytes.as_slice();
        for frame in &frames {
            assert_eq!(
                read_frame(&mut reader).expect("decode").as_ref(),
                Some(frame)
            );
        }
        assert_eq!(read_frame(&mut reader).expect("clean end"), None);
    }

    #[test]
    fn truncated_oversized_and_unknown_frames() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Frame::Busy).expect("encode");
        assert!(read_frame(&mut &bytes[..3]).is_err(), "a cut length");
        let mut truncated = Vec::new();
        write_frame(
            &mut truncated,
            &Frame::Output {
                offset: 1,
                data: vec![1, 2, 3],
            },
        )
        .expect("encode");
        truncated.pop();
        assert!(read_frame(&mut truncated.as_slice()).is_err());

        let oversized = u32::try_from(MAX_FRAME + 1).expect("bound").to_be_bytes();
        assert!(read_frame(&mut &oversized[..]).is_err());
        assert!(read_frame(&mut &[0, 0, 0, 0][..]).is_err());
        assert!(write_frame(
            &mut Vec::new(),
            &Frame::Output {
                offset: 0,
                data: vec![0; MAX_FRAME],
            }
        )
        .is_err());

        let unknown = [0, 0, 0, 3, 0x7f, b'h', b'i'];
        let mut reader = &unknown[..];
        assert_eq!(
            read_frame(&mut reader).expect("skipped"),
            Some(Frame::Unknown(0x7f))
        );
        assert_eq!(read_frame(&mut reader).expect("end"), None);
        assert!(read_frame(&mut &[0, 0, 0, 3, 0x04, b'{', b'}'][..]).is_ok());
    }

    #[test]
    fn unknown_fields_of_a_newer_hello_are_ignored() {
        let body = br#"{"protocol":1,"sessionId":"s","future":true}"#;
        let mut bytes = u32::try_from(body.len() + 1)
            .expect("len")
            .to_be_bytes()
            .to_vec();
        bytes.push(0x01);
        bytes.extend_from_slice(body);
        let Some(Frame::Hello(hello)) = read_frame(&mut bytes.as_slice()).expect("decode") else {
            panic!("a hello");
        };
        assert_eq!(hello.session_id, "s");
        assert_eq!(hello.pid, None);
        assert_eq!(hello.executable, "", "an older keeper names none");
    }
}
