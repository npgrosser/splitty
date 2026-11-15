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
    let control = crate::control::spawn_at(runtime.state.clone(), control_path);
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
