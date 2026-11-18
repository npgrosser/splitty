//! No shared Windows/WSL loopback reservation broker has been established.
//! Every share operation fails before preparing or spawning SSH. Older
//! browser gateway claims remain part of the exposure registry format.
use splitty_transport::{admission::Admission, worker::Worker};
use std::io;
fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported,
        "reverse sharing is unavailable without a shared Windows/WSL loopback reservation authority")
}
pub fn check_sharing_supported() -> io::Result<()> {
    Err(unsupported())
}
#[derive(Clone)]
pub struct Share;
impl Share {
    pub async fn reserve(_port: u16) -> io::Result<Self> {
        Err(unsupported())
    }
    pub async fn check(&self, _port: u16) -> io::Result<()> {
        Err(unsupported())
    }
    pub async fn spawn(
        &self,
        _command: &mut tokio::process::Command,
        _admission: &Admission,
    ) -> io::Result<Worker> {
        Err(unsupported())
    }
}
