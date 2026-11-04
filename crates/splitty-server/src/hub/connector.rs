//! The hub's way to hosts: the system `ssh`, as `splitty connect` uses it,
//! over connections that carry only the commands of the install, the
//! handshake, the update, the bridge and the relay preparation and their
//! standard input and output, only one port forward, or only one remote
//! forward and the relay that serves it.

use futures::future::BoxFuture;
use splitty_remote::bridge::NativeBridge;
use splitty_remote::handshake::RunningDaemon;
use splitty_remote::handshake::{self, Handshake, HandshakeError};
use splitty_remote::install::{self, Prepared, Server};
use splitty_remote::ssh::{run_checked, Ssh, SshError, SshTarget};
use splitty_remote::tunnel::{
    parse_prepared, prepare_command, PortReservation, PortTunnel, ReverseTunnel,
};
use splitty_transport::{admission, negotiation, wire};

use super::{Connector, Forwarding, HubError, VERSION};

/// Hosts reached with the system `ssh`, which get this client's own server
/// when they lack it.
pub struct SshConnector;

impl Connector for SshConnector {
    fn prepare<'a>(
        &'a self,
        target: &'a SshTarget,
        on_install: Box<dyn FnOnce(&str) + Send + 'a>,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<Prepared, HubError>> {
        Box::pin(async move {
            let ssh = Ssh::stdio_only(target.clone()).with_context(context.clone());
            let prepared = install::prepare(&ssh, VERSION, install::payload_for, |probe| {
                on_install(&probe.arch)
            })
            .await?;
            Ok(prepared)
        })
    }

    fn handshake<'a>(
        &'a self,
        target: &'a SshTarget,
        prepared: &'a Prepared,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<splitty_transport::budget::Charged<Handshake>, HubError>> {
        Box::pin(async move {
            let ssh = Ssh::stdio_only(target.clone()).with_context(context.clone());
            match handshake::handshake(&ssh, &prepared.server, true).await {
                // `prepare` found neither this version installed nor a payload
                // for the host, and the host's `PATH` has no splitty either.
                Err(HandshakeError::Ssh(SshError::NotFound {
                    host,
                    allocation: _source_allocation,
                })) if prepared.server == Server::Path => {
                    let storage = context
                        .budget
                        .reserve(
                            host.capacity()
                                + prepared.probe.arch.len()
                                + std::mem::size_of::<HubError>()
                                + std::mem::size_of::<splitty_transport::budget::Lease>(),
                            false,
                        )
                        .map_err(|error| {
                            HubError::NativeBridge(std::io::Error::new(
                                std::io::ErrorKind::WouldBlock,
                                error,
                            ))
                        })?;
                    Err(HubError::Retained {
                        error: Box::new(HubError::NoServer {
                            host,
                            arch: prepared.probe.arch.clone(),
                        }),
                        allocation: Box::new(storage),
                    })
                }
                answer => Ok(answer?),
            }
        })
    }

    fn stop_daemon<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
        daemon: &'a RunningDaemon,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        Box::pin(async move {
            let ssh = Ssh::stdio_only(target.clone()).with_context(context.clone());
            let instance = daemon
                .instance
                .as_deref()
                .ok_or_else(|| HubError::NativeTransport {
                    host: target.as_str().into(),
                })?;
            let line = server
                .command()
                .word("daemon")
                .word("stop")
                .word("--if-instance")
                .value(instance);
            run_checked(&ssh, &line, None)
                .await
                .map_err(HubError::Stop)?;
            Ok(())
        })
    }

    fn migrate_legacy<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
        daemon: &'a RunningDaemon,
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<(), HubError>> {
        Box::pin(async move {
            let Some(splitty_remote::handshake::LegacyMigration::Ready { token, .. }) =
                &daemon.legacy_migration
            else {
                return Err(HubError::MigrationUnavailable {
                    message: "Legacy migration is unavailable.".into(),
                });
            };
            let ssh = Ssh::stdio_only(target.clone()).with_context(context.clone());
            let line = server
                .command()
                .word("remote-migrate")
                .word("--expected")
                .value(token);
            run_checked(&ssh, &line, None)
                .await
                .map_err(HubError::Stop)?;
            Ok(())
        })
    }

    fn bridge<'a>(
        &'a self,
        target: &'a SshTarget,
        server: &'a Server,
        daemon: &'a RunningDaemon,
        capacity: (admission::Carrier, admission::Opening),
        context: &'a splitty_transport::queue::Context,
    ) -> BoxFuture<'a, Result<NativeBridge, HubError>> {
        Box::pin(async move {
            NativeBridge::open_context(
                target,
                server,
                capacity,
                context.clone(),
                negotiation::Expected {
                    instance: daemon.instance.as_deref(),
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: daemon.protocol,
                    preparation: wire::HANDSHAKE_TIMEOUT,
                },
            )
            .await
            .map_err(HubError::NativeBridge)
        })
    }

    fn forward<'a>(
        &'a self,
        target: &'a SshTarget,
        local: PortReservation,
        remote: u16,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        Box::pin(async move {
            let ssh = Ssh::forward_only(target.clone());
            let tunnel = PortTunnel::open(&ssh, local, remote)
                .await
                .map_err(HubError::Forward)?;
            Ok(Box::new(tunnel) as Box<dyn Forwarding>)
        })
    }

    fn reserve_share(
        &self,
        local: u16,
    ) -> BoxFuture<'_, Result<Option<splitty_remote::tunnel::exposure::Share>, HubError>> {
        Box::pin(async move {
            splitty_remote::tunnel::exposure::Share::reserve(local)
                .await
                .map(Some)
                .map_err(|error| HubError::LocalShare { port: local, error })
        })
    }

    fn reverse<'a>(
        &'a self,
        target: &'a SshTarget,
        local: u16,
        port: u16,
        share: Option<&'a splitty_remote::tunnel::exposure::Share>,
    ) -> BoxFuture<'a, Result<Box<dyn Forwarding>, HubError>> {
        Box::pin(async move {
            let share = share.ok_or_else(|| HubError::LocalShare {
                port: local,
                error: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "reverse forwarding has no reservation",
                ),
            })?;
            share
                .check(local)
                .await
                .map_err(|error| HubError::LocalShare { port: local, error })?;
            let tunnel = self.reverse_tunnel(target, local, port, share).await?;
            Ok(Box::new(tunnel) as Box<dyn Forwarding>)
        })
    }
}

