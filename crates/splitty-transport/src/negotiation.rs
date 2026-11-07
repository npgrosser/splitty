//! Framing and service admission are separate. Helpers may prepare a backend
//! between them, but no application frame is accepted by either handshake.
use crate::{
    budget::{Charged, ConnectionBudget},
    codec::{self, Frame},
    wire::{self, Hello, Kind, Purpose, Ready, ReadyAccept},
};
use std::{future::Future, io, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::timeout,
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

async fn deadline<T>(future: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    timeout(wire::HANDSHAKE_TIMEOUT, future)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "native handshake timed out"))?
}

async fn magic<R: AsyncRead + Unpin>(read: &mut R) -> io::Result<()> {
    let mut bytes = [0; 8];
    read.read_exact(&mut bytes).await?;
    if &bytes != wire::MAGIC {
        return Err(invalid("unsupported native transport magic"));
    }
    Ok(())
}

async fn expect<R: AsyncRead + Unpin, T: wire::ControlMessage>(
    read: &mut R,
    budget: &ConnectionBudget,
    kind: Kind,
) -> io::Result<Charged<T>> {
    let frame = codec::read(read, budget)
        .await?
        .ok_or_else(|| invalid("EOF during native handshake"))?;
    if frame.value.kind != kind {
        return Err(invalid("unexpected frame during native handshake"));
    }
    frame.value.decode_control(budget)
}

/// Reserve the outgoing handshake frame before serialization and keep it
/// across partial writes. A negotiation has no writer queue yet.
async fn write_control<W: AsyncWrite + Unpin, T: serde::Serialize>(
    write: &mut W,
    budget: &ConnectionBudget,
    kind: Kind,
    value: &T,
) -> io::Result<()> {
    let frame = Frame::control(kind, value, budget)?;
    codec::write(write, &frame).await
}

pub async fn client<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
) -> io::Result<()> {
    client_with_host(read, write, budget, None).await
}

/// Host intent is supplied by an authorized main broker/gateway. It is not a
/// command understood by a remote bridge or an assigned remote window.
pub async fn client_with_host<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
    open_host: Option<wire::OpenHost>,
) -> io::Result<()> {
    let mut disconnected_write = None;
    let result = deadline(async {
        let sent = async {
            write.write_all(wire::MAGIC).await?;
            write_control(
                write,
                budget,
                Kind::Hello,
                &Hello {
                    transport_version: wire::VERSION,
                    open_host,
                },
            )
            .await
        }
        .await;
        if let Err(error) = sent {
            if !matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::UnexpectedEof
            ) {
                return Err(error);
            }
            // A peer can write an incompatible response and exit before our
            // first write. Inspect that response under the same fixed deadline
            // rather than let scheduling turn a protocol failure into retryable
            // pipe loss. Empty/silent/valid responses retain the write failure.
            disconnected_write = Some(error);
        }
        magic(read).await?;
        let hello: Charged<Hello> = expect(read, budget, Kind::Accept).await?;
        if hello.transport_version != wire::VERSION || hello.open_host.is_some() {
            return Err(invalid("unsupported native transport acceptance"));
        }
        Ok(())
    })
    .await;
    match disconnected_write {
        Some(error)
            if !result
                .as_ref()
                .is_err_and(|reason| reason.kind() == io::ErrorKind::InvalidData) =>
        {
            Err(error)
        }
        _ => result,
    }
}

pub async fn server<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
) -> io::Result<()> {
    server_hello(read, write, budget, false).await.map(|_| ())
}

/// Only the peer-user-checked local owner endpoint may enable host opens.
/// It must compare owner_instance against its immutable service before SSH.
pub async fn server_main<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
) -> io::Result<Charged<Option<wire::OpenHost>>> {
    server_hello(read, write, budget, true).await
}

async fn server_hello<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
    allow_host: bool,
) -> io::Result<Charged<Option<wire::OpenHost>>> {
    deadline(async {
        magic(read).await?;
        let hello: Charged<Hello> = expect(read, budget, Kind::Hello).await?;
        if hello.transport_version != wire::VERSION {
            return Err(invalid("unsupported native transport version"));
        }
        if !allow_host && hello.open_host.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "host opens are unavailable on this assigned carrier",
            ));
        }
        write.write_all(wire::MAGIC).await?;
        write_control(
            write,
            budget,
            Kind::Accept,
            &Hello {
                transport_version: wire::VERSION,
                open_host: None,
            },
        )
        .await?;
        Ok(hello.map(|hello| hello.open_host))
    })
    .await
}

