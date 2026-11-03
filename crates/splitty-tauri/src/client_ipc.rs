//! Native-owned document and connection authority, independent of the WebView
//! carrier. Only native window orchestration registers policies or commits a
//! document. The carrier supplies the actual WebView label and a proof kept in
//! the main document's private capsule; the public document ID is not a proof.
//!
//! A parent-owned popout has no registration. Calls delegated through its
//! parent share that parent's authority and quota, as they do in Electron.

pub(crate) mod bootstrap;
pub(crate) mod carrier;
pub(crate) mod pipe;
pub(crate) mod provider;
pub(crate) mod session;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use splitty_remote::ssh::SshTarget;
use splitty_transport::{admission, negotiation, queue, wire};

/// Native composition maps this opaque identity to an owned backend/provider.
/// Renderer fields never construct or replace a backend identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct BackendId([u8; 16]);
impl BackendId {
    pub(crate) fn new() -> Self {
        Self(rand::random())
    }
}

#[derive(Clone)]
pub(crate) enum Policy {
    Main {
        backend: BackendId,
    },
    Remote {
        backend: BackendId,
        target: SshTarget,
    },
    #[cfg_attr(
        not(any(target_os = "windows", test)),
        expect(
            dead_code,
            reason = "the setup role is constructed only by the Windows shell"
        )
    )]
    Setup,
}