impl SshConnector {
    async fn reverse_tunnel(
        &self,
        target: &SshTarget,
        local: u16,
        port: u16,
        share: &splitty_remote::tunnel::exposure::Share,
    ) -> Result<ReverseTunnel, HubError> {
        let prepared = self
            .prepare(target, Box::new(|_| {}), &Default::default())
            .await?;
        if prepared.probe.kernel != "Linux" {
            return Err(HubError::ReverseUnsupported {
                host: target.as_str().to_owned(),
                kernel: prepared.probe.kernel,
            });
        }
        let ssh = Ssh::stdio_only(target.clone());
        let output = run_checked(&ssh, &prepare_command(&prepared.server), None)
            .await
            .map_err(HubError::RelayPrepare)?;
        let dir = parse_prepared(&output.stdout).map_err(|detail| HubError::RelayAnswer {
            host: target.as_str().to_owned(),
            detail,
        })?;
        // The directory is the user's own; the name needs only to differ
        // from the other forwards' and from earlier attempts'.
        let name = format!("{:016x}.sock", rand::random::<u64>());
        ReverseTunnel::open_reserved(
            &Ssh::relay_only(target.clone()),
            &prepared.server,
            &dir,
            &name,
            local,
            port,
            share.clone(),
        )
        .await
        .map_err(HubError::Reverse)
    }
}

impl Forwarding for ReverseTunnel {
    fn ended(&mut self) -> BoxFuture<'_, String> {
        Box::pin(self.closed())
    }

    fn stop(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(self.close())
    }
}

impl Forwarding for PortTunnel {
    fn ended(&mut self) -> BoxFuture<'_, String> {
        Box::pin(self.closed())
    }

    fn stop(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(self.close())
    }
}

#[cfg(test)]
mod tests {
    /// A hub names the configuration directory of the server it installed on
    /// a macOS host by the variable that server reads.
    #[test]
    fn the_hub_sets_the_variable_the_server_reads() {
        assert_eq!(
            splitty_remote::install::CONFIG_DIR_VARIABLE,
            splitty_core::config::CONFIG_DIR_VARIABLE
        );
    }
}
