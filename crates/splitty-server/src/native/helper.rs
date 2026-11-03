//! Owned desktop child backend. Standard output is exclusively native frames;
//! the inherited pipe lifetime owns the embedded workspace and its programs.
use splitty_transport::{admission, negotiation, queue};
use std::{io, sync::Arc};

pub mod owner;

/// A standalone helper must let cancelled setup/bridge reapers observe exit
/// before its composition root uses process::exit to end blocking stdio reads.
pub async fn settle_workers() -> io::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while admission::Admission::global().counts().workers != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "native worker exit could not be confirmed",
        )
    })
}

/// Own only an SSH attachment. Losing these pipes never stops the host daemon
/// or its keepers. The shell broker opens a fresh helper/generation on retry.
pub async fn ssh<R, W>(
    mut read: R,
    mut write: W,
    target: splitty_remote::ssh::SshTarget,
    remember_target: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use splitty_remote::{install, ssh::Ssh, tunnel};
    use splitty_transport::{peer::Peer, wire::Kind};
    let admission = admission::Admission::global();
    let _carrier = admission.carrier().map_err(super::busy)?;
    let assignment =
        admission::Assignment::new(admission::Backend::new(), admission::Domain::new());
    let opening = admission.begin(assignment.clone()).map_err(super::busy)?;
    let context = queue::Context::default();
    negotiation::server(&mut read, &mut write, &context.budget).await?;
    tokio::pin!(shutdown);
    let (reports, mut progress) = tokio::sync::mpsc::channel(8);
    // The helper fixes the host before preparation. The host's direct daemon
    // identity remains upstream; the desktop sees its assigned host purpose.
    let mut prepared = None;
    let prepare = async {
        prepared = Some(
            tunnel::open(
                Ssh::stdio_only(target.clone()).with_context(context.clone()),
                env!("CARGO_PKG_VERSION"),
                splitty_core::protocol::PROTOCOL_VERSION,
                install::payload_for,
                assignment,
                |report| {
                    let _ = reports.try_send(report);
                },
            )
            .await
            .map_err(io::Error::other)?,
        );
        Ok(())
    };
    let connected = super::relay::prepare(
        &mut read,
        &mut write,
        &context,
        prepare,
        &mut progress,
        |report| {
            Ok(report.map(|report| {
                let (phase, message) = match report {
                    tunnel::Progress::Step { step, message } => {
                        (format!("{step:?}").to_lowercase(), message)
                    }
                    tunnel::Progress::Notice { message } => ("notice".into(), message),
                };
                splitty_transport::wire::Progress { phase, message }
            }))
        },
        &mut shutdown,
    )
    .await;
    if !matches!(connected, Ok(Some(()))) {
        if let Some((_remote, connected)) = prepared.take() {
            let _ = connected.client.finish().await;
        }
    }
    let connected = match connected {
        Ok(connected) => connected,
        Err(error) => {
            let remote = error
                .get_ref()
                .and_then(|source| source.downcast_ref::<tunnel::RemoteError>());
            let retryable = remote.map_or_else(
                || {
                    matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                    )
                },
                tunnel::RemoteError::retryable,
            );
            let failure = remote.map_or_else(
                || tunnel::Failure::from_error_budgeted(&error, &context.budget),
                |error| error.failure_budgeted(&context.budget),
            )?;
            let frame = failure_frame(failure, retryable, &context.budget)?;
            use tokio::io::AsyncReadExt;
            tokio::select! { biased;
                _ = &mut shutdown => {},
                _ = read.read_u8() => {},
                _ = tokio::time::timeout(splitty_transport::wire::HANDSHAKE_TIMEOUT,
                    splitty_transport::codec::write(&mut write, &frame)) => {},
            }
            return Err(error);
        }
    };
    let Some(()) = connected else {
        return Ok(());
    };
    let (_remote, connected) = prepared.expect("completed SSH preparation");
    let mut bridge = connected.client;
    let mut client = None;
    let mut stream = None;
    let result = async {
    let ready = negotiation::host_ready(bridge.ready(), target.as_str(), &context.budget)?;
    negotiation::offer(&mut read, &mut write, &context.budget, &ready).await?;
    stream = Some(opening.admit().map_err(super::busy)?);
    // Only an explicit native open may reveal a forgotten host. Background
    // recovery can refresh visible metadata but must preserve a removal.
    let config = splitty_core::config::config_dir();
    let recorded = if remember_target {
        splitty_remote::hosts::record_recent(&config, &target)
    } else {
        splitty_remote::hosts::record_recent_if_visible(&config, &target)
    };
    if let Err(error) = recorded { tracing::warn!(%error, "could not record recent SSH host"); }
    let client = client.insert(Peer::start(read, write, &ready, context));
    loop {
        tokio::select! { biased;
            _ = &mut shutdown => break Ok(()),
            incoming = client.recv() => {
                let Some(message) = incoming else { break Ok(()); };
                if let Err(error) = super::relay::validate_input(&message.frame, &client.sender.context().budget)
                    .and_then(|()| super::relay::forward(&bridge.sender(), &message.frame)) { break Err(error); }
            }
            outgoing = bridge.recv() => {
                let Some(message) = outgoing else { break Err(io::Error::new(io::ErrorKind::ConnectionAborted, "SSH backend connection lost")); };
                if !matches!(message.frame.kind, Kind::Text | Kind::Binary | Kind::Reply) {
                    break Err(super::invalid("unexpected host reply"));
                }
                if let Err(error) = super::relay::forward(&client.sender, &message.frame) { break Err(error); }
            }
        }
    }
    }.await;
    let cleanup = async {
        match client {
            Some(client) => client.finish().await,
            None => Ok(()),
        }
    };
    let (client_result, (bridge_result, _ended)) = tokio::join!(cleanup, bridge.finish());
    drop(stream);
    result.and(client_result).and(bridge_result)
}