pub fn validate_ready(
    ready: &Ready,
    instance: Option<&str>,
    purpose: &Purpose,
    mux: u32,
) -> io::Result<()> {
    if ready.generation == 0
        || !ready.limits.valid()
        || ready.instance.len() != 32
        || !ready
            .instance
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid native service identity or limits"));
    }
    if instance.is_some_and(|expected| expected != ready.instance) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, StaleInstance));
    }
    if &ready.purpose != purpose {
        return Err(invalid(
            "native service purpose differs from assigned backend",
        ));
    }
    if ready.mux_protocol != mux {
        return Err(invalid("incompatible mux protocol"));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("native service does not match discovery")]
pub struct StaleInstance;

pub fn stale_instance(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<StaleInstance>())
}

/// A relay changes only the purpose assigned by its native owner. Retain the
/// metadata copy's reservation alongside it; an upstream Ready remains charged
/// by the upstream connection for its own lifetime.
pub fn host_ready(
    source: &Ready,
    target: &str,
    budget: &ConnectionBudget,
) -> io::Result<Charged<Ready>> {
    let bytes = std::mem::size_of::<Ready>()
        .checked_add(source.instance.len())
        .and_then(|bytes| bytes.checked_add(target.len()))
        .ok_or_else(|| invalid("native service metadata exceeds capacity"))?;
    budget
        .charge(bytes, true, || Ready {
            generation: source.generation,
            instance: source.instance.clone(),
            mux_protocol: source.mux_protocol,
            purpose: Purpose::Host {
                target: target.to_owned(),
            },
            limits: source.limits.clone(),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))
}

pub async fn offer<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
    ready: &Ready,
) -> io::Result<()> {
    validate_ready(ready, None, &ready.purpose, ready.mux_protocol)?;
    deadline(async {
        write_control(write, budget, Kind::Ready, ready).await?;
        let accept: Charged<ReadyAccept> = expect(read, budget, Kind::ReadyAccept).await?;
        if accept.generation != ready.generation || accept.instance != ready.instance {
            return Err(invalid("native service acknowledgement does not match"));
        }
        Ok(())
    })
    .await
}

/// The preparation deadline comes from the assigned connector, never the peer.
/// A progress flood cannot extend it or allocate an unbounded pending list.
pub async fn accept<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    read: &mut R,
    write: &mut W,
    budget: &ConnectionBudget,
    expected: Expected<'_>,
    mut progress: impl FnMut(Charged<wire::Progress>),
) -> io::Result<Charged<Ready>> {
    timeout(expected.preparation, async {
        loop {
            // Wait for a new preparation event, then bound the whole handshake
            // frame to five seconds (including header and payload).
            let mut first = [0; 1];
            read.read_exact(&mut first).await?;
            let mut prefixed = first.as_slice().chain(&mut *read);
            let frame = deadline(async {
                codec::read(&mut prefixed, budget)
                    .await?
                    .ok_or_else(|| invalid("EOF during service negotiation"))
            })
            .await?;
            match frame.value.kind {
                Kind::Progress => progress(frame.value.decode_control(budget)?),
                Kind::Ready => {
                    let ready: Charged<Ready> = frame.value.decode_control(budget)?;
                    // A hub resolves the host's mux version during setup. It
                    // accepts our version or its supported predecessor, while
                    // direct discovery still requires the exact reported one.
                    let mux = if matches!(expected.purpose, Purpose::Host { .. })
                        && ready.mux_protocol > 0
                        && ready.mux_protocol.checked_add(1) == Some(expected.mux_protocol)
                    {
                        ready.mux_protocol
                    } else {
                        expected.mux_protocol
                    };
                    validate_ready(&ready, expected.instance, expected.purpose, mux)?;
                    deadline(write_control(
                        write,
                        budget,
                        Kind::ReadyAccept,
                        &ReadyAccept {
                            generation: ready.generation,
                            instance: ready.instance.clone(),
                        },
                    ))
                    .await?;
                    return Ok(ready);
                }
                Kind::Closed => {
                    let closed: Charged<wire::Closed> = frame.value.decode_control(budget)?;
                    return Err(io::Error::new(
                        if closed.retryable {
                            io::ErrorKind::ConnectionAborted
                        } else {
                            io::ErrorKind::PermissionDenied
                        },
                        PreparationFailure(closed),
                    ));
                }
                _ => return Err(invalid("application frame before service admission")),
            }
        }
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "native backend preparation timed out",
        )
    })?
}

