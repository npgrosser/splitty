//! Fixed-backend relay validation; neither hop acquires local hub authority.
use splitty_transport::{
    codec::Frame,
    peer::Sender,
    wire::{self, Kind},
};
use std::io;

/// During preparation only framing controls may travel. An EOF cancels setup;
/// early application input is refused. Periodic controls also detect a broken
/// stdout pipe while SSH is still waiting for a host to answer.
pub(crate) async fn prepare<R, W, P>(
    read: &mut R,
    write: &mut W,
    context: &splitty_transport::queue::Context,
    future: impl std::future::Future<Output = io::Result<()>>,
    reports: &mut tokio::sync::mpsc::Receiver<P>,
    mut report: impl FnMut(P) -> io::Result<splitty_transport::budget::Charged<wire::Progress>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<Option<()>>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncReadExt;
    tokio::pin!(future, shutdown);
    let deadline = tokio::time::Instant::now() + splitty_remote::tunnel::PREPARE_TIMEOUT;
    let mut pulse = tokio::time::interval(std::time::Duration::from_secs(1));
    pulse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut latest = context
        .budget
        .display(&"Preparing the host connection...", true)?
        .map(|message| wire::Progress {
            phase: "connecting".into(),
            message,
        });
    // One bounded serialization buffer, charged before constructing it.
    let _progress_bytes = context
        .budget
        .reserve(wire::MAX_CONTROL_BYTES + wire::HEADER_BYTES, true)
        .map_err(io::Error::other)?;
    let mut closing = context.closing();
    macro_rules! emit_progress {
        () => {{
            let frame = Frame::json(Kind::Progress, &*latest)?;
            tokio::select! { biased;
                _ = &mut shutdown => return Ok(None),
                _ = async { let _ = closing.wait_for(|closed| *closed).await; } => return Ok(None),
                byte = read.read_u8() => return early(byte),
                result = tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + wire::HANDSHAKE_TIMEOUT),
                    splitty_transport::codec::write(write, &frame)) => {
                    result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "preparation progress delivery timed out"))??;
                }
            }
        }};
    }
    let mut completed = false;
    let result = async {
    loop {
        tokio::select! { biased;
            _ = &mut shutdown => return Ok(None),
            _ = async { let _ = closing.wait_for(|closed| *closed).await; } => return Ok(None),
            _ = tokio::time::sleep_until(deadline) => return Err(io::Error::new(io::ErrorKind::TimedOut, "backend preparation timed out")),
            byte = read.read_u8() => return early(byte),
            result = &mut future => {
                completed = true;
                while let Ok(next) = reports.try_recv() {
                    latest = report(next)?;
                    emit_progress!();
                }
                return result.map(Some);
            },
            next = reports.recv(), if !reports.is_closed() || !reports.is_empty() => {
                if let Some(next) = next { latest = report(next)?; }
            }
            _ = pulse.tick() => {},
        }
        emit_progress!();
    }
    }.await;
    if !completed {
        // Setup shares this signal. Let its own cancellation path close pipes
        // and reap children; dropping the future would only request cleanup.
        context.close();
        let _ = future.await;
    }
    result
}

fn early<T>(byte: io::Result<u8>) -> io::Result<Option<T>> {
    match byte {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "application input before service admission",
        )),
    }
}

pub(crate) fn forward(sender: &Sender, frame: &Frame) -> io::Result<()> {
    if frame.kind == Kind::Cancel {
        let (mut cancel, _decoded) = frame
            .decode_control::<wire::Cancel>(&sender.context().budget)?
            .into_parts();
        cancel.generation = sender.generation();
        sender.control(Kind::Cancel, &cancel)
    } else {
        // Charge the new buffer before releasing the received one. Receipts
        // and delivery credit belong to their own hop.
        sender.data(frame.kind, frame.application()?.1)
    }
}

pub(crate) fn validate_input(
    frame: &Frame,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<()> {
    use crate::service::catalog::{self, Role};
    match frame.kind {
        Kind::Text => catalog::validate_session(frame.application()?.1, budget)
            .map(|_| ())
            .map_err(io::Error::other),
        Kind::Request => {
            let (request, _decoded) = frame.decode_charged::<wire::Request>(budget)?.into_parts();
            let operation = serde_json::from_value(serde_json::Value::String(request.operation))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "unknown native operation")
                })?;
            catalog::Request {
                operation,
                args: request.args,
            }
            .validate(Role::Remote)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "native operation or arguments are not permitted",
                )
            })
        }
        Kind::Cancel => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported remote client frame",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn preparation_cancellation_waits_for_the_setup_cleanup_acknowledgment() {
        let context = splitty_transport::queue::Context::default();
        let budget = context.budget.clone();
        let (mut input, mut read) = tokio::io::duplex(1024);
        let (mut write, _output) = tokio::io::duplex(8192);
        let (_reports, mut reports) = tokio::sync::mpsc::channel::<wire::Progress>(1);
        let (started, cleaning) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let mut closing = context.closing();
        let future = async move {
            let _ = closing.wait_for(|closed| *closed).await;
            started.send(()).unwrap();
            finished.await.unwrap();
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "setup ended after cancellation",
            ))
        };
        let task = tokio::spawn(async move {
            prepare(
                &mut read,
                &mut write,
                &context,
                future,
                &mut reports,
                |report| {
                    context
                        .budget
                        .charge(4096, true, || report)
                        .map_err(io::Error::other)
                },
                std::future::pending(),
            )
            .await
        });
        input.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), cleaning)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        assert!(
            budget.used() > 0,
            "the preparation scope still owns its resources"
        );
        finish.send(()).unwrap();
        assert!(task.await.unwrap().unwrap().is_none());
        assert_eq!(budget.used(), 0);
    }
    struct Cancelled(Arc<AtomicBool>);
    impl Drop for Cancelled {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn eof_early_input_and_broken_output_cancel_backend_preparation() {
        for mode in ["eof", "early", "output"] {
            let (mut input, read) = tokio::io::duplex(1024);
            let (write, output) = tokio::io::duplex(8192);
            let mut read = tokio::io::split(read).0;
            let mut write = tokio::io::split(write).1;
            let (_reports, mut reports) = tokio::sync::mpsc::channel::<wire::Progress>(1);
            let dropped = Arc::new(AtomicBool::new(false));
            let guard = Cancelled(dropped.clone());
            let context = splitty_transport::queue::Context::default();
            let mut closing = context.closing();
            let future = async move {
                let _guard = guard;
                let _ = closing.wait_for(|closed| *closed).await;
                Ok(())
            };
            let mut output = Some(output);
            match mode {
                "eof" => input.shutdown().await.unwrap(),
                "early" => input.write_all(&[wire::Kind::Text as u8]).await.unwrap(),
                "output" => {
                    output.take();
                }
                _ => unreachable!(),
            }
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                prepare(
                    &mut read,
                    &mut write,
                    &context,
                    future,
                    &mut reports,
                    |report| {
                        context
                            .budget
                            .charge(4096, true, || report)
                            .map_err(io::Error::other)
                    },
                    std::future::pending(),
                ),
            )
            .await
            .unwrap();
            if mode == "eof" {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
            assert!(
                dropped.load(Ordering::SeqCst),
                "setup remains live after {mode}"
            );
        }
    }
}
