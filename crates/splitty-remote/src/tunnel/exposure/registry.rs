//! One OS-user authority for browser listeners and deliberately shared local
//! ports. No control/HTTP service is opened to coordinate reservations.
#[cfg(any(target_os = "macos", test))]
mod acl;
#[cfg(all(test, target_os = "linux"))]
mod crash;
mod platform;
#[cfg(unix)]
mod storage;

use serde::{Deserialize, Serialize};
use splitty_transport::{
    admission::Admission,
    budget::{ConnectionBudget, Lease},
    worker::Worker,
};
use std::{
    io,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

const MAX_STATE_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 1024;
const MAX_GENERATIONS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    machine: String,
    boot: String,
    network: u64,
    pid_namespace: u64,
    uid: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    pid: u32,
    start: u64,
    subsecond: u64,
    uid: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Records {
    format: u32,
    entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    port: u16,
    scope: Scope,
    owner: Identity,
    kind: Purpose,
    active: bool,
    workers: Vec<Generation>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Purpose {
    Gateway,
    Share,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    id: String,
    worker: Option<Identity>,
}

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported,
        "reverse sharing is unavailable without a shared Windows/WSL loopback reservation authority")
}
fn denied(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason.into())
}
fn nonce() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// The service calls this before installation/SSH preparation as well as before
/// an actual reverse-forward worker. Failure never creates another endpoint.
pub fn check_sharing_supported() -> io::Result<()> {
    match platform::policy()? {
        platform::Policy::Native { scope, .. } => {
            let me = platform::identity(std::process::id())?;
            if matches!(platform::observe(&scope, &me), platform::Observed::Live) {
                Ok(())
            } else {
                Err(denied("cannot verify local forwarding worker identity"))
            }
        }
        #[cfg(not(target_os = "macos"))]
        platform::Policy::NoSharing => Err(unsupported()),
    }
}