/// This value is injected privately after a native-confirmed document commit.
/// It intentionally has no Debug or Serialize implementation. Public frontend
/// assignments contain only `document`, never this secret.
#[derive(Clone)]
pub(crate) struct Proof {
    pub(crate) document: u64,
    secret: [u8; 32],
}
impl Proof {
    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
    pub(crate) fn from_request(document: u64, secret: [u8; 32]) -> Self {
        Self { document, secret }
    }
    fn matches(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq;
        self.document == other.document && bool::from(self.secret.ct_eq(&other.secret))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct Handle([u8; 16]);
impl Handle {
    pub(crate) fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
    pub(crate) fn from_request(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Error {
    Denied,
    Busy,
    Conflict,
    Invalid,
    Closed,
}

/// Immutable native routing intent. Only a main policy can select a host in
/// its own backend's hub; a remote policy always names its assigned host.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub(crate) backend: BackendId,
    pub(crate) target: Option<SshTarget>,
    pub(crate) update: bool,
}
impl Plan {
    fn purpose(&self) -> wire::Purpose {
        match &self.target {
            Some(target) => wire::Purpose::Host {
                target: target.as_str().to_owned(),
            },
            None => wire::Purpose::Direct,
        }
    }
}

struct Window {
    policy: Policy,
    proof: Option<Proof>,
    attempts: HashSet<Handle>,
    bootstrap: Bootstrap,
}
struct Bootstrap {
    receiver: String,
    secret: [u8; 32],
    committed: bool,
    loaded: bool,
    pending: Option<Challenge>,
}
#[derive(Clone)]
pub(crate) struct Challenge {
    pub(crate) nonce: String,
    pub(crate) token: [u8; 32],
}
impl Bootstrap {
    fn new() -> Self {
        Self {
            receiver: format!("__splitty_bootstrap_{:032x}", rand::random::<u128>()),
            secret: rand::random(),
            committed: false,
            loaded: false,
            pending: None,
        }
    }
    fn revoke(&mut self) {
        self.committed = false;
        self.loaded = false;
        self.pending = None;
    }
    fn authenticates(&self, secret: &[u8; 32]) -> bool {
        use subtle::ConstantTimeEq;
        bool::from(self.secret.ct_eq(secret))
    }
}
struct Entry {
    window: String,
    document: u64,
    request: u32,
    context: queue::Context,
    plan: Plan,
    opening: bool,
    revoked: bool,
}
#[derive(Default)]
struct Registry {
    windows: HashMap<String, Window>,
    entries: HashMap<Handle, Entry>,
    next_document: u64,
    opening: usize,
    closed: bool,
}
impl Registry {
    fn window(&self, label: &str, proof: &Proof) -> Result<&Window, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let window = self.windows.get(label).ok_or(Error::Denied)?;
        if !window
            .proof
            .as_ref()
            .is_some_and(|current| current.matches(proof))
        {
            return Err(Error::Denied);
        }
        Ok(window)
    }
    fn current(&self, handle: Handle) -> bool {
        self.entries.get(&handle).is_some_and(|entry| {
            !entry.revoked
                && !entry.context.is_closed()
                && self
                    .windows
                    .get(&entry.window)
                    .and_then(|window| window.proof.as_ref())
                    .is_some_and(|proof| proof.document == entry.document)
        })
    }
    fn revoke(&mut self, label: &str) {
        let Some(window) = self.windows.get_mut(label) else {
            return;
        };
        window.proof = None;
        window.bootstrap.revoke();
        for handle in &window.attempts {
            if let Some(entry) = self.entries.get_mut(handle) {
                entry.revoked = true;
                entry.context.close();
            }
        }
    }
    fn release(&mut self, handle: Handle) {
        if let Some(entry) = self.entries.remove(&handle) {
            entry.context.close();
            if entry.opening {
                self.opening -= 1;
            }
            if let Some(window) = self.windows.get_mut(&entry.window) {
                window.attempts.remove(&handle);
            }
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            entry.context.close();
        }
    }
}

#[derive(Default)]
pub(crate) struct Broker {
    registry: Arc<Mutex<Registry>>,
}
impl Broker {
    /// Native-only registration. Labels are supplied by the window builder,
    /// never by a request body, URL parameter, origin or claimed role.
    pub(crate) fn register(&self, label: &str, policy: Policy) -> Result<(), Error> {
        if label.is_empty() || label.len() > 128 {
            return Err(Error::Invalid);
        }
        if matches!(&policy, Policy::Remote { target, .. } if target.as_str().len() > 4096) {
            return Err(Error::Invalid);
        }
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        if registry.windows.contains_key(label) {
            return Err(Error::Conflict);
        }
        if registry.windows.len() >= admission::STREAMS {
            return Err(Error::Busy);
        }
        // Native label reuse must not reset the quota of cleanup still in
        // flight for a destroyed predecessor.
        let attempts = registry
            .entries
            .iter()
            .filter_map(|(handle, entry)| (entry.window == label).then_some(*handle))
            .collect();
        registry.windows.insert(
            label.to_owned(),
            Window {
                policy,
                proof: None,
                attempts,
                bootstrap: Bootstrap::new(),
            },
        );
        Ok(())
    }

    pub(crate) fn registered_policy(&self, label: &str) -> Result<Policy, Error> {
        let registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        Ok(registry
            .windows
            .get(label)
            .ok_or(Error::Denied)?
            .policy
            .clone())
    }
    pub(crate) fn bootstrap_receiver(&self, label: &str) -> Result<String, Error> {
        let registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        Ok(registry
            .windows
            .get(label)
            .ok_or(Error::Denied)?
            .bootstrap
            .receiver
            .clone())
    }
    pub(crate) fn authorize(&self, label: &str, proof: &Proof) -> Result<(), Error> {
        self.registry
            .lock()
            .expect("native document registry")
            .window(label, proof)
            .map(|_| ())
    }
    pub(crate) fn policy(&self, label: &str, proof: &Proof) -> Result<Policy, Error> {
        Ok(self
            .registry
            .lock()
            .expect("native document registry")
            .window(label, proof)?
            .policy
            .clone())
    }
    /// Linearize a small native commit against document revocation. The
    /// closure must not call the broker or await; it may only update the
    /// caller's native registry under the established broker-first lock order.
    pub(crate) fn with_authority<R>(
        &self,
        label: &str,
        proof: &Proof,
        commit: impl FnOnce() -> R,
    ) -> Result<R, Error> {
        let registry = self.registry.lock().expect("native document registry");
        registry.window(label, proof)?;
        Ok(commit())
    }
    /// Native callback only; never exposed as a command accepting a label.
    pub(crate) fn current_document(&self, label: &str) -> Option<u64> {
        let registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return None;
        }
        registry
            .windows
            .get(label)?
            .proof
            .as_ref()
            .map(|proof| proof.document)
    }
    pub(crate) fn bootstrap_key(&self, label: &str) -> Result<[u8; 32], Error> {
        let registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        Ok(registry
            .windows
            .get(label)
            .ok_or(Error::Denied)?
            .bootstrap
            .secret)
    }
    /// The initialization capsule names its random document nonce. The native
    /// Finished callback and an eval into the current main document must both
    /// agree before a proof can be issued. At most one pending nonce is kept.
    pub(crate) fn bootstrap_begin(
        &self,
        label: &str,
        secret: &[u8; 32],
        nonce: &str,
    ) -> Result<Option<Challenge>, Error> {
        if nonce.len() != 64 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid);
        }
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        let window = registry.windows.get_mut(label).ok_or(Error::Denied)?;
        if !window.bootstrap.authenticates(secret)
            || window.proof.is_some()
            || window.bootstrap.pending.is_some()
        {
            return Err(Error::Denied);
        }
        let challenge = Challenge {
            nonce: nonce.to_owned(),
            token: rand::random(),
        };
        window.bootstrap.pending = Some(challenge.clone());
        Ok(window.bootstrap.loaded.then_some(challenge))
    }
    /// Wry reports Started at native main-document commit. Keep the nonce
    /// that its document-created script may already have registered.
    pub(crate) fn bootstrap_committed(&self, label: &str) -> Result<(), Error> {
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        let window = registry.windows.get_mut(label).ok_or(Error::Denied)?;
        window.bootstrap.committed = true;
        Ok(())
    }
    pub(crate) fn bootstrap_loaded(&self, label: &str) -> Result<Option<Challenge>, Error> {
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        let window = registry.windows.get_mut(label).ok_or(Error::Denied)?;
        if window.proof.is_some() || !window.bootstrap.committed {
            return Ok(None);
        }
        window.bootstrap.loaded = true;
        Ok(window.bootstrap.pending.clone())
    }
    pub(crate) fn bootstrap_confirm(
        &self,
        label: &str,
        secret: &[u8; 32],
        nonce: &str,
        token: &[u8; 32],
    ) -> Result<(Proof, Policy), Error> {
        use subtle::ConstantTimeEq;
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        let window = registry.windows.get(label).ok_or(Error::Denied)?;
        let pending = window.bootstrap.pending.as_ref().ok_or(Error::Denied)?;
        if !window.bootstrap.loaded
            || !window.bootstrap.authenticates(secret)
            || pending.nonce != nonce
            || !bool::from(pending.token.ct_eq(token))
            || window.proof.is_some()
        {
            return Err(Error::Denied);
        }
        let document = registry.next_document.checked_add(1).ok_or(Error::Closed)?;
        registry.next_document = document;
        let window = registry.windows.get_mut(label).expect("registered window");
        let proof = Proof {
            document,
            secret: rand::random(),
        };
        window.bootstrap.pending = None;
        window.proof = Some(proof.clone());
        Ok((proof, window.policy.clone()))
    }

    /// Invoke only after an allowed main-document commit (or confirmed aborted
    /// navigation). A denied navigation never calls this or `navigating`.
    #[cfg(test)]
    pub(crate) fn committed(&self, label: &str) -> Result<Proof, Error> {
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        if !registry.windows.contains_key(label) {
            return Err(Error::Denied);
        }
        let document = registry.next_document.checked_add(1).ok_or(Error::Closed)?;
        registry.revoke(label);
        registry.next_document = document;
        let proof = Proof {
            document,
            secret: rand::random(),
        };
        registry
            .windows
            .get_mut(label)
            .expect("registered window")
            .proof = Some(proof.clone());
        Ok(proof)
    }
    pub(crate) fn navigating(&self, label: &str) {
        self.registry
            .lock()
            .expect("native document registry")
            .revoke(label);
    }
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn replace(&self, label: &str, policy: Policy) -> Result<(), Error> {
        if matches!(&policy, Policy::Remote { target, .. } if target.as_str().len() > 4096) {
            return Err(Error::Invalid);
        }
        let mut registry = self.registry.lock().expect("native document registry");
        if registry.closed {
            return Err(Error::Closed);
        }
        if !registry.windows.contains_key(label) {
            return Err(Error::Denied);
        }
        registry.revoke(label);
        registry
            .windows
            .get_mut(label)
            .expect("registered window")
            .policy = policy;
        Ok(())
    }
    pub(crate) fn destroyed(&self, label: &str) {
        let mut registry = self.registry.lock().expect("native document registry");
        registry.revoke(label);
        registry.windows.remove(label);
    }
    pub(crate) fn shutdown(&self) {
        let mut registry = self.registry.lock().expect("native document registry");
        registry.closed = true;
        for window in registry.windows.values_mut() {
            window.proof = None;
        }
        for entry in registry.entries.values_mut() {
            entry.revoked = true;
            entry.context.close();
        }
    }
    pub(crate) fn main_action(&self, label: &str, proof: &Proof) -> Result<(), Error> {
        let registry = self.registry.lock().expect("native document registry");
        if matches!(registry.window(label, proof)?.policy, Policy::Main { .. }) {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }
    pub(crate) fn setup_action(&self, label: &str, proof: &Proof) -> Result<(), Error> {
        let registry = self.registry.lock().expect("native document registry");
        if matches!(registry.window(label, proof)?.policy, Policy::Setup) {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }

    /// Synchronous admission precedes creating an async setup task. The caller
    /// transfers this context to its connector, and must retain `Attempt` until
    /// that connector's real cleanup completes, including worker exit.
    pub(crate) fn open(
        &self,
        label: &str,
        proof: &Proof,
        request: u32,
        target: Option<&str>,
        update: bool,
    ) -> Result<Attempt, Error> {
        if request == 0
            || target.is_some_and(|value| value.len() > 4096)
            || (update && target.is_none())
        {
            return Err(Error::Invalid);
        }
        let mut registry = self.registry.lock().expect("native document registry");
        let window = registry.window(label, proof)?;
        let backend = match &window.policy {
            Policy::Setup => return Err(Error::Denied),
            Policy::Remote { .. } if target.is_some() || update => return Err(Error::Denied),
            Policy::Main { backend } | Policy::Remote { backend, .. } => *backend,
        };
        if window
            .attempts
            .iter()
            .filter_map(|handle| registry.entries.get(handle))
            .any(|entry| entry.document == proof.document && entry.request == request)
        {
            return Err(Error::Conflict);
        }
        if window.attempts.len() >= admission::DOMAIN_STREAMS
            || registry.entries.len() >= admission::STREAMS
            || registry
                .entries
                .values()
                .filter(|entry| entry.plan.backend == backend)
                .count()
                >= admission::BACKEND_STREAMS
            || registry.opening >= admission::OPENING
            || window
                .attempts
                .iter()
                .filter_map(|handle| registry.entries.get(handle))
                .filter(|entry| entry.opening)
                .count()
                >= admission::DOMAIN_OPENING
        {
            return Err(Error::Busy);
        }
        let plan = match &window.policy {
            Policy::Main { backend } => Plan {
                backend: *backend,
                target: target
                    .map(SshTarget::parse)
                    .transpose()
                    .map_err(|_| Error::Invalid)?,
                update,
            },
            Policy::Remote { backend, target } => Plan {
                backend: *backend,
                target: Some(target.clone()),
                update: false,
            },
            Policy::Setup => return Err(Error::Denied),
        };
        let handle = loop {
            let handle = Handle(rand::random());
            if !registry.entries.contains_key(&handle) {
                break handle;
            }
        };
        let context = queue::Context::default();
        registry.entries.insert(
            handle,
            Entry {
                window: label.to_owned(),
                document: proof.document,
                request,
                context: context.clone(),
                plan: plan.clone(),
                opening: true,
                revoked: false,
            },
        );
        registry
            .windows
            .get_mut(label)
            .expect("authorized window")
            .attempts
            .insert(handle);
        registry.opening += 1;
        Ok(Attempt {
            registry: Arc::downgrade(&self.registry),
            handle,
            context,
            plan,
        })
    }
    pub(crate) fn connection(
        &self,
        label: &str,
        proof: &Proof,
        handle: Handle,
    ) -> Result<queue::Context, Error> {
        let registry = self.registry.lock().expect("native document registry");
        registry.window(label, proof)?;
        let entry = registry.entries.get(&handle).ok_or(Error::Denied)?;
        if entry.opening
            || entry.window != label
            || entry.document != proof.document
            || !registry.current(handle)
        {
            return Err(Error::Denied);
        }
        Ok(entry.context.clone())
    }
    pub(crate) fn cancel(&self, label: &str, proof: &Proof, handle: Handle) -> Result<(), Error> {
        let mut registry = self.registry.lock().expect("native document registry");
        registry.window(label, proof)?;
        let entry = registry.entries.get_mut(&handle).ok_or(Error::Denied)?;
        if entry.window != label || entry.document != proof.document {
            return Err(Error::Denied);
        }
        entry.revoked = true;
        entry.context.close();
        // Cleanup, not this request, returns its pending/live capacity.
        Ok(())
    }
    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let registry = self.registry.lock().expect("native document registry");
        (registry.entries.len(), registry.opening)
    }
}

