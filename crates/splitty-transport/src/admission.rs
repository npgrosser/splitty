//! Descriptor/task admission is independent of byte budgets. Quota objects are
//! assigned by the native broker/authenticated gateway, never decoded from peer
//! strings. There is no waiting queue and a failed promotion rolls back fully.
use crate::budget::{Budget, Exhausted, Permit};
use std::sync::{Arc, OnceLock};

pub const CARRIERS: usize = 256;
pub const STREAMS: usize = 128;
pub const OPENING: usize = 16;
pub const BACKEND_STREAMS: usize = 64;
pub const DOMAIN_STREAMS: usize = 32;
pub const DOMAIN_OPENING: usize = 4;
pub const WORKERS: usize = 32;

pub struct Admission {
    carriers: Arc<Budget>,
    streams: Arc<Budget>,
    opening: Arc<Budget>,
    workers: Arc<Budget>,
}
pub struct Backend {
    streams: Arc<Budget>,
}
pub struct Domain {
    streams: Arc<Budget>,
    opening: Arc<Budget>,
}
#[derive(Clone)]
pub struct Assignment {
    backend: Arc<Backend>,
    domain: Arc<Domain>,
}
pub struct Carrier {
    _slot: Permit,
}
pub struct Opening {
    slots: StreamSlots,
    _process: Permit,
    _domain: Permit,
}
struct StreamSlots {
    _process: Permit,
    _backend: Permit,
    _domain: Permit,
    _assignment: Assignment,
}
#[derive(Clone)]
pub struct Stream {
    _slots: Arc<StreamSlots>,
}
#[derive(Debug, Eq, PartialEq)]
pub struct Counts {
    pub carriers: usize,
    pub streams: usize,
    pub opening: usize,
    pub workers: usize,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            carriers: Budget::new(CARRIERS),
            streams: Budget::new(STREAMS),
            opening: Budget::new(OPENING),
            workers: Budget::new(WORKERS),
        }
    }
}
impl Admission {
    pub fn global() -> Arc<Self> {
        static GLOBAL: OnceLock<Arc<Admission>> = OnceLock::new();
        GLOBAL.get_or_init(|| Arc::new(Self::default())).clone()
    }
    pub fn carrier(&self) -> Result<Carrier, Exhausted> {
        Ok(Carrier {
            _slot: self.carriers.acquire(1)?,
        })
    }
    pub fn begin(self: &Arc<Self>, assignment: Assignment) -> Result<Opening, Exhausted> {
        let process = self.opening.acquire(1)?;
        let domain = assignment.domain.opening.acquire(1)?;
        // Reserve the eventual live capacity before any dial/setup/spawn.
        // Promotion must never discover quota exhaustion after upstream work.
        let slots = StreamSlots {
            _process: self.streams.acquire(1)?,
            _backend: assignment.backend.streams.acquire(1)?,
            _domain: assignment.domain.streams.acquire(1)?,
            _assignment: assignment,
        };
        Ok(Opening {
            slots,
            _process: process,
            _domain: domain,
        })
    }
    pub(crate) fn worker(&self) -> Result<Permit, Exhausted> {
        self.workers.acquire(1)
    }
    pub fn counts(&self) -> Counts {
        Counts {
            carriers: self.carriers.used(),
            streams: self.streams.used(),
            opening: self.opening.used(),
            workers: self.workers.used(),
        }
    }
}
impl Backend {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            streams: Budget::new(BACKEND_STREAMS),
        })
    }
}
impl Domain {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            streams: Budget::new(DOMAIN_STREAMS),
            opening: Budget::new(DOMAIN_OPENING),
        })
    }
}
impl Assignment {
    pub fn new(backend: Arc<Backend>, domain: Arc<Domain>) -> Self {
        Self { backend, domain }
    }
}
impl Opening {
    pub fn admit(self) -> Result<Stream, Exhausted> {
        Ok(Stream {
            _slots: Arc::new(self.slots),
        })
    }
}

/// An aborted accept belongs to one peer, while resource exhaustion needs a
/// short pause. Neither failure should terminate other admitted connections.
pub fn accept_backoff(error: &std::io::Error) -> Option<std::time::Duration> {
    match error.kind() {
        std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::Interrupted => None,
        _ => Some(std::time::Duration::from_secs(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_admitted_stream_retains_its_trusted_quota_identity() {
        let admission = Arc::new(Admission::default());
        let backend = Backend::new();
        let weak = Arc::downgrade(&backend);
        let stream = admission
            .begin(Assignment::new(backend, Domain::new()))
            .unwrap()
            .admit()
            .unwrap();
        assert!(
            weak.upgrade().is_some(),
            "a registry must reuse an active stream's backend quota"
        );
        drop(stream);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn anonymous_carriers_and_handshakes_are_bounded_before_upstream_work() {
        let admission = Arc::new(Admission::default());
        let carriers: Vec<_> = (0..CARRIERS)
            .map(|_| admission.carrier().unwrap())
            .collect();
        assert!(admission.carrier().is_err());
        let assignment = Assignment::new(Backend::new(), Domain::new());
        let pending: Vec<_> = (0..DOMAIN_OPENING)
            .map(|_| admission.begin(assignment.clone()).unwrap())
            .collect();
        assert!(admission.begin(assignment).is_err());
        assert_eq!(admission.counts().opening, DOMAIN_OPENING);
        drop(pending);
        let pending: Vec<_> = (0..OPENING)
            .map(|_| {
                admission
                    .begin(Assignment::new(Backend::new(), Domain::new()))
                    .unwrap()
            })
            .collect();
        assert!(admission
            .begin(Assignment::new(Backend::new(), Domain::new()))
            .is_err());
        drop(pending);
        drop(carriers);
        assert_eq!(
            admission.counts(),
            Counts {
                carriers: 0,
                streams: 0,
                opening: 0,
                workers: 0
            }
        );
    }
    #[test]
    fn streams_are_limited_by_process_backend_and_shared_trust_domain() {
        let admission = Arc::new(Admission::default());
        let backend = Backend::new();
        let domain = Domain::new();
        let assignment = Assignment::new(backend.clone(), domain);
        let mut streams: Vec<_> = (0..DOMAIN_STREAMS)
            .map(|_| {
                admission
                    .begin(assignment.clone())
                    .unwrap()
                    .admit()
                    .unwrap()
            })
            .collect();
        assert!(
            admission.begin(assignment).is_err(),
            "a saturated domain cannot start upstream setup"
        );
        for _ in DOMAIN_STREAMS..BACKEND_STREAMS {
            streams.push(
                admission
                    .begin(Assignment::new(backend.clone(), Domain::new()))
                    .unwrap()
                    .admit()
                    .unwrap(),
            );
        }
        assert!(
            admission
                .begin(Assignment::new(backend, Domain::new()))
                .is_err(),
            "a saturated backend cannot dial"
        );
        for _ in BACKEND_STREAMS..STREAMS {
            streams.push(
                admission
                    .begin(Assignment::new(Backend::new(), Domain::new()))
                    .unwrap()
                    .admit()
                    .unwrap(),
            );
        }
        assert!(
            admission
                .begin(Assignment::new(Backend::new(), Domain::new()))
                .is_err(),
            "process stream capacity is reserved before spawn"
        );
        assert_eq!(admission.counts().streams, STREAMS);
        assert_eq!(admission.counts().opening, 0);
        drop(streams);
        assert_eq!(admission.counts().streams, 0);
    }
}