const MAX_CLEANUP: usize = MAX_ENTRIES * 2 + 64;
struct Cleanup {
    sequence: u64,
    entry: String,
    generation: Option<String>,
}
struct Retired {
    next: u64,
    entries: Vec<Arc<Cleanup>>,
    exhausted: bool,
}
struct Registry {
    directory: PathBuf,
    scope: Scope,
    owner: Identity,
    retired: Mutex<Retired>,
    _cleanup_bytes: Lease,
    directory_identity: OnceLock<(u64, u64)>,
}
impl Registry {
    fn system() -> io::Result<Option<Arc<Self>>> {
        match platform::policy()? {
            #[cfg(not(target_os = "macos"))]
            platform::Policy::NoSharing => Ok(None),
            platform::Policy::Native { scope, directory } => {
                let owner = platform::identity(std::process::id())?;
                if owner.uid != scope.uid {
                    return Err(denied("reservation owner identity mismatch"));
                }
                static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
                let registry = if let Some(registry) = REGISTRY.get() {
                    registry.clone()
                } else {
                    let candidate = Self::new(directory.clone(), scope.clone(), owner.clone())?;
                    REGISTRY.get_or_init(|| candidate).clone()
                };
                if registry.scope != scope
                    || registry.directory != directory
                    || registry.owner != owner
                {
                    return Err(denied(
                        "forwarding reservation authority changed within this process",
                    ));
                }
                Ok(Some(registry))
            }
        }
    }
    fn new(directory: PathBuf, scope: Scope, owner: Identity) -> io::Result<Arc<Self>> {
        let bytes = ConnectionBudget::default()
            .reserve(512 * 1024, false)
            .map_err(io::Error::other)?;
        Ok(Arc::new(Self {
            directory,
            scope,
            owner,
            retired: Mutex::new(Retired {
                next: 0,
                entries: Vec::with_capacity(MAX_CLEANUP),
                exhausted: false,
            }),
            _cleanup_bytes: bytes,
            directory_identity: OnceLock::new(),
        }))
    }
    fn retire(&self, entry: &str, generation: Option<&str>) {
        let mut retired = self.retired.lock().expect("reservation cleanup");
        if retired.entries.len() == MAX_CLEANUP || retired.next == u64::MAX {
            retired.exhausted = true;
            return; // persisted records still deny exposure
        }
        retired.next += 1;
        let sequence = retired.next;
        retired.entries.push(Arc::new(Cleanup {
            sequence,
            entry: entry.to_owned(),
            generation: generation.map(str::to_owned),
        }));
    }
    #[cfg(unix)]
    fn transaction<T>(&self, edit: impl FnOnce(&mut Records) -> io::Result<T>) -> io::Result<T> {
        // Account for the bounded input, typed structures, Vec capacity growth
        // and serialized replacement before reading or deserializing anything.
        let budget = ConnectionBudget::default();
        let _bytes = budget
            .reserve(16 * MAX_STATE_BYTES, false)
            .map_err(io::Error::other)?;
        let retired = {
            let retired = self.retired.lock().expect("reservation cleanup");
            if retired.exhausted {
                return Err(denied(
                    "reservation cleanup capacity exhausted; exposure remains closed",
                ));
            }
            retired.entries.clone()
        };
        if let Some(expected) = self.directory_identity.get() {
            use std::os::unix::fs::MetadataExt;
            let actual = std::fs::symlink_metadata(&self.directory)?;
            if actual.file_type().is_symlink() || (actual.dev(), actual.ino()) != *expected {
                return Err(denied(
                    "forwarding reservation directory was replaced; admission is closed",
                ));
            }
        }
        let lock = storage::Locked::acquire(&self.directory)?;
        let identity = lock.identity()?;
        if *self.directory_identity.get_or_init(|| identity) != identity {
            return Err(denied(
                "forwarding reservation directory changed; admission is closed",
            ));
        }
        let mut records = lock.read()?;
        let mut identities = std::collections::HashSet::new();
        let mut generations = std::collections::HashSet::new();
        let valid_id = |id: &str| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit());
        if records.format != 1
            || records.entries.len() > MAX_ENTRIES
            || records.entries.iter().any(|entry| {
                entry.port == 0
                    || !valid_id(&entry.id)
                    || !identities.insert(entry.id.as_str())
                    || entry.owner.pid == 0
                    || entry.owner.uid != entry.scope.uid
                    || entry.workers.len() > MAX_GENERATIONS
                    || entry.scope.machine.is_empty()
                    || entry.scope.machine.len() > 128
                    || entry.scope.boot.is_empty()
                    || entry.scope.boot.len() > 128
                    || (entry.kind == Purpose::Gateway && !entry.workers.is_empty())
                    || entry.workers.iter().any(|worker| {
                        !valid_id(&worker.id)
                            || !generations.insert(worker.id.as_str())
                            || worker.worker.as_ref().is_some_and(|identity| {
                                identity.pid == 0 || identity.uid != entry.scope.uid
                            })
                    })
            })
        {
            return Err(denied(
                "invalid forwarding reservation registry; admission is closed",
            ));
        }
        for cleanup in &retired {
            if let Some(entry) = records
                .entries
                .iter_mut()
                .find(|entry| entry.id == cleanup.entry)
            {
                if let Some(generation) = &cleanup.generation {
                    entry.workers.retain(|worker| &worker.id != generation);
                } else {
                    entry.active = false;
                }
            }
        }
        self.sweep(&mut records);
        let result = edit(&mut records)?;
        lock.write(&records)?;
        if let Some(last) = retired.last() {
            self.retired
                .lock()
                .expect("reservation cleanup")
                .entries
                .retain(|cleanup| cleanup.sequence > last.sequence);
        }
        Ok(result)
    }
    #[cfg(not(unix))]
    fn transaction<T>(&self, _edit: impl FnOnce(&mut Records) -> io::Result<T>) -> io::Result<T> {
        Err(unsupported())
    }

    async fn retry<T>(&self, mut edit: impl FnMut(&mut Records) -> io::Result<T>) -> io::Result<T> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match self.transaction(&mut edit) {
                Err(error)
                    if storage::is_busy(&error) && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await
                }
                result => return result,
            }
        }
    }
    fn sweep(&self, records: &mut Records) {
        records.entries.retain_mut(|entry| {
            if !entry.active && entry.workers.is_empty() {
                return false;
            }
            if entry.scope.machine == self.scope.machine && entry.scope.boot != self.scope.boot {
                return false; // positively identified old boot, no surviving local worker
            }
            if entry.scope.machine != self.scope.machine
                || entry.scope.boot != self.scope.boot
                || entry.scope.pid_namespace != self.scope.pid_namespace
                || entry.scope.uid != self.scope.uid
            {
                return true; // never interpret a foreign process view as absence
            }
            entry.workers.retain(|generation| {
                !generation.worker.as_ref().is_some_and(|worker| {
                    matches!(
                        platform::observe(&self.scope, worker),
                        platform::Observed::Terminated
                    )
                })
            });
            if matches!(
                platform::observe(&self.scope, &entry.owner),
                platform::Observed::Terminated
            ) {
                entry.active = false;
            }
            entry.active || !entry.workers.is_empty()
        });
    }
    fn conflict(&self, records: &Records, port: u16, purpose: Purpose) -> bool {
        records.entries.iter().any(|entry| {
            entry.port == port
                && entry.kind != purpose
                && (entry.scope.machine != self.scope.machine
                    || entry.scope.boot != self.scope.boot
                    || entry.scope.network == self.scope.network)
        })
    }
    fn add(
        self: &Arc<Self>,
        records: &mut Records,
        port: u16,
        purpose: Purpose,
    ) -> io::Result<Claim> {
        if self.conflict(records, port, purpose) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("local port {port} is reserved by an internal listener or reverse share"),
            ));
        }
        if records.entries.len() == MAX_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "forwarding reservation ceiling reached",
            ));
        }
        let id = nonce();
        records.entries.push(Entry {
            id: id.clone(),
            port,
            scope: self.scope.clone(),
            owner: self.owner.clone(),
            kind: purpose,
            active: true,
            workers: Vec::new(),
        });
        Ok(Claim {
            registry: self.clone(),
            id,
            port,
        })
    }
    fn bind(self: &Arc<Self>, address: SocketAddr) -> io::Result<GatewayListener> {
        self.transaction(|records| {
            // Port-zero allocation may select a currently unbound but still
            // durably shared destination. Keep retrying a bounded number of
            // kernel choices while the same conflict lock is held.
            for _ in 0..64 {
                if address.port() != 0 && self.conflict(records, address.port(), Purpose::Gateway) {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "requested browser port is still shared",
                    ));
                }
                let listener = TcpListener::bind(address)?;
                let port = listener.local_addr()?.port();
                if self.conflict(records, port, Purpose::Gateway) {
                    drop(listener);
                    continue;
                }
                listener.set_nonblocking(true)?;
                let claim = self.add(records, port, Purpose::Gateway)?;
                let listener = tokio::net::TcpListener::from_std(listener)?;
                return Ok(GatewayListener {
                    listener,
                    _guard: GatewayGuard(Some(claim)),
                });
            }
            Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no provably unshared browser port is available",
            ))
        })
    }
}