/// Owned by native setup/session work, never by a renderer request. A late
/// completion must pass `ready` and each delivery must recheck `current`.
pub(crate) struct Attempt {
    registry: Weak<Mutex<Registry>>,
    handle: Handle,
    pub(crate) context: queue::Context,
    pub(crate) plan: Plan,
}
impl Attempt {
    pub(crate) fn handle(&self) -> Handle {
        self.handle
    }
    pub(crate) fn current(&self) -> bool {
        self.registry.upgrade().is_some_and(|registry| {
            registry
                .lock()
                .expect("native document registry")
                .current(self.handle)
        })
    }
    pub(crate) fn live(&self) -> bool {
        self.registry.upgrade().is_some_and(|registry| {
            let registry = registry.lock().expect("native document registry");
            registry.current(self.handle)
                && registry
                    .entries
                    .get(&self.handle)
                    .is_some_and(|entry| !entry.opening)
        })
    }

    pub(crate) fn ready(&self, ready: &wire::Ready, expected_mux: u32) -> Result<(), Error> {
        let registry = self.registry.upgrade().ok_or(Error::Closed)?;
        let mut registry = registry.lock().expect("native document registry");
        if !registry.current(self.handle) {
            return Err(Error::Closed);
        }
        let entry = registry
            .entries
            .get_mut(&self.handle)
            .ok_or(Error::Closed)?;
        if !entry.opening {
            return Err(Error::Conflict);
        }
        if negotiation::validate_ready(ready, None, &entry.plan.purpose(), expected_mux).is_err() {
            entry.revoked = true;
            entry.context.close();
            return Err(Error::Invalid);
        }
        entry.opening = false;
        registry.opening -= 1;
        Ok(())
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        self.context.close();
        if let Some(registry) = self.registry.upgrade() {
            registry
                .lock()
                .expect("native document registry")
                .release(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bootstrap_requires_current_loaded_document_challenge() {
        let broker = super::Broker::default();
        broker
            .register(
                "main",
                super::Policy::Main {
                    backend: super::BackendId::new(),
                },
            )
            .unwrap();
        let key = broker.bootstrap_key("main").unwrap();
        let nonce = "a".repeat(64);
        assert!(broker.bootstrap_begin("main", &[0; 32], &nonce).is_err());
        assert!(broker
            .bootstrap_begin("main", &key, &nonce)
            .unwrap()
            .is_none());
        assert!(broker.bootstrap_loaded("main").unwrap().is_none());
        broker.bootstrap_committed("main").unwrap();
        let challenge = broker.bootstrap_loaded("main").unwrap().unwrap();
        assert!(broker
            .bootstrap_confirm("main", &key, &"b".repeat(64), &challenge.token)
            .is_err());
        broker.navigating("main");
        assert!(broker
            .bootstrap_confirm("main", &key, &nonce, &challenge.token)
            .is_err());
        assert!(broker.bootstrap_loaded("main").unwrap().is_none());
        broker.bootstrap_committed("main").unwrap();
        broker.bootstrap_loaded("main").unwrap();
        let next = broker
            .bootstrap_begin("main", &key, &"b".repeat(64))
            .unwrap()
            .unwrap();
        assert!(broker
            .bootstrap_confirm("main", &key, &nonce, &challenge.token)
            .is_err());
        let (proof, _) = broker
            .bootstrap_confirm("main", &key, &next.nonce, &next.token)
            .unwrap();
        assert!(broker.authorize("main", &proof).is_ok());
        assert!(broker
            .bootstrap_confirm("main", &key, &next.nonce, &next.token)
            .is_err());
        assert!(broker.bootstrap_begin("main", &key, &nonce).is_err());
        broker.navigating("main");
        assert!(broker.authorize("main", &proof).is_err());
        assert_eq!(broker.current_document("main"), None);
        broker.shutdown();
        assert!(broker.bootstrap_loaded("main").is_err());
    }

    use super::*;

    fn local() -> Policy {
        Policy::Main {
            backend: BackendId::new(),
        }
    }
    fn ready(target: Option<&str>) -> wire::Ready {
        wire::Ready {
            generation: 1,
            instance: "a".repeat(32),
            mux_protocol: 1,
            purpose: target.map_or(wire::Purpose::Direct, |target| wire::Purpose::Host {
                target: target.into(),
            }),
            limits: wire::Limits::default(),
        }
    }
    fn window(broker: &Broker, label: &str, policy: Policy) -> Proof {
        broker.register(label, policy).unwrap();
        broker.committed(label).unwrap()
    }
    fn open_error(result: Result<Attempt, Error>) -> Error {
        match result {
            Ok(_) => panic!("open unexpectedly admitted"),
            Err(error) => error,
        }
    }

    #[test]
    fn native_policy_and_private_proof_define_authority_not_public_ids_or_copied_handles() {
        let broker = Broker::default();
        let main = window(&broker, "main", local());
        let backend = BackendId::new();
        let remote = window(
            &broker,
            "remote-1",
            Policy::Remote {
                backend,
                target: SshTarget::parse("vm").unwrap(),
            },
        );
        let setup = window(&broker, "setup", Policy::Setup);
        let forged = Proof::from_request(main.document, [0; 32]);
        assert_eq!(broker.main_action("main", &forged), Err(Error::Denied));
        assert_eq!(broker.main_action("remote-1", &main), Err(Error::Denied));
        assert_eq!(broker.main_action("remote-1", &remote), Err(Error::Denied));
        assert_eq!(broker.main_action("setup", &setup), Err(Error::Denied));
        assert_eq!(broker.setup_action("main", &main), Err(Error::Denied));
        assert_eq!(broker.setup_action("setup", &setup), Ok(()));
        assert_eq!(
            broker.main_action("main", &Proof::from_request(main.document, *main.secret())),
            Ok(())
        );
        assert_eq!(
            open_error(broker.open("popout", &main, 1, None, false)),
            Error::Denied
        );
        assert_eq!(
            open_error(broker.open("setup", &setup, 1, None, false)),
            Error::Denied
        );
        assert_eq!(
            open_error(broker.open("remote-1", &remote, 1, Some("vm"), false)),
            Error::Denied
        );
        let attempt = broker.open("remote-1", &remote, 1, None, false).unwrap();
        assert_eq!(attempt.plan.backend, backend);
        assert_eq!(attempt.plan.target.as_ref().unwrap().as_str(), "vm");
        assert!(!attempt.plan.update);
        attempt.ready(&ready(Some("vm")), 1).unwrap();
        let copied = Handle::from_request(*attempt.handle().bytes());
        assert!(broker.connection("main", &main, copied).is_err());
        assert!(broker.connection("remote-1", &remote, copied).is_ok());
        assert_eq!(broker.cancel("main", &main, copied), Err(Error::Denied));
        assert!(attempt.current());
        let host = broker
            .open("main", &main, 2, Some("another-vm"), true)
            .unwrap();
        assert_eq!(host.plan.target.as_ref().unwrap().as_str(), "another-vm");
        assert!(host.plan.update);
        host.ready(&ready(Some("another-vm")), 1).unwrap();
    }

    #[test]
    fn navigation_rejects_stale_proofs_late_ready_and_delivery_without_returning_capacity() {
        let broker = Broker::default();
        let proof = window(&broker, "main", local());
        let mut attempts: Vec<_> = (1..=4)
            .map(|id| broker.open("main", &proof, id, None, false).unwrap())
            .collect();
        assert!(broker
            .connection("main", &proof, attempts[0].handle())
            .is_err());
        let old_context = attempts[0].context.clone();
        broker.navigating("main");
        assert!(old_context.is_closed());
        for attempt in &attempts {
            assert!(!attempt.current());
            assert_eq!(attempt.ready(&ready(None), 1), Err(Error::Closed));
        }
        assert_eq!(broker.main_action("main", &proof), Err(Error::Denied));
        let next = broker.committed("main").unwrap();
        assert_ne!(next.document, proof.document);
        assert_eq!(broker.main_action("main", &proof), Err(Error::Denied));
        assert!(broker
            .connection("main", &next, attempts[0].handle())
            .is_err());
        assert_eq!(
            open_error(broker.open("main", &next, 1, None, false)),
            Error::Busy
        );
        assert_eq!(broker.counts(), (4, 4));
        attempts.pop();
        let admitted = broker.open("main", &next, 1, None, false).unwrap();
        admitted.ready(&ready(None), 1).unwrap();
        assert_eq!(broker.counts(), (4, 3));
        drop((admitted, attempts));
        assert_eq!(broker.counts(), (0, 0));
    }

    #[test]
    fn cancelling_a_pending_open_needs_actual_cleanup_before_its_slot_returns() {
        let broker = Broker::default();
        let proof = window(&broker, "main", local());
        let attempts: Vec<_> = (1..=4)
            .map(|id| broker.open("main", &proof, id, None, false).unwrap())
            .collect();
        for attempt in &attempts {
            broker.cancel("main", &proof, attempt.handle()).unwrap();
        }
        assert_eq!(
            open_error(broker.open("main", &proof, 5, None, false)),
            Error::Busy
        );
        assert_eq!(broker.counts(), (4, 4));
        drop(attempts);
        assert_eq!(broker.counts(), (0, 0));
        let attempt = broker.open("main", &proof, 5, None, false).unwrap();
        attempt.ready(&ready(None), 1).unwrap();
        assert_eq!(attempt.ready(&ready(None), 1), Err(Error::Conflict));
        assert_eq!(broker.counts(), (1, 0));
        broker.cancel("main", &proof, attempt.handle()).unwrap();
        assert_eq!(broker.counts(), (1, 0));
        drop(attempt);
        assert_eq!(broker.counts(), (0, 0));
    }

    #[test]
    fn document_and_process_limits_hold_across_live_streams_and_reload() {
        let broker = Broker::default();
        let mut attempts = Vec::new();
        let mut proofs = Vec::new();
        for window_id in 0..4 {
            let label = format!("window-{window_id}");
            let proof = window(&broker, &label, local());
            for id in 1..=32 {
                let attempt = broker.open(&label, &proof, id, None, false).unwrap();
                attempt.ready(&ready(None), 1).unwrap();
                attempts.push(attempt);
            }
            assert_eq!(
                open_error(broker.open(&label, &proof, 33, None, false)),
                Error::Busy
            );
            proofs.push(proof);
        }
        assert_eq!(broker.counts(), (128, 0));
        let fifth = window(&broker, "fifth", local());
        assert_eq!(
            open_error(broker.open("fifth", &fifth, 1, None, false)),
            Error::Busy
        );
        let fresh = broker.committed("window-0").unwrap();
        assert_eq!(
            open_error(broker.open("window-0", &fresh, 33, None, false)),
            Error::Busy
        );
        assert!(attempts.iter().take(32).all(|attempt| !attempt.current()));
        assert!(attempts.iter().skip(32).all(Attempt::current));
        attempts.clear();
        assert_eq!(broker.counts(), (0, 0));
        let mut pending = Vec::new();
        for (window_id, proof) in proofs.iter().enumerate().skip(1) {
            for id in 1..=4 {
                pending.push(
                    broker
                        .open(&format!("window-{window_id}"), proof, id, None, false)
                        .unwrap(),
                );
            }
        }
        for id in 1..=4 {
            pending.push(broker.open("window-0", &fresh, id, None, false).unwrap());
        }
        assert_eq!(broker.counts(), (16, 16));
        assert_eq!(
            open_error(broker.open("fifth", &fifth, 1, None, false)),
            Error::Busy
        );
    }

    #[test]
    fn wrong_backend_purpose_fails_before_live_assignment_and_does_not_touch_other_streams() {
        let broker = Broker::default();
        let proof = window(&broker, "main", local());
        let local = broker.open("main", &proof, 1, None, false).unwrap();
        let host = broker.open("main", &proof, 2, Some("vm"), false).unwrap();
        assert_eq!(host.ready(&ready(None), 1), Err(Error::Invalid));
        assert!(!host.current());
        assert!(local.current());
        local.ready(&ready(None), 1).unwrap();
        let other = broker.open("main", &proof, 3, None, false).unwrap();
        let mut invalid = ready(None);
        invalid.generation = 0;
        assert_eq!(other.ready(&invalid, 1), Err(Error::Invalid));
        assert_eq!(broker.counts(), (3, 2));
        let incompatible = broker.open("main", &proof, 4, None, false).unwrap();
        assert_eq!(incompatible.ready(&ready(None), 2), Err(Error::Invalid));
    }

    #[test]
    fn replacement_destruction_and_broker_drop_revoke_only_their_own_connections() {
        let broker = Broker::default();
        let main = window(&broker, "main", local());
        let remote = window(
            &broker,
            "remote",
            Policy::Remote {
                backend: BackendId::new(),
                target: SshTarget::parse("vm").unwrap(),
            },
        );
        let old = broker.open("main", &main, 1, None, false).unwrap();
        let other = broker.open("remote", &remote, 1, None, false).unwrap();
        let replacement = BackendId::new();
        broker
            .replace(
                "main",
                Policy::Main {
                    backend: replacement,
                },
            )
            .unwrap();
        assert!(!old.current());
        assert!(other.current());
        assert_eq!(broker.main_action("main", &main), Err(Error::Denied));
        let main = broker.committed("main").unwrap();
        let new = broker.open("main", &main, 1, None, false).unwrap();
        assert_eq!(new.plan.backend, replacement);
        broker.destroyed("main");
        assert!(!new.current());
        assert!(other.current());
        let reused = window(&broker, "main", local());
        let reused_attempt = broker.open("main", &reused, 1, None, false).unwrap();
        drop((old, new));
        assert!(reused_attempt.current());
        assert_eq!(broker.counts(), (2, 2));
        broker.shutdown();
        assert_eq!(broker.committed("main").err(), Some(Error::Closed));
        assert_eq!(broker.register("late", local()), Err(Error::Closed));
        assert_eq!(broker.replace("remote", local()), Err(Error::Closed));
        assert_eq!(broker.main_action("main", &reused), Err(Error::Closed));
        assert_eq!(
            open_error(broker.open("main", &reused, 2, None, false)),
            Error::Closed
        );
        assert!(!other.current() && !reused_attempt.current());
        drop(broker);
        assert!(other.context.is_closed() && reused_attempt.context.is_closed());
        drop((other, reused_attempt));
    }
    #[test]
    fn backend_capacity_and_destroyed_window_capacity_wait_for_cleanup() {
        let broker = Broker::default();
        let backend = BackendId::new();
        let mut attempts = Vec::new();
        for label in ["main", "other"] {
            let proof = window(&broker, label, Policy::Main { backend });
            for request in 1..=32 {
                let attempt = broker.open(label, &proof, request, None, false).unwrap();
                attempt.ready(&ready(None), 1).unwrap();
                attempts.push(attempt);
            }
        }
        let third = window(&broker, "third", Policy::Main { backend });
        assert_eq!(
            open_error(broker.open("third", &third, 1, None, false)),
            Error::Busy
        );
        broker.destroyed("main");
        let main = window(&broker, "main", local());
        assert_eq!(
            open_error(broker.open("main", &main, 1, None, false)),
            Error::Busy
        );
        assert_eq!(
            open_error(broker.open("third", &third, 1, None, false)),
            Error::Busy
        );
        drop(attempts.remove(0));
        let reopened = broker.open("main", &main, 1, None, false).unwrap();
        let other = broker.open("third", &third, 1, None, false).unwrap();
        drop((attempts, reopened, other));
        assert_eq!(broker.counts(), (0, 0));

        let pending: Vec<_> = (1..=4)
            .map(|id| broker.open("main", &main, id, None, false).unwrap())
            .collect();
        broker.destroyed("main");
        let next = window(&broker, "main", local());
        assert_eq!(
            open_error(broker.open("main", &next, 1, None, false)),
            Error::Busy
        );
        drop(pending);
        assert!(broker.open("main", &next, 1, None, false).is_ok());
    }
}
