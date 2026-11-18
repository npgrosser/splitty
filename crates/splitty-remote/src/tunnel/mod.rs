//! Host update policy and deliberately forwarded ports over owned SSH workers.

pub mod exposure;
mod forward;
mod reverse;
mod supervisor;

pub use forward::{
    check_forward_address, reserve, reserve_exact, reserve_free_at, Forward, PortForward,
    PortTunnel, PortTunnelError,
};
pub use reverse::{
    parse_prepared, prepare_command, socket_path, ReverseTunnel, ReverseTunnelError, HEARTBEAT,
    MAX_SOCKET_PATH, RELAY_FAILED, RELAY_READY,
};
pub use supervisor::{backoff, migration_offer, older, update_plan, UpdatePlan, PREPARE_TIMEOUT};

use std::io;
use std::net::TcpListener;
use std::time::Duration;

use serde::Serialize;
use splitty_transport::worker::Worker;

/// Safe renderer-facing migration offer. The host's revalidation token stays
/// in native handshake state and is never serialized into a UI diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationOffer {
    pub available: bool,
    pub from_version: String,
    pub to_version: String,
    pub sessions: u32,
}

/// A local port held open until the tunnel takes it over.
///
/// Keep the reservation until just before SSH binds the forward, reducing
/// the interval in which another process could take the port.
pub struct PortReservation {
    bind: std::net::IpAddr,
    listener: Option<TcpListener>,
    port: u16,
}

impl PortReservation {
    /// Reserve any free loopback port.
    pub fn acquire() -> io::Result<Self> {
        Self::bind(0)
    }

    /// Reserve a specific loopback port. Fails if anything listens on it.
    pub fn acquire_port(port: u16) -> io::Result<Self> {
        Self::bind(port)
    }

    fn bind(port: u16) -> io::Result<Self> {
        Self::acquire_at(std::net::Ipv4Addr::LOCALHOST.into(), port)
    }

    pub fn acquire_at(bind: std::net::IpAddr, port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind((bind, port))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener: Some(listener),
            bind,
            port,
        })
    }

    /// Recheck IPv6 while the IPv4 reservation stays held (including rebuilds).
    pub fn check_forward_address(&self) -> io::Result<()> {
        Self::check_bind(self.bind, self.port)
    }

    pub fn bind_address(&self) -> std::net::IpAddr {
        self.bind
    }
    pub fn check_bind(bind: std::net::IpAddr, port: u16) -> io::Result<()> {
        if bind.is_ipv4()
            && (bind.is_unspecified()
                || bind == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        {
            forward::check_forward_address(port)
        } else {
            Ok(())
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Transfer the already-bound listener without releasing the port.
    pub fn into_listener(mut self) -> io::Result<TcpListener> {
        self.listener.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "port reservation was already handed off",
            )
        })
    }
}

const EXIT_GRACE: Duration = Duration::from_secs(2);

async fn wait_for_forward_at(
    child: &mut Worker,
    local_port: u16,
    bind: std::net::IpAddr,
) -> io::Result<()> {
    let bind = if bind.is_unspecified() {
        if bind.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        }
    } else {
        bind
    };
    use tokio::net::TcpStream;
    use tokio::time::{sleep, timeout, Instant};

    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline {
        // If ssh exited, ExitOnForwardFailure did its job — report that rather
        // than waiting out the timeout.
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "ssh exited before the forward was established ({status})"
            )));
        }

        let connect = TcpStream::connect((bind, local_port));
        if let Ok(Ok(_)) = timeout(Duration::from_millis(250), connect).await {
            // Re-check liveness: something else could be listening on that port.
            // With ExitOnForwardFailure set, a live ssh means the forward bound.
            if child.try_wait()?.is_none() {
                return Ok(());
            }
            return Err(io::Error::other(
                "the local port is in use by another process, not by the ssh forward",
            ));
        }

        sleep(Duration::from_millis(100)).await;
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "ssh forward was not established in time",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_reservation_holds_its_port_until_released() {
        let reservation = PortReservation::acquire().expect("acquire");
        let port = reservation.port();

        // While reserved, a competing bind must fail.
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_err(),
            "the reservation must actually hold the port",
        );

        drop(reservation);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "the port must be free once the reservation is dropped",
        );
    }

    /// An exact reservation reports a conflict on an occupied port.
    #[test]
    fn a_specific_port_is_reserved_or_refused() {
        let first = PortReservation::acquire().expect("acquire");
        let port = first.port();

        assert!(
            PortReservation::acquire_port(port).is_err(),
            "a port that is in use must not be reserved twice",
        );

        drop(first);
        let second = PortReservation::acquire_port(port).expect("free port");
        assert_eq!(second.port(), port);
    }
}