// Keep peer-supplied diagnostics counted for as long as the error retains them.
#[derive(Debug)]
struct PreparationFailure(Charged<wire::Closed>);
/// Return the actual structured preparation failure, if this error came from
/// a peer `Closed` frame. The error retains its byte charge while borrowed.
pub fn preparation_failure(error: &io::Error) -> Option<&wire::Closed> {
    error
        .get_ref()?
        .downcast_ref::<PreparationFailure>()
        .map(|failure| &failure.0.value)
}
impl std::fmt::Display for PreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message)
    }
}
impl std::error::Error for PreparationFailure {}

pub struct Expected<'a> {
    pub instance: Option<&'a str>,
    pub purpose: &'a Purpose,
    pub mux_protocol: u32,
    pub preparation: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, split};
    fn ready() -> Ready {
        Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 17,
            purpose: Purpose::Direct,
            limits: wire::Limits::default(),
        }
    }
    #[tokio::test]
    async fn control_parsers_admit_before_decoding_and_keep_retained_metadata_charged() {
        let budget = ConnectionBudget::default();
        let payload = format!(
            "{{\"transportVersion\":1,\"extra\":[{}0]}}",
            "0,".repeat(1500)
        )
        .into_bytes();
        let mut input = wire::MAGIC.to_vec();
        input.extend(
            Frame {
                kind: Kind::Hello,
                payload,
            }
            .encode()
            .unwrap(),
        );
        let mut output = Vec::new();
        assert_eq!(
            server_main(&mut input.as_slice(), &mut output, &budget)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert!(
            output.is_empty(),
            "no framing acceptance before parser admission"
        );
        assert_eq!(budget.used(), 0);

        let mut input = Frame::json(
            Kind::Progress,
            &wire::Progress {
                phase: "connecting".into(),
                message: "Preparing host".into(),
            },
        )
        .unwrap()
        .encode()
        .unwrap();
        input.extend(
            Frame::json(Kind::Ready, &ready())
                .unwrap()
                .encode()
                .unwrap(),
        );
        let mut reports = Vec::new();
        let accepted = accept(
            &mut input.as_slice(),
            &mut output,
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            |report| reports.push(report),
        )
        .await
        .unwrap();
        let retained = budget.control.used();
        assert!(retained > 0);
        assert_eq!(budget.data.used(), 0, "negotiation uses reserved controls");
        drop(reports);
        assert!(budget.control.used() > 0 && budget.control.used() < retained);
        drop(accepted);
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn queued_maximum_progress_and_retained_ready_leave_room_for_control_decoding() {
        let budget = ConnectionBudget::default();
        let mut progress = wire::Progress {
            phase: "connecting".into(),
            message: String::new(),
        };
        let empty = Frame::json(Kind::Progress, &progress)
            .unwrap()
            .payload
            .len();
        progress.message = "\\".repeat((Kind::Progress.payload_limit() - empty) / 2);
        let mut input = Frame::json(Kind::Progress, &progress)
            .unwrap()
            .encode()
            .unwrap();
        input.extend(
            Frame::json(Kind::Ready, &ready())
                .unwrap()
                .encode()
                .unwrap(),
        );
        let mut reports = Vec::new();
        let mut output = Vec::new();
        let accepted = accept(
            &mut input.as_slice(),
            &mut output,
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            |report| reports.push(report),
        )
        .await
        .unwrap();
        assert_eq!(reports[0].message, progress.message);
        assert!(
            budget.control.used() < 4096,
            "parser scratch is released after conversion"
        );
        let mut closed = wire::Closed {
            code: wire::CloseCode::BackendLost,
            message: String::new(),
            retryable: false,
        };
        let empty = Frame::json(Kind::Closed, &closed).unwrap().payload.len();
        closed.message = "\\".repeat((Kind::Closed.payload_limit() - empty) / 2);
        let input = Frame::json(Kind::Closed, &closed)
            .unwrap()
            .encode()
            .unwrap();
        let error = accept(
            &mut input.as_slice(),
            &mut output,
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), closed.message);
        assert!(budget.control.high_water() <= wire::CONNECTION_CONTROL_BYTES);
        drop((error, accepted, reports));
        assert_eq!(budget.used(), 0);
    }

    #[tokio::test]
    async fn host_open_is_explicit_and_rejected_by_direct_or_remote_carriers() {
        let intent = wire::OpenHost {
            owner_instance: "a".repeat(32),
            target: "me@192.0.2.10".into(),
            update: false,
        };
        let hello = Frame::json(
            Kind::Hello,
            &Hello {
                transport_version: 1,
                open_host: Some(intent.clone()),
            },
        )
        .unwrap();
        let mut bytes = wire::MAGIC.to_vec();
        bytes.extend(hello.encode().unwrap());
        let budget = ConnectionBudget::default();
        let mut output = Vec::new();
        assert_eq!(
            server(&mut bytes.as_slice(), &mut output, &budget)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            output.is_empty(),
            "an assigned remote carrier cannot even accept host intent"
        );
        assert_eq!(
            server_main(&mut bytes.as_slice(), &mut output, &budget)
                .await
                .unwrap(),
            Some(intent)
        );
        let mut response = output.as_slice();
        magic(&mut response).await.unwrap();
        let accepted: Charged<Hello> = expect(&mut response, &budget, Kind::Accept).await.unwrap();
        assert!(accepted.open_host.is_none());
        drop(accepted);
        assert_eq!(budget.used(), 0);
        assert!(
            serde_json::from_value::<Hello>(serde_json::json!({"transportVersion":1,"openHost": {
                "ownerInstance":"a".repeat(32),"target":"vm","update":false,"role":"main"
            }}))
            .is_err(),
            "intent does not carry caller-selected authority"
        );
    }

    #[tokio::test]
    async fn a_host_can_resolve_a_supported_previous_mux_without_weakening_direct_identity() {
        let budget = ConnectionBudget::default();
        let mut previous = ready();
        previous.mux_protocol = 16;
        previous.purpose = Purpose::Host {
            target: "vm".into(),
        };
        let bytes = Frame::json(Kind::Ready, &previous)
            .unwrap()
            .encode()
            .unwrap();
        let mut output = Vec::new();
        assert_eq!(
            accept(
                &mut bytes.as_slice(),
                &mut output,
                &budget,
                Expected {
                    instance: None,
                    purpose: &previous.purpose,
                    mux_protocol: 17,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                |_| {}
            )
            .await
            .unwrap(),
            previous
        );
        previous.purpose = Purpose::Direct;
        let bytes = Frame::json(Kind::Ready, &previous)
            .unwrap()
            .encode()
            .unwrap();
        assert!(accept(
            &mut bytes.as_slice(),
            &mut Vec::new(),
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
            |_| {}
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn preparation_failure_preserves_retryability_without_admitting_a_service() {
        for retryable in [false, true] {
            let frame = Frame::json(
                Kind::Closed,
                &wire::Closed {
                    code: wire::CloseCode::BackendLost,
                    retryable,
                    message: "setup failed".into(),
                },
            )
            .unwrap()
            .encode()
            .unwrap();
            let mut read = frame.as_slice();
            let mut write = Vec::new();
            let budget = ConnectionBudget::default();
            let error = accept(
                &mut read,
                &mut write,
                &budget,
                Expected {
                    instance: None,
                    purpose: &Purpose::Direct,
                    mux_protocol: 17,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
                |_| {},
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.kind(),
                if retryable {
                    io::ErrorKind::ConnectionAborted
                } else {
                    io::ErrorKind::PermissionDenied
                }
            );
            assert_eq!(error.to_string(), "setup failed");
            assert_eq!(
                preparation_failure(&error),
                Some(&wire::Closed {
                    code: wire::CloseCode::BackendLost,
                    retryable,
                    message: "setup failed".into(),
                })
            );
            assert!(budget.control.used() > 0);
            drop(error);
            assert!(write.is_empty());
            assert_eq!(budget.used(), 0);
        }
        assert!(preparation_failure(&io::Error::other("setup failed")).is_none());
    }
    #[tokio::test]
    async fn framing_then_preparation_then_verified_service() {
        let (a, b) = duplex(32);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);
        let task = tokio::spawn(async move {
            let budget = ConnectionBudget::default();
            server(&mut br, &mut bw, &budget).await.unwrap();
            codec::write(
                &mut bw,
                &Frame::json(
                    Kind::Progress,
                    &wire::Progress {
                        phase: "connecting".into(),
                        message: "Preparing host".into(),
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();
            offer(&mut br, &mut bw, &budget, &ready()).await.unwrap();
        });
        let budget = ConnectionBudget::default();
        client(&mut ar, &mut aw, &budget).await.unwrap();
        let mut phases = 0;
        let actual = accept(
            &mut ar,
            &mut aw,
            &budget,
            Expected {
                instance: Some(&"a".repeat(32)),
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: Duration::from_secs(30),
            },
            |_| phases += 1,
        )
        .await
        .unwrap();
        assert_eq!(actual, ready());
        assert_eq!(phases, 1);
        task.await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn incomplete_framing_and_ready_ack_each_have_a_fixed_deadline() {
        let (a, _b) = duplex(1024);
        let (mut read, mut write) = split(a);
        let budget = ConnectionBudget::default();
        assert_eq!(
            server(&mut read, &mut write, &budget)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            offer(&mut read, &mut write, &budget, &ready())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(budget.used(), 0);
    }
    #[tokio::test]
    async fn unsupported_hello_and_early_application_data_never_admit() {
        let budget = ConnectionBudget::default();
        let mut bytes = wire::MAGIC.to_vec();
        bytes.extend(
            Frame::json(
                Kind::Hello,
                &Hello {
                    transport_version: 2,
                    open_host: None,
                },
            )
            .unwrap()
            .encode()
            .unwrap(),
        );
        let mut response = Vec::new();
        assert!(server(&mut bytes.as_slice(), &mut response, &budget)
            .await
            .is_err());
        assert!(response.is_empty());
        let bytes = Frame::data(Kind::Text, wire::Delivery::new(1), br#"{"type":"create"}"#)
            .unwrap()
            .encode()
            .unwrap();
        assert!(
            offer(&mut bytes.as_slice(), &mut response, &budget, &ready())
                .await
                .is_err()
        );
    }
    #[test]
    fn service_instance_purpose_mux_and_ceilings_are_independently_verified() {
        let expected = ready();
        assert!(validate_ready(&expected, Some(&"b".repeat(32)), &Purpose::Direct, 17).is_err());
        assert!(validate_ready(
            &expected,
            None,
            &Purpose::Host {
                target: "elsewhere".into()
            },
            17
        )
        .is_err());
        assert!(validate_ready(&expected, None, &Purpose::Direct, 1).is_err());
        let mut raised = expected;
        raised.limits.frame_bytes += 1;
        assert!(validate_ready(&raised, None, &Purpose::Direct, 17).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn helper_preparation_has_its_own_deadline_and_partial_ready_gets_only_five_seconds() {
        let budget = ConnectionBudget::default();
        let (client, mut server) = duplex(1024);
        let (mut read, mut write) = split(client);
        let now = tokio::time::Instant::now();
        let error = accept(
            &mut read,
            &mut write,
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: Duration::from_secs(20),
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(now.elapsed(), Duration::from_secs(20));
        server.write_all(&[Kind::Ready as u8]).await.unwrap();
        let now = tokio::time::Instant::now();
        let error = accept(
            &mut read,
            &mut write,
            &budget,
            Expected {
                instance: None,
                purpose: &Purpose::Direct,
                mux_protocol: 17,
                preparation: Duration::from_secs(20),
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(now.elapsed(), wire::HANDSHAKE_TIMEOUT);
        assert_eq!(budget.used(), 0);
    }
    #[tokio::test(start_paused = true)]
    async fn incompatible_response_survives_simultaneous_broken_outbound_pipe() {
        struct BrokenWrite;
        impl tokio::io::AsyncWrite for BrokenWrite {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &[u8],
            ) -> std::task::Poll<io::Result<usize>> {
                std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "outbound closed",
                )))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }
        let budget = ConnectionBudget::default();
        let mut valid = wire::MAGIC.to_vec();
        valid.extend(
            Frame::json(
                Kind::Accept,
                &Hello {
                    transport_version: wire::VERSION,
                    open_host: None,
                },
            )
            .unwrap()
            .encode()
            .unwrap(),
        );
        let mut unsupported = wire::MAGIC.to_vec();
        unsupported.extend(
            Frame::json(
                Kind::Accept,
                &Hello {
                    transport_version: wire::VERSION + 1,
                    open_host: None,
                },
            )
            .unwrap()
            .encode()
            .unwrap(),
        );
        for (bytes, expected) in [
            (b"INVALID!".as_slice(), io::ErrorKind::InvalidData),
            (unsupported.as_slice(), io::ErrorKind::InvalidData),
            (b"".as_slice(), io::ErrorKind::BrokenPipe),
            (valid.as_slice(), io::ErrorKind::BrokenPipe),
        ] {
            let error = client(&mut &*bytes, &mut BrokenWrite, &budget)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), expected, "{error}");
        }
        let (mut silent, _peer) = duplex(64);
        let start = tokio::time::Instant::now();
        let error = client(&mut silent, &mut BrokenWrite, &budget)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(start.elapsed(), wire::HANDSHAKE_TIMEOUT);
        assert_eq!(budget.used(), 0);
    }
}