/// Preserve structured SSH hints without allowing diagnostics to exceed the
/// control budget or truncating the JSON envelope into an undecodable string.
fn failure_frame(
    mut failure: impl std::borrow::BorrowMut<splitty_remote::tunnel::Failure>,
    retryable: bool,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<splitty_transport::budget::Charged<splitty_transport::codec::Frame>> {
    use splitty_transport::codec;
    let failure = failure.borrow_mut();
    #[derive(serde::Serialize)]
    struct Diagnostic<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        failure: &'a splitty_remote::tunnel::Failure,
    }
    loop {
        let diagnostic = Diagnostic {
            kind: "remote.error",
            failure,
        };
        match codec::closed_diagnostic(&diagnostic, retryable, budget) {
            Ok(frame) => return Ok(frame),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {}
            Err(error) => return Err(error),
        }
        let value = match failure.hint.as_mut() {
            Some(hint) if hint.len() > failure.message.len() => hint,
            _ => &mut failure.message,
        };
        if value.is_empty() {
            return Err(super::invalid("SSH diagnostic exceeds control limit"));
        }
        let mut end = value.len() / 2;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
}

pub async fn embedded<R, W>(
    mut read: R,
    mut write: W,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let admission = admission::Admission::global();
    let carrier = admission.carrier().map_err(super::busy)?;
    let assignment = super::Assignment::default();
    let opening = admission
        .begin(admission::Assignment::new(
            admission::Backend::new(),
            assignment.domain.clone(),
        ))
        .map_err(super::busy)?;
    let context = queue::Context::default();
    // Framing first, including the five-second deadline. Legacy bootstrap lines
    // and invalid peers cannot create a workspace or touch daemon discovery.
    negotiation::server(&mut read, &mut write, &context.budget).await?;
    #[cfg(unix)]
    let control_path = crate::control::socket_path();
    #[cfg(not(unix))]
    let control_path = std::path::PathBuf::new();
    let runtime = crate::service::runtime::build(
        "embedded",
        None,
        Arc::new(splitty_core::pty::terminal::LocalTerminals),
        &control_path,
    );
    #[cfg(unix)]
    let control = crate::control::spawn_service(runtime.state.clone(), control_path);
    let mut closed = context.closing();
    let result = tokio::select! {
        result = super::serve_ready(read, write, runtime.state.clone(), assignment, context, opening, carrier) => result,
        // Release watch's read guard before select drops the serving future:
        // Peer/Connection drop closes this same context with a write lock.
        _ = async { let _ = closed.wait_for(|closed| *closed).await; } => Ok(()),
        _ = shutdown => Ok(()),
    };
    // Cleanup is awaited even after a broken output pipe or failed ReadyAccept.
    let cleanup = runtime.close().await;
    #[cfg(unix)]
    control.shutdown();
    cleanup.and(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_full_control_budget_rejects_failure_without_retrying_smaller_diagnostics() {
        use splitty_transport::{budget::ConnectionBudget, wire};
        let budget = ConnectionBudget::default();
        let held = budget
            .reserve(wire::CONNECTION_CONTROL_BYTES, true)
            .unwrap();
        let failure = splitty_remote::tunnel::Failure {
            message: "Host key verification failed".into(),
            hint: Some("Check the fingerprint".into()),
        };
        assert_eq!(
            super::failure_frame(failure, false, &budget)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::OutOfMemory
        );
        assert_eq!(budget.used(), wire::CONNECTION_CONTROL_BYTES);
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn ssh_failure_preserves_hint_and_valid_json_under_control_limit() {
        for message in [
            "Host key verification failed".into(),
            "\"\\\n界".repeat(50_000),
        ] {
            let frame = super::failure_frame(
                splitty_remote::tunnel::Failure {
                    message,
                    hint: Some("Run `ssh vm1` once and check the fingerprint".into()),
                },
                false,
                &Default::default(),
            )
            .unwrap();
            assert!(frame.payload.len() <= splitty_transport::wire::MAX_CONTROL_BYTES);
            let closed: splitty_transport::wire::Closed =
                serde_json::from_slice(&frame.payload).unwrap();
            assert!(!closed.retryable);
            let diagnostic: serde_json::Value = serde_json::from_str(&closed.message).unwrap();
            assert_eq!(diagnostic["type"], "remote.error");
            assert_eq!(
                diagnostic["hint"],
                "Run `ssh vm1` once and check the fingerprint"
            );
        }
    }
}
