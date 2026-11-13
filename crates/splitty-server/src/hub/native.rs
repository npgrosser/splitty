//! A native host stream is assigned by its main-window broker. No peer frame
//! can change its host, access a local service or open another SSH connection.
use super::*;
use splitty_transport::{negotiation, peer::Peer, queue};

impl Hub {
    pub async fn serve_native<R, W>(
        &self,
        mut read: R,
        mut write: W,
        target: SshTarget,
        update: bool,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> io::Result<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let carrier = admission::Admission::global()
            .carrier()
            .map_err(io::Error::other)?;
        let context = queue::Context::default();
        negotiation::server(&mut read, &mut write, &context.budget).await?;
        self.serve_native_accepted(
            (read, write),
            target,
            update,
            shutdown,
            (context, carrier, None),
        )
        .await
    }

    /// A main-service broker has already verified framing and the local
    /// service instance. Its capacity remains owned through host preparation.
    pub(crate) async fn serve_native_accepted<R, W>(
        &self,
        (mut read, mut write): (R, W),
        target: SshTarget,
        update: bool,
        shutdown: impl std::future::Future<Output = ()>,
        (context, _carrier, opening): (
            queue::Context,
            admission::Carrier,
            Option<admission::Opening>,
        ),
    ) -> io::Result<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (reports, mut progress) = mpsc::channel(8);
        let host = target.as_str().to_owned();
        // Keep a completed backend outside the progress writer: a late EOF or
        // failed progress write must still await that backend's cleanup.
        let mut reached = None;
        let prepare = async {
            reached = Some(self.reach(target, update, reports, &context).await);
            Ok(())
        };
        let mut prelude = Vec::new();
        tokio::pin!(shutdown);
        let result = crate::native::relay::prepare(
            &mut read,
            &mut write,
            &context,
            prepare,
            &mut progress,
            |report: Charged<Progress>| {
                let control = context
                    .budget
                    .display(&report.message, true)?
                    .map(|message| wire::Progress {
                        phase: format!("{:?}", report.step).to_lowercase(),
                        message,
                    });
                // At most install/daemon/update/started/bridge, all locally built.
                if prelude.len() < 8 {
                    prelude.push(report);
                }
                Ok(control)
            },
            &mut shutdown,
        )
        .await;
        if !matches!(result, Ok(Some(()))) {
            if let Some(Ok(reached)) = reached {
                let _ = reached.bridge.finish().await;
            }
            return result.map(|_| ());
        }
        let reached = match reached.expect("completed native preparation") {
            Ok(reached) => reached,
            Err(error) => {
                let frame = failure_frame(
                    error.failure_budgeted(&host, &context.budget)?,
                    &context.budget,
                )?;
                tokio::time::timeout(
                    wire::HANDSHAKE_TIMEOUT,
                    splitty_transport::codec::write(&mut write, &frame),
                )
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "host failure delivery timed out")
                })??;
                return Err(io::Error::other(error));
            }
        };
        let Reached {
            mut bridge,
            first,
            daemon,
            update,
            screens,
            target,
            _metadata,
            _authentication,
        } = reached;
        let mut client = None;
        let mut stream = None;
        let result = async {
        let ready = negotiation::host_ready(bridge.ready(), &host, &context.budget)?;
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            result = negotiation::offer(&mut read, &mut write, &context.budget, &ready) => result?,
        }
        stream = opening
            .map(admission::Opening::admit)
            .transpose()
            .map_err(io::Error::other)?;
        let client = client.insert(Peer::start(read, write, &ready, context));
        for progress in prelude {
            client.sender.json(
                Kind::Text,
                &HubMessage::Progress {
                    step: progress.step,
                    message: &progress.message,
                },
            )?;
        }
        client.sender.json(
            Kind::Text,
            &HubMessage::Ready {
                target: target.as_str(),
                daemon: ReadyDaemon {
                    version: &daemon.version,
                    protocol: daemon.protocol,
                    keeps_sessions: daemon.keeps_sessions,
                    started: daemon.started,
                },
                update,
                screens: &screens,
            },
        )?;
        crate::native::relay::forward(&client.sender, &first.frame)?;
        drop(first);
        if let Err(error) = hosts::record_recent_if_visible(&self.config_dir, &target) {
            tracing::warn!(%error, "could not remember the connected host");
        }
        self.observe_handshake(target.as_str(), &screens);
        self.forwards.restore(target.as_str());
        self.reverses.restore(target.as_str());
        let mut ports = self.auto_forwards.watch(target.as_str());
        loop {
            tokio::select! { biased;
                _ = &mut shutdown => break Ok(()),
                incoming = client.recv() => {
                    let Some(message) = incoming else { break Ok(()); };
                    if let Err(error) = crate::native::relay::validate_input(&message.frame, &client.sender.context().budget)
                        .and_then(|()| crate::native::relay::forward(&bridge.sender(), &message.frame)) { break Err(error); }
                }
                outgoing = bridge.recv() => {
                    let Some(message) = outgoing else { break Err(io::Error::new(io::ErrorKind::ConnectionAborted, "host stream ended")); };
                    if !matches!(message.frame.kind, Kind::Text | Kind::Binary | Kind::Reply) {
                        break Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected host event"));
                    }
                    self.observe_ports(&mut ports, &message.frame);
                    if let Err(error) = crate::native::relay::forward(&client.sender, &message.frame) { break Err(error); }
                }
            }

        }
        }.await;
        let client_cleanup = async {
            match client {
                Some(client) => client.finish().await,
                None => Ok(()),
            }
        };
        let (client_result, (bridge_result, _ended)) =
            tokio::join!(client_cleanup, bridge.finish());
        drop(stream);
        result.and(client_result).and(bridge_result)
    }
}