struct Claim {
    registry: Arc<Registry>,
    id: String,
    port: u16,
}
impl Drop for Claim {
    fn drop(&mut self) {
        // Do not lose a cleanup assertion merely because another process is
        // committing right now. The per-process authority retains it for the
        // next transaction; a crash still leaves conservative on-disk claims.
        self.registry.retire(&self.id, None);
        let _ = self.registry.transaction(|_| Ok(()));
    }
}

/// This guard must be held until the actual listener is closed. It carries no
/// token, HTTP handler or authority to expose another destination.
struct GatewayGuard(Option<Claim>);
impl Drop for GatewayGuard {
    fn drop(&mut self) {
        self.0.take();
    }
}
pub struct GatewayListener {
    // Drop closes the actual listener before releasing its reservation.
    listener: tokio::net::TcpListener,
    _guard: GatewayGuard,
}
impl GatewayListener {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
    pub async fn accept(&self) -> io::Result<(tokio::net::TcpStream, SocketAddr)> {
        self.listener.accept().await
    }
}

/// Bind and publish under the same reservation lock. Windows/WSL allow
/// gateways while refusing all reverse sharing until their broker authority is
/// shared; a local registry must not pretend those runtimes are isolated.
pub async fn bind_gateway(address: SocketAddr) -> io::Result<GatewayListener> {
    if !address.ip().is_loopback() {
        return Err(denied("browser listeners require loopback"));
    }
    match Registry::system()? {
        Some(registry) => {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match registry.bind(address) {
                    Err(error)
                        if storage::is_busy(&error) && tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await
                    }
                    result => break result,
                }
            }
        }
        None => {
            let listener = TcpListener::bind(address)?;
            listener.set_nonblocking(true)?;
            Ok(GatewayListener {
                listener: tokio::net::TcpListener::from_std(listener)?,
                _guard: GatewayGuard(None),
            })
        }
    }
}