/// A diagnostic stays a valid remote.error within the control-frame ceiling.
/// Large screen lists are omitted as unknown rather than reporting a partial
/// count; session preservation and the typed failure code remain explicit.
fn failure_frame(
    mut failure: impl std::borrow::BorrowMut<Failure>,
    budget: &splitty_transport::budget::ConnectionBudget,
) -> io::Result<splitty_transport::budget::Charged<splitty_transport::codec::Frame>> {
    let failure = failure.borrow_mut();
    let retryable = failure.retryable;
    loop {
        let message = HubMessage::Error(failure);
        match splitty_transport::codec::closed_diagnostic(&message, retryable, budget) {
            Ok(frame) => return Ok(frame),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {}
            Err(error) => return Err(error),
        }
        if failure.screens.take().is_some() {
            continue;
        }
        let mut absent_hint = String::new();
        let mut absent_version = String::new();
        let longest = [
            &mut failure.message,
            failure.hint.as_mut().unwrap_or(&mut absent_hint),
            failure.version.as_mut().unwrap_or(&mut absent_version),
        ];
        // Reduce only diagnostic strings; keep code/protocol/keepsSessions.
        let value = longest
            .into_iter()
            .max_by_key(|value| value.len())
            .expect("message field");
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native failure envelope exceeds control limit",
            ));
        }
        let mut end = value.len() / 2;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::testing::ScriptedHost;
    use serde_json::{json, Value};

    fn legacy_ready() -> splitty_remote::handshake::LegacyMigration {
        splitty_remote::handshake::LegacyMigration::Ready {
            token: "a".repeat(64),
            sessions: 3,
        }
    }

    #[tokio::test]
    async fn explicit_update_rechecks_policy_and_confirms_a_fresh_daemon() {
        let dir = tempfile::tempdir().unwrap();
        for (version, keeps, replacement, stops, expected_stops, success) in [
            ("0.0.1", true, VERSION, true, 1, true),
            (VERSION, true, VERSION, true, 0, true),
            ("99.0.0", true, VERSION, true, 0, false),
            ("unparseable", true, VERSION, true, 0, false),
            ("0.0.1", false, VERSION, true, 0, false),
            ("0.0.1", true, "0.0.1", true, 0, false),
            ("0.0.1", true, VERSION, false, 1, false),
        ] {
            let host = ScriptedHost::new(dir.path()).with_server_version(replacement);
            let host = if keeps {
                host.with_keeping_daemon(version, PROTOCOL_VERSION)
            } else {
                host.with_daemon(version, PROTOCOL_VERSION)
            };
            let host = if stops { host } else { host.not_stopping() };
            let hub = Hub::new(Box::new(host.clone()), dir.path().join("config"), None);
            let (reports, _received) = tokio::sync::mpsc::channel(16);
            let result = hub
                .reach(
                    SshTarget::parse("vm").unwrap(),
                    true,
                    reports,
                    &Default::default(),
                )
                .await;
            assert_eq!(
                result.is_ok(),
                success,
                "{version}/{keeps}/{replacement}/{stops}"
            );
            let calls = host.asked();
            assert_eq!(
                calls.iter().filter(|call| *call == "stop").count(),
                expected_stops
            );
            assert!(!calls.iter().any(|call| call == "migrate"));
            if let Ok(reached) = result {
                assert_eq!(
                    reached.daemon.version,
                    if expected_stops == 1 {
                        VERSION
                    } else {
                        version
                    }
                );
                assert_eq!(calls.last().unwrap(), "bridge");
                let _ = reached.bridge.finish().await;
            } else {
                assert!(!calls.iter().any(|call| call == "bridge"));
            }
        }
    }

    #[tokio::test]
    async fn legacy_connect_only_offers_migration_and_explicit_update_hands_over_once() {
        let dir = tempfile::tempdir().unwrap();
        let host = ScriptedHost::new(dir.path()).with_legacy_daemon(Some(legacy_ready()));
        let hub = Hub::new(Box::new(host.clone()), dir.path().join("config"), None);
        let context = splitty_transport::queue::Context::default();
        let (reports, received) = tokio::sync::mpsc::channel(8);
        let error = hub
            .reach(SshTarget::parse("vm").unwrap(), false, reports, &context)
            .await
            .err()
            .unwrap();
        let failure = error.failure_budgeted("vm", &context.budget).unwrap();
        drop(error);
        drop(received);
        assert!(context.budget.used() > 0, "copied offer remains charged");
        let frame = failure_frame(failure, &context.budget).unwrap();
        let closed: wire::Closed = frame.decode().unwrap();
        assert!(!closed.retryable);
        let diagnostic: Value = serde_json::from_str(&closed.message).unwrap();
        assert_eq!(diagnostic["migration"]["available"], true);
        assert_eq!(diagnostic["migration"]["sessions"], 3);
        assert_eq!(diagnostic["migration"]["fromVersion"], "0.1.27");
        assert_eq!(diagnostic["keepsSessions"], true);
        assert!(!closed.message.contains(&"a".repeat(64)));
        assert_eq!(host.asked(), ["prepare vm", "handshake"]);
        drop(frame);
        assert_eq!(context.budget.used(), 0);
        let (reports, _received) = tokio::sync::mpsc::channel(8);
        let reached = hub
            .reach(SshTarget::parse("vm").unwrap(), true, reports, &context)
            .await
            .unwrap();
        assert!(reached.daemon.supports_native());
        assert_eq!(reached.daemon.version, VERSION);
        assert_eq!(
            host.asked(),
            [
                "prepare vm",
                "handshake",
                "prepare vm",
                "handshake",
                "migrate",
                "handshake",
                "bridge"
            ]
        );
        let _ = reached.bridge.finish().await;
    }

    #[tokio::test]
    async fn blocked_or_ambiguous_legacy_update_has_no_update_offer_and_no_automatic_retry() {
        let dir = tempfile::tempdir().unwrap();
        for (host, mutations) in [
            (ScriptedHost::new(dir.path()).with_legacy_daemon(None), 0),
            (
                ScriptedHost::new(dir.path()).with_legacy_daemon(Some(
                    splitty_remote::handshake::LegacyMigration::Blocked {
                        message: "private owner proof unavailable".into(),
                    },
                )),
                0,
            ),
            (
                ScriptedHost::new(dir.path())
                    .with_legacy_daemon(Some(legacy_ready()))
                    .with_server_version("0.1.29"),
                0,
            ),
            (
                ScriptedHost::new(dir.path())
                    .with_legacy_daemon(Some(legacy_ready()))
                    .not_stopping(),
                1,
            ),
        ] {
            let hub = Hub::new(Box::new(host.clone()), dir.path().join("config"), None);
            let (reports, _received) = tokio::sync::mpsc::channel(8);
            let error = hub
                .reach(
                    SshTarget::parse("vm").unwrap(),
                    true,
                    reports,
                    &Default::default(),
                )
                .await
                .err()
                .unwrap();
            let failure = error.failure("vm");
            assert_eq!(failure.code, Code::MigrationUnavailable);
            assert!(failure.migration.is_none());
            let closed: wire::Closed = failure_frame(failure, &Default::default())
                .unwrap()
                .decode()
                .unwrap();
            assert!(!closed.retryable);
            let asked = host.asked();
            assert_eq!(
                asked.iter().filter(|call| *call == "migrate").count(),
                mutations
            );
            assert!(!asked.iter().any(|call| call == "stop" || call == "bridge"));
        }
    }

    #[test]
    fn exhausted_control_capacity_keeps_failure_delivery_refused() {
        let budget = splitty_transport::budget::ConnectionBudget::default();
        let held = budget
            .reserve(wire::CONNECTION_CONTROL_BYTES, true)
            .unwrap();
        let failure = HubError::NativeTransport { host: "vm".into() }.failure("vm");
        assert_eq!(
            failure_frame(failure, &budget).unwrap_err().kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(budget.used(), wire::CONNECTION_CONTROL_BYTES);
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn oversized_native_errors_preserve_types_and_mark_screen_counts_unknown() {
        let failure = Failure {
            retryable: false,
            update: None,
            migration: None,
            code: Code::Incompatible,
            message: "\u{0001}🙂".repeat(8192),
            hint: Some("更新🙂".repeat(8192)),
            version: Some("9".repeat(8192)),
            protocol: Some(3),
            keeps_sessions: Some(false),
            screens: Some(
                (0..200)
                    .map(|id| Screen {
                        id: id.to_string(),
                        name: Some("x".repeat(1024)),
                        panes: 2,
                    })
                    .collect(),
            ),
        };
        let frame = failure_frame(failure, &Default::default()).unwrap();
        assert!(frame.size() <= wire::MAX_CONTROL_BYTES);
        let closed: wire::Closed = frame.decode().unwrap();
        assert!(!closed.retryable);
        let error: Value = serde_json::from_str(&closed.message).unwrap();
        assert_eq!(error["type"], "remote.error");
        assert_eq!(error["code"], "incompatible");
        assert_eq!(error["protocol"], 3);
        assert_eq!(error["keepsSessions"], false);
        assert!(
            error.get("screens").is_none(),
            "never present a partial list as the sessions at risk"
        );
        let auth = HubError::NativeTransport { host: "vm".into() }.failure("vm");
        let closed: wire::Closed = failure_frame(auth, &Default::default())
            .unwrap()
            .decode()
            .unwrap();
        assert!(!closed.retryable);
    }

    #[tokio::test]
    async fn native_setup_failures_keep_their_error_code_and_never_admit_application_input() {
        use crate::hub::testing::Prepare;
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            (
                ScriptedHost::new(dir.path())
                    .preparing(Prepare::SshFails("Host key verification failed.")),
                false,
                "host_key",
                false,
            ),
            (
                ScriptedHost::new(dir.path())
                    .preparing(Prepare::SshFails("Permission denied (publickey).")),
                false,
                "auth",
                false,
            ),
            (
                ScriptedHost::new(dir.path()).preparing(Prepare::SshFails("Connection refused")),
                false,
                "unreachable",
                true,
            ),
            (
                ScriptedHost::new(dir.path()).with_daemon("0.0.1", 9),
                false,
                "incompatible",
                false,
            ),
            (
                ScriptedHost::new(dir.path()).with_daemon("0.0.1", 1),
                true,
                "incompatible",
                false,
            ),
        ];
        for (host, update, code, retryable) in cases {
            let hub = Arc::new(Hub::new(
                Box::new(host.clone()),
                dir.path().join("config"),
                None,
            ));
            let (client, server) = tokio::io::duplex(8192);
            let (read, write) = tokio::io::split(server);
            let task = tokio::spawn(async move {
                hub.serve_native(
                    read,
                    write,
                    SshTarget::parse("vm").unwrap(),
                    update,
                    std::future::pending(),
                )
                .await
            });
            let (mut read, mut write) = tokio::io::split(client);
            let budget = ConnectionBudget::default();
            negotiation::client(&mut read, &mut write, &budget)
                .await
                .unwrap();
            let error = negotiation::accept(
                &mut read,
                &mut write,
                &budget,
                negotiation::Expected {
                    instance: None,
                    purpose: &wire::Purpose::Host {
                        target: "vm".into(),
                    },
                    mux_protocol: PROTOCOL_VERSION,
                    preparation: Duration::from_secs(3),
                },
                |_| {},
            )
            .await
            .unwrap_err();
            let failure: Value = serde_json::from_str(&error.to_string()).unwrap();
            assert_eq!(failure["type"], "remote.error");
            assert_eq!(failure["code"], code);
            assert_eq!(
                error.kind(),
                if retryable {
                    io::ErrorKind::ConnectionAborted
                } else {
                    io::ErrorKind::PermissionDenied
                }
            );
            assert!(task.await.unwrap().is_err());
            assert!(
                !host
                    .asked()
                    .iter()
                    .any(|call| call == "bridge" || call == "stop"),
                "failed setup cannot bridge or replace an unsafe daemon: {:?}",
                host.asked()
            );
        }
    }

    #[tokio::test]
    async fn bridge_hop_authentication_and_protocol_failures_are_permanent() {
        let dir = tempfile::tempdir().unwrap();
        for (host, code) in [
            (
                ScriptedHost::new(dir.path())
                    .with_bridge("echo 'Host key verification failed.' >&2; exit 255"),
                "host_key",
            ),
            (
                ScriptedHost::new(dir.path())
                    .with_bridge("echo 'Permission denied (publickey).' >&2; exit 255"),
                "auth",
            ),
            (
                ScriptedHost::new(dir.path()).with_bridge("printf INVALID!; exit 255"),
                "incompatible",
            ),
            (
                ScriptedHost::new(dir.path()).with_welcome(r#"{"type":"remote.ready"}"#),
                "incompatible",
            ),
            (
                ScriptedHost::new(dir.path()).with_welcome("__INVALID_FRAME_EXIT_255__"),
                "incompatible",
            ),
        ] {
            let hub = Arc::new(Hub::new(
                Box::new(host.clone()),
                dir.path().join("config"),
                None,
            ));
            let (client, server) = tokio::io::duplex(8192);
            let (read, write) = tokio::io::split(server);
            let task = tokio::spawn(async move {
                hub.serve_native(
                    read,
                    write,
                    SshTarget::parse("vm").unwrap(),
                    false,
                    std::future::pending(),
                )
                .await
            });
            let (mut read, mut write) = tokio::io::split(client);
            let budget = ConnectionBudget::default();
            negotiation::client(&mut read, &mut write, &budget)
                .await
                .unwrap();
            let error = negotiation::accept(
                &mut read,
                &mut write,
                &budget,
                negotiation::Expected {
                    instance: None,
                    purpose: &wire::Purpose::Host {
                        target: "vm".into(),
                    },
                    mux_protocol: PROTOCOL_VERSION,
                    preparation: Duration::from_secs(3),
                },
                |_| {},
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.kind(),
                io::ErrorKind::PermissionDenied,
                "{code}: {error}"
            );
            let failure: Value = serde_json::from_str(&error.to_string()).unwrap();
            assert_eq!(failure["code"], code);
            assert!(task.await.unwrap().is_err());
            assert!(host.asked().iter().any(|call| call == "bridge"));
        }
        let stale = HubError::NativeBridge(io::Error::new(
            io::ErrorKind::InvalidData,
            negotiation::StaleInstance,
        ));
        let frame = failure_frame(stale.failure("vm"), &Default::default()).unwrap();
        assert!(frame.decode::<wire::Closed>().unwrap().retryable);
    }

    async fn open(hub: Arc<Hub>, target: &str) -> (Peer, tokio::task::JoinHandle<io::Result<()>>) {
        let (client, server) = tokio::io::duplex(128 * 1024);
        let (read, write) = tokio::io::split(server);
        let target = SshTarget::parse(target).unwrap();
        let purpose = wire::Purpose::Host {
            target: target.as_str().into(),
        };
        let task = tokio::spawn(async move {
            hub.serve_native(read, write, target, false, std::future::pending())
                .await
        });
        let (mut read, mut write) = tokio::io::split(client);
        let context = queue::Context::default();
        negotiation::client(&mut read, &mut write, &context.budget)
            .await
            .unwrap();
        let ready = negotiation::accept(
            &mut read,
            &mut write,
            &context.budget,
            negotiation::Expected {
                instance: Some("0123456789abcdef0123456789abcdef"),
                purpose: &purpose,
                mux_protocol: PROTOCOL_VERSION,
                preparation: Duration::from_secs(10),
            },
            |_| {},
        )
        .await
        .unwrap();
        (Peer::start(read, write, &ready, context), task)
    }

    #[tokio::test]
    async fn failed_progress_and_ready_delivery_wait_for_the_acquired_bridge() {
        use std::{
            pin::Pin,
            task::{Context, Poll},
        };
        use tokio::io::AsyncWrite;
        struct FailLate<W> {
            write: W,
            host: ScriptedHost,
            enabled: bool,
        }
        impl<W: AsyncWrite + Unpin> AsyncWrite for FailLate<W> {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                bytes: &[u8],
            ) -> Poll<io::Result<usize>> {
                if self.enabled && self.host.bridge_pid().is_some() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "late progress refused",
                    )));
                }
                Pin::new(&mut self.write).poll_write(cx, bytes)
            }
            fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Pin::new(&mut self.write).poll_flush(cx)
            }
            fn poll_shutdown(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<io::Result<()>> {
                Pin::new(&mut self.write).poll_shutdown(cx)
            }
        }
        for late_progress in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let host = ScriptedHost::new(dir.path()).with_welcome("__WELCOME_THEN_IGNORE_EOF__");
            let hub = Arc::new(Hub::new(
                Box::new(host.clone()),
                dir.path().join("config"),
                None,
            ));
            let (client, server) = tokio::io::duplex(8192);
            let (read, write) = tokio::io::split(server);
            let write = FailLate {
                write,
                host: host.clone(),
                enabled: late_progress,
            };
            let task = tokio::spawn(async move {
                hub.serve_native(
                    read,
                    write,
                    SshTarget::parse("vm").unwrap(),
                    false,
                    std::future::pending(),
                )
                .await
            });
            let (mut read, mut write) = tokio::io::split(client);
            let budget = ConnectionBudget::default();
            negotiation::client(&mut read, &mut write, &budget)
                .await
                .unwrap();
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    splitty_transport::codec::read(&mut read, &budget),
                )
                .await
                .unwrap()
                {
                    Ok(Some(frame)) if frame.kind == Kind::Progress => {}
                    Ok(Some(frame)) => {
                        assert!(!late_progress);
                        assert_eq!(frame.kind, Kind::Ready);
                        break;
                    }
                    Ok(None) | Err(_) => {
                        assert!(late_progress);
                        break;
                    }
                }
            }
            // Reject Ready without acknowledging it, or close after failed
            // final progress. The fixture ignores EOF and requires reaping.
            drop((read, write));
            assert!(tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            let pid = host.bridge_pid().expect("bridge was acquired");
            assert!(
                !std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success(),
                "return precedes actual child cleanup"
            );
        }
    }

    async fn json_message(peer: &mut Peer) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(3), peer.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.frame.kind, Kind::Text);
        serde_json::from_slice(message.frame.application().unwrap().1).unwrap()
    }

    async fn prelude(peer: &mut Peer, target: &str) {
        let mut steps = Vec::new();
        loop {
            let message = json_message(peer).await;
            match message["type"].as_str().unwrap() {
                "remote.progress" => steps.push(message["step"].as_str().unwrap().to_owned()),
                "remote.ready" => {
                    assert_eq!(message["target"], target);
                    break;
                }
                other => panic!("application event before host prelude: {other}"),
            }
        }
        assert!(steps.iter().any(|step| step == "daemon"));
        assert_eq!(steps.last().map(String::as_str), Some("bridge"));
        assert_eq!(json_message(peer).await["type"], "mux.welcome");
    }

    #[tokio::test]
    async fn native_host_streams_have_fixed_targets_independent_lifetimes_and_no_hub_authority() {
        let dir = tempfile::tempdir().unwrap();
        let host = ScriptedHost::new(dir.path());
        let hub = Arc::new(Hub::new(
            Box::new(host.clone()),
            dir.path().join("config"),
            None,
        ));
        let (mut first, first_task) = open(hub.clone(), "vm1").await;
        prelude(&mut first, "vm1").await;
        let first_pid = host.bridge_pid().unwrap();
        let (mut second, second_task) = open(hub.clone(), "vm2").await;
        prelude(&mut second, "vm2").await;
        assert_ne!(first_pid, host.bridge_pid().unwrap());
        first
            .sender
            .json(
                Kind::Request,
                &wire::Request {
                    id: 1,
                    operation: "host.list".into(),
                    args: serde_json::Map::new(),
                },
            )
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), first.recv())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            first_task.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let resync = json!({"type":"mux.resync"});
        second.sender.json(Kind::Text, &resync).unwrap();
        assert_eq!(
            json_message(&mut second).await,
            resync,
            "the other host remains usable"
        );
        first.shutdown().await;
        second.shutdown().await;
        second_task.await.unwrap().unwrap();
        assert_eq!(
            host.asked()
                .iter()
                .filter(|call| call.starts_with("prepare "))
                .count(),
            2,
            "denied input must never prepare another host"
        );
    }

    /// What a host's daemon sends, through the scripted bridge.
    async fn host_says(peer: &mut Peer, event: &str) {
        let input = json!({"type":"input","id":"__emit__","data":event});
        peer.sender.json(Kind::Text, &input).unwrap();
        let echoed = json_message(peer).await;
        assert_eq!(echoed, serde_json::from_str::<Value>(event).unwrap());
    }

    /// A `ports.changed` as a daemon writes it, its tag first.
    fn ports(list: &[(u16, &str)]) -> String {
        let ports: Vec<String> = list
            .iter()
            .map(|(port, process)| {
                format!(r#"{{"port":{port},"sessionId":"s1","process":"{process}"}}"#)
            })
            .collect();
        format!(
            r#"{{"type":"ports.changed","id":"e1","timestamp":1,"ports":[{}]}}"#,
            ports.join(",")
        )
    }

    // remote-screens, Automatic Port Forwarding: the hub forwards a port that
    // appears on a host once for all streams of the host, never a paused
    // rule, and nothing while the setting is off.
    #[tokio::test]
    async fn the_hub_forwards_appearing_ports_once_for_all_streams_of_a_host() {
        let dir = tempfile::tempdir().unwrap();
        let host = ScriptedHost::new(dir.path());
        let settings = Arc::new(std::sync::RwLock::new(
            splitty_core::config::HomeConfig::default(),
        ));
        settings.write().unwrap().remote.auto_forward_ports = true;
        let hub = Arc::new(
            Hub::new(Box::new(host.clone()), dir.path().join("config"), None)
                .with_settings(settings.clone()),
        );
        let [vite, paused, later] = [(); 3].map(|()| crate::hub::testing::free_port());
        hub.save_forward_with_restore("vm1", paused.into(), None, None, Some(false))
            .await
            .unwrap();
        let (mut first, first_task) = open(hub.clone(), "vm1").await;
        prelude(&mut first, "vm1").await;
        let (mut second, second_task) = open(hub.clone(), "vm1").await;
        prelude(&mut second, "vm1").await;
        let forwards = || {
            host.asked()
                .into_iter()
                .filter(|call| call.starts_with("forward "))
                .collect::<Vec<_>>()
        };

        // The first list of each stream is a baseline.
        host_says(&mut first, &ports(&[(22, "sshd")])).await;
        host_says(&mut second, &ports(&[(22, "sshd")])).await;
        let appeared = ports(&[(22, "sshd"), (vite, "vite"), (paused, "api")]);
        host_says(&mut first, &appeared).await;
        host_says(&mut second, &appeared).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while !hub.forwards().iter().any(|forward| forward.remote == vite) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the appeared port is forwarded");
        assert_eq!(forwards(), [format!("forward vm1 {vite} at {vite}")]);
        assert!(!hub
            .forward_preferences()
            .forwards
            .iter()
            .any(|row| row.enabled));

        // Off: what appears is not forwarded, also not after turning it on.
        settings.write().unwrap().remote.auto_forward_ports = false;
        let off = ports(&[
            (22, "sshd"),
            (vite, "vite"),
            (paused, "api"),
            (later, "ruby"),
        ]);
        host_says(&mut first, &off).await;
        settings.write().unwrap().remote.auto_forward_ports = true;
        host_says(&mut first, &off).await;
        assert_eq!(forwards().len(), 1);

        first.shutdown().await;
        second.shutdown().await;
        first_task.await.unwrap().unwrap();
        second_task.await.unwrap().unwrap();
    }
}