/// An admitted share intent survives reconnect backoff independently of every
/// worker generation. Only the trusted connector creates it, before any SSH.
#[derive(Clone)]
pub struct Share {
    claim: Arc<Claim>,
}
impl Share {
    pub async fn reserve(port: u16) -> io::Result<Self> {
        if port == 0 {
            return Err(denied("a shared destination needs a nonzero port"));
        }
        let registry = Registry::system()?.ok_or_else(unsupported)?;
        registry
            .retry(|records| registry.add(records, port, Purpose::Share))
            .await
            .map(|claim| Self {
                claim: Arc::new(claim),
            })
    }
    pub async fn check(&self, port: u16) -> io::Result<()> {
        if self.claim.port != port {
            return Err(denied(
                "share reservation belongs to a different destination",
            ));
        }
        self.claim
            .registry
            .retry(|records| {
                if records.entries.iter().any(|entry| {
                    entry.id == self.claim.id
                        && entry.active
                        && entry.port == port
                        && entry.kind == Purpose::Share
                }) {
                    Ok(())
                } else {
                    Err(denied("share reservation was revoked before forwarding"))
                }
            })
            .await
    }
    /// Persist the ambiguous generation before OS spawn. An exit hook knows
    /// only its own generation; dropping the hook without observing exit does
    /// not clear the reservation.
    pub async fn spawn(
        &self,
        command: &mut tokio::process::Command,
        admission: &Admission,
    ) -> io::Result<Worker> {
        let generation = nonce();
        let id = &self.claim.id;
        let registry = &self.claim.registry;
        registry
            .retry(|records| {
                let entry = records
                    .entries
                    .iter_mut()
                    .find(|entry| &entry.id == id && entry.active)
                    .ok_or_else(|| denied("share intent was revoked before worker spawn"))?;
                if entry.workers.len() >= MAX_GENERATIONS {
                    return Err(denied("unresolved forwarding generations remain reserved"));
                }
                entry.workers.push(Generation {
                    id: generation.clone(),
                    worker: None,
                });
                Ok(())
            })
            .await?;
        #[cfg(all(test, target_os = "linux"))]
        crash::pause("beforeSpawn", None);
        let clear = {
            let registry = registry.clone();
            let id = id.clone();
            let generation = generation.clone();
            move || {
                registry.retire(&id, Some(&generation));
                let _ = registry.transaction(|_| Ok(()));
            }
        };
        let mut worker = match Worker::spawn(command, admission) {
            Ok(worker) => worker,
            Err(error) => {
                clear();
                return Err(error);
            } // spawn syscall proved no child
        };
        worker.on_exit(clear);
        let worker_pid = worker.id();
        #[cfg(all(test, target_os = "linux"))]
        crash::pause("beforePublication", worker_pid);
        let publish = async {
            let identity =
                platform::identity(worker_pid.ok_or_else(|| denied("worker has no identity"))?)?;
            if identity.uid != registry.scope.uid {
                return Err(denied("forwarding worker user mismatch"));
            }
            registry
                .retry(|records| {
                    let worker = records
                        .entries
                        .iter_mut()
                        .find(|entry| &entry.id == id)
                        .and_then(|entry| {
                            entry
                                .workers
                                .iter_mut()
                                .find(|worker| worker.id == generation)
                        })
                        .ok_or_else(|| {
                            denied("forwarding generation was revoked before publication")
                        })?;
                    worker.worker = Some(identity.clone());
                    Ok(())
                })
                .await
        }
        .await;
        if let Err(error) = publish {
            let _ = worker.kill().await; // hook clears only after observed exit
            return Err(error);
        }
        #[cfg(all(test, target_os = "linux"))]
        crash::pause("afterPublication", worker_pid);
        Ok(worker)
    }
}

#[cfg(all(test, unix))]
pub(crate) fn test_share(port: u16) -> (tempfile::TempDir, Share) {
    let (dir, registry) = tests::fixture();
    (dir, tests::share(&registry, port))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(super) fn fixture() -> (tempfile::TempDir, Arc<Registry>) {
        // Use a persistent local filesystem, irrespective of a tmpfs TMPDIR.
        // Only this private unit fixture supplies a path/scope directly; the
        // production entry points always verify the system authority above.
        let dir = tempfile::Builder::new()
            .prefix(".exposure-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let owner = platform::identity(std::process::id()).unwrap();
        let scope = Scope {
            machine: "unit-machine".into(),
            boot: "unit-boot".into(),
            network: 0,
            pid_namespace: 0,
            uid: owner.uid,
        };
        let registry = Registry::new(dir.path().join("registry"), scope, owner).unwrap();
        (dir, registry)
    }
    pub(super) fn share(registry: &Arc<Registry>, port: u16) -> Share {
        Share {
            claim: Arc::new(
                registry
                    .transaction(|records| registry.add(records, port, Purpose::Share))
                    .unwrap(),
            ),
        }
    }
    fn port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn address(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[tokio::test]
    async fn share_intents_exclude_gateways_across_configurations_and_both_families() {
        let (_dir, registry) = fixture();
        let port = port();
        let share = share(&registry, port);
        let other = Registry::new(
            registry.directory.clone(),
            registry.scope.clone(),
            registry.owner.clone(),
        )
        .unwrap();
        for address in [
            address(port),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ] {
            assert_eq!(
                other.bind(address).err().unwrap().kind(),
                io::ErrorKind::AddrInUse
            );
        }
        let alternate = other.bind(address(0)).unwrap();
        assert_ne!(alternate.local_addr().unwrap().port(), port);
        drop(alternate);
        drop(share);
        let gateway = other.bind(address(port)).unwrap();
        assert!(other
            .transaction(|records| other.add(records, port, Purpose::Share))
            .is_err());
        drop(gateway);
        drop(
            other
                .transaction(|records| other.add(records, port, Purpose::Share))
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn worker_exit_and_intent_release_are_independent_even_during_backoff() {
        let (_dir, registry) = fixture();
        let port = port();
        let share = share(&registry, port);
        let admission = Admission::default();
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let mut worker = share.spawn(&mut command, &admission).await.unwrap();
        worker.kill().await.unwrap();
        assert_eq!(admission.counts().workers, 0);
        assert!(
            registry.bind(address(port)).is_err(),
            "a reconnect intent survives a dead generation"
        );
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let mut worker = share.spawn(&mut command, &admission).await.unwrap();
        drop(share);
        assert!(
            registry.bind(address(port)).is_err(),
            "closing the coordinator intent is not worker exit"
        );
        worker.kill().await.unwrap();
        let gateway = registry.bind(address(port)).unwrap();
        drop(gateway);
    }

    #[tokio::test]
    async fn old_reapers_remove_only_their_own_generation() {
        let (_dir, registry) = fixture();
        let port = port();
        let share = share(&registry, port);
        let admission = Admission::default();
        let mut first = tokio::process::Command::new("sh");
        first.args(["-c", "exec sleep 30"]);
        let mut first = share.spawn(&mut first, &admission).await.unwrap();
        let mut second = tokio::process::Command::new("sh");
        second.args(["-c", "exec sleep 30"]);
        let mut second = share.spawn(&mut second, &admission).await.unwrap();
        drop(share);
        first.kill().await.unwrap();
        assert!(registry.bind(address(port)).is_err());
        second.kill().await.unwrap();
        drop(registry.bind(address(port)).unwrap());
    }

    #[tokio::test]
    async fn pending_generations_remain_reserved_but_proven_replacements_can_be_released() {
        for pending in [true, false] {
            let (_dir, registry) = fixture();
            let port = port();
            let share = share(&registry, port);
            registry
                .transaction(|records| {
                    let entry = records
                        .entries
                        .iter_mut()
                        .find(|entry| entry.id == share.claim.id)
                        .unwrap();
                    entry.owner.pid = u32::MAX; // positively absent coordinator
                    let mut reused = registry.owner.clone();
                    reused.start += 1;
                    entry.workers.push(Generation {
                        id: nonce(),
                        worker: if pending { None } else { Some(reused) },
                    });
                    Ok(())
                })
                .unwrap();
            drop(share);
            if pending {
                assert!(
                    registry.bind(address(port)).is_err(),
                    "unknown generation must survive owner death"
                );
                registry
                    .transaction(|records| {
                        assert_eq!(records.entries.len(), 1);
                        assert!(!records.entries[0].active);
                        Ok(())
                    })
                    .unwrap();
            } else {
                drop(
                    registry
                        .bind(address(port))
                        .expect("different verified process birth proves saved worker ended"),
                );
            }
        }
    }

    #[tokio::test]
    async fn foreign_process_views_do_not_turn_an_absent_coordinator_into_exit_proof() {
        let (_dir, registry) = fixture();
        let port = port();
        let claim = registry
            .transaction(|records| {
                let claim = registry.add(records, port, Purpose::Share)?;
                let entry = records.entries.last_mut().unwrap();
                entry.owner.pid = u32::MAX;
                entry.scope.pid_namespace = 99;
                Ok(claim)
            })
            .unwrap();
        assert!(registry.bind(address(port)).is_err());
        // A real owner can still revoke its intent, which has no worker.
        drop(claim);
        drop(registry.bind(address(port)).unwrap());
    }

    #[tokio::test]
    async fn corrupt_missing_or_unsafe_state_fails_closed_and_keeps_the_lock_inode() {
        let (_dir, registry) = fixture();
        registry.transaction(|_| Ok(())).unwrap();
        use std::os::unix::fs::MetadataExt;
        let lock = registry.directory.join("registry.lock");
        let inode = std::fs::metadata(&lock).unwrap().ino();
        for _ in 0..3 {
            let gateway = registry.bind(address(0)).unwrap();
            drop(gateway);
        }
        assert_eq!(std::fs::metadata(&lock).unwrap().ino(), inode);
        let state = registry.directory.join("registry.json");
        std::fs::write(&state, b"not json").unwrap();
        assert!(registry.bind(address(0)).is_err());
        std::fs::remove_file(&state).unwrap();
        assert!(
            registry.bind(address(0)).is_err(),
            "missing publication after an existing lock is not empty"
        );
        std::fs::write(&state, br#"{"format":1,"entries":[]}"#).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(registry.bind(address(0)).is_err());
        std::fs::remove_file(&state).unwrap();
        std::os::unix::fs::symlink("registry.lock", &state).unwrap();
        assert!(registry.bind(address(0)).is_err());
    }

    #[tokio::test]
    async fn first_lock_winner_finishes_another_creators_initial_publication() {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let (_dir, registry) = fixture();
        std::fs::create_dir(&registry.directory).unwrap();
        std::fs::set_permissions(&registry.directory, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        // A created the stable file, but B wins flock before A acquires it.
        let creator = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(registry.directory.join("registry.lock"))
            .unwrap();
        let inode = creator.metadata().unwrap().ino();
        registry
            .transaction(|records| {
                assert!(records.entries.is_empty());
                Ok(())
            })
            .unwrap();
        let port = port();
        let share = share(&registry, port);
        let late = storage::Locked::acquire(&registry.directory).unwrap();
        assert_eq!(
            late.read().unwrap().entries.len(),
            1,
            "late creator cannot reset an admitted claim"
        );
        assert_eq!(creator.metadata().unwrap().ino(), inode);
        drop(late);
        assert!(registry.bind(address(port)).is_err());
        drop(share);
        drop(registry.bind(address(port)).unwrap());
    }

    #[tokio::test]
    async fn contended_cleanup_is_retained_until_the_next_atomic_transaction() {
        let (_dir, registry) = fixture();
        let port = port();
        let share = share(&registry, port);
        let lock = storage::Locked::acquire(&registry.directory).unwrap();
        drop(share); // nonblocking Drop cannot take the already-held lock
        assert_eq!(registry.retired.lock().unwrap().entries.len(), 1);
        drop(lock);
        drop(registry.bind(address(port)).unwrap());
        assert!(registry.retired.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn storage_failure_never_releases_a_share_and_repair_preserves_its_claim() {
        let (_dir, registry) = fixture();
        let port = port();
        let share = share(&registry, port);
        let state = registry.directory.join("registry.json");
        let valid = std::fs::read(&state).unwrap();
        std::fs::remove_file(&state).unwrap();
        std::fs::create_dir(&state).unwrap();
        assert!(registry.bind(address(port)).is_err());
        std::fs::remove_dir(&state).unwrap();
        std::fs::write(&state, valid).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(registry.bind(address(port)).is_err());
        drop(share);
        drop(registry.bind(address(port)).unwrap());
    }
}
