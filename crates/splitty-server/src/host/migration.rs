//! Explicit legacy migration. Discovery is read-only; the commit revalidates
//! its process token and signals only a retained Linux pidfd. No TCP fallback,
//! keeper claim, session-ending command or kill escalation is used here.
use splitty_core::daemon::DaemonInfo;
use splitty_remote::handshake::{
    valid_migration_token, LegacyMigration, LEGACY_MIGRATION_MESSAGE_LIMIT,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The legacy daemon changed; refresh its status before updating.")]
    Changed,
    #[error("{0}")]
    Blocked(String),
}
impl Error {
    pub fn exit_code(&self) -> i32 {
        if matches!(self, Self::Changed) {
            75
        } else {
            1
        }
    }
}
fn blocked(message: impl std::fmt::Display) -> Error {
    Error::Blocked(message.to_string())
}
fn eligibility(info: &DaemonInfo) -> Result<(), Error> {
    if !info.legacy() || !info.keeps_sessions {
        return Err(blocked("This daemon cannot be migrated while preserving its sessions. Update it explicitly on the host."));
    }
    let parse = |value: &str| semver::Version::parse(value.strip_prefix('v').unwrap_or(value));
    let older = match (parse(&info.version), parse(env!("CARGO_PKG_VERSION"))) {
        (Ok(old), Ok(new)) => old.cmp_precedence(&new).is_lt(),
        _ => false,
    };
    if !older || info.protocol != splitty_core::protocol::PROTOCOL_VERSION {
        return Err(blocked("Only an older daemon with a supported session protocol can be migrated. Update it explicitly on the host."));
    }
    Ok(())
}
fn offer(error: Error) -> LegacyMigration {
    let mut message = error.to_string();
    if message.len() > LEGACY_MIGRATION_MESSAGE_LIMIT {
        let mut end = LEGACY_MIGRATION_MESSAGE_LIMIT;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    LegacyMigration::Blocked { message }
}

/// Advertise only what this installed binary can verify without changing it.
pub async fn probe(info: &DaemonInfo) -> LegacyMigration {
    if let Err(error) = eligibility(info) {
        return offer(error);
    }
    #[cfg(target_os = "linux")]
    {
        match linux::inspect(info).await {
            Ok(proof) => LegacyMigration::Ready {
                token: proof.token(),
                sessions: proof.sessions,
            },
            Err(error) => offer(error),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        offer(blocked("Verified legacy migration is unavailable on this host. Run the newly installed Splitty binary's `daemon restart` on the host without `--end-sessions`, then reconnect."))
    }
}

/// Stop exactly the freshly verified legacy generation. The CLI then invokes
/// its normal start path, which adopts keepers and restores workspace state.
pub async fn migrate(expected: &str) -> Result<(), Error> {
    if !valid_migration_token(expected) {
        return Err(Error::Changed);
    }
    let info = super::running_daemon()
        .map_err(blocked)?
        .ok_or(Error::Changed)?;
    eligibility(&info)?;
    #[cfg(target_os = "linux")]
    {
        let proof = linux::inspect(&info).await?;
        proof.stop(expected).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(blocked("Verified legacy migration is unavailable on this host. Restart the daemon explicitly on the host."))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use http_body_util::{BodyExt, Empty};
    use hyper::{body::Bytes, client::conn::http1::SendRequest, Request};
    use hyper_util::rt::TokioIo;
    use sha2::{Digest, Sha256};
    use splitty_core::{
        daemon,
        keeper::wire::Hello,
        mux::{SessionStatus, WorkspaceState},
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs, io,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::fs::{FileTypeExt, MetadataExt},
        },
        path::{Path, PathBuf},
        time::Duration,
    };
    use tokio::io::AsyncReadExt;

    const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
    const STOP_TIMEOUT: Duration = Duration::from_secs(10);
    const MAX_BODY: usize = 1 << 20;
    const MAX_KEEPERS: usize = 1024;
    const MAX_ENTRIES: usize = 8192;

    struct Process(OwnedFd);
    impl Process {
        fn open(pid: u32) -> Result<Self, Error> {
            let pid = i32::try_from(pid).map_err(|_| Error::Changed)?;
            // SAFETY: pidfd_open receives a validated PID and zero flags.
            let fd = unsafe { nix::libc::syscall(nix::libc::SYS_pidfd_open, pid, 0) };
            if fd < 0 {
                return Err(blocked(format!(
                    "Cannot pin the legacy daemon: {}. Restart it explicitly on the host.",
                    io::Error::last_os_error()
                )));
            }
            let fd = i32::try_from(fd).map_err(|_| blocked("Invalid process descriptor"))?;
            // SAFETY: successful pidfd_open transferred this new descriptor.
            Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
        }
        fn exited(&self) -> Result<bool, Error> {
            let mut event = nix::libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            };
            // SAFETY: event is valid storage for one descriptor, timeout zero.
            let result = unsafe { nix::libc::poll(&mut event, 1, 0) };
            if result < 0 {
                return Err(blocked(io::Error::last_os_error()));
            }
            if event.revents & (nix::libc::POLLERR | nix::libc::POLLNVAL) != 0 {
                return Err(blocked("Cannot observe the pinned daemon"));
            }
            Ok(event.revents & (nix::libc::POLLIN | nix::libc::POLLHUP) != 0)
        }
        fn live(&self) -> Result<(), Error> {
            if self.exited()? {
                Err(Error::Changed)
            } else {
                Ok(())
            }
        }
        fn terminate(&self) -> Result<(), Error> {
            // SAFETY: the owned pidfd fixes the target generation. No PID-based
            // fallback, process group signal or forced kill is permitted.
            let result = unsafe {
                nix::libc::syscall(
                    nix::libc::SYS_pidfd_send_signal,
                    self.0.as_raw_fd(),
                    nix::libc::SIGTERM,
                    std::ptr::null::<nix::libc::siginfo_t>(),
                    0,
                )
            };
            if result < 0 {
                return Err(blocked(io::Error::last_os_error()));
            }
            Ok(())
        }
    }

    #[derive(Debug, PartialEq, Eq, serde::Serialize)]
    struct Identity {
        uid: u32,
        start: u64,
        executable: (u64, u64),
        namespace: (u64, u64),
        boot: String,
    }
    fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
        use std::io::Read;
        let file = fs::File::open(path).map_err(blocked)?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(blocked)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(blocked("Process identity exceeds its bound"));
        }
        Ok(bytes)
    }
    fn stat(pid: u32) -> Result<(char, u64), Error> {
        let data = read_bounded(&PathBuf::from(format!("/proc/{pid}/stat")), 8192)?;
        let data = std::str::from_utf8(&data).map_err(blocked)?;
        let rest = data
            .rsplit_once(") ")
            .ok_or_else(|| blocked("Invalid process identity"))?
            .1;
        let fields: Vec<_> = rest.split_ascii_whitespace().collect();
        let state = fields
            .first()
            .and_then(|s| s.chars().next())
            .ok_or(Error::Changed)?;
        let start = fields
            .get(19)
            .ok_or(Error::Changed)?
            .parse()
            .map_err(blocked)?;
        Ok((state, start))
    }
    fn identity(pid: u32) -> Result<Identity, Error> {
        let (state, start) = stat(pid)?;
        if state == 'Z' {
            return Err(Error::Changed);
        }
        let data = read_bounded(&PathBuf::from(format!("/proc/{pid}/status")), 16384)?;
        let data = std::str::from_utf8(&data).map_err(blocked)?;
        let uid: u32 = data
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            .ok_or(Error::Changed)?
            .parse()
            .map_err(blocked)?;
        if uid != nix::unistd::geteuid().as_raw() {
            return Err(blocked("The legacy daemon belongs to another user"));
        }
        let executable = fs::metadata(format!("/proc/{pid}/exe")).map_err(blocked)?;
        let namespace = fs::metadata(format!("/proc/{pid}/ns/pid")).map_err(blocked)?;
        let ours = fs::metadata("/proc/self/ns/pid").map_err(blocked)?;
        if (namespace.dev(), namespace.ino()) != (ours.dev(), ours.ino()) {
            return Err(blocked("The daemon's process namespace cannot be verified"));
        }
        let boot = String::from_utf8(read_bounded(
            Path::new("/proc/sys/kernel/random/boot_id"),
            64,
        )?)
        .map_err(blocked)?;
        Ok(Identity {
            uid,
            start,
            executable: (executable.dev(), executable.ino()),
            namespace: (namespace.dev(), namespace.ino()),
            boot,
        })
    }

    fn no_direct_ptys(pid: u32) -> Result<(), Error> {
        // Legacy dispatch can own fallback PTYs that never appeared in mux
        // state. Keeper-backed masters live in keeper processes; a master in
        // the daemon itself cannot survive its shutdown.
        let entries = fs::read_dir(format!("/proc/{pid}/fd")).map_err(blocked)?;
        for (index, entry) in entries.enumerate() {
            if index >= MAX_ENTRIES {
                return Err(blocked("Too many daemon descriptors to verify migration"));
            }
            let metadata = match fs::metadata(entry.map_err(blocked)?.path()) {
                Ok(metadata) => metadata,
                // Ordinary request descriptors can close during inspection.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(blocked(error)),
            };
            if metadata.file_type().is_char_device()
                && nix::libc::major(metadata.rdev()) == 5
                && nix::libc::minor(metadata.rdev()) == 2
            {
                return Err(blocked("The daemon owns a terminal without a keeper. Migration was blocked to avoid ending it; handle that session on the host first."));
            }
        }
        Ok(())
    }

    struct Owner {
        sender: SendRequest<Empty<Bytes>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Owner {
        async fn connect(info: &DaemonInfo) -> Result<Self, Error> {
            let path = Path::new(&info.socket);
            if path != crate::control::daemon_socket_path() {
                return Err(blocked(
                    "The legacy owner endpoint is outside this configuration",
                ));
            }
            // The endpoint verifier accepts only private owner sockets on a
            // local Unix filesystem and checks the inode across connect.
            let stream = crate::native::endpoint::connect(path)
                .await
                .map_err(blocked)?;
            let peer = stream.peer_cred().map_err(blocked)?;
            if peer.uid() != nix::unistd::geteuid().as_raw()
                || peer.pid().and_then(|pid| u32::try_from(pid).ok()) != Some(info.pid)
            {
                return Err(blocked(
                    "The owner socket does not belong to the recorded daemon",
                ));
            }
            let (sender, connection) = hyper::client::conn::http1::Builder::new()
                .max_buf_size(8192)
                .handshake(TokioIo::new(stream))
                .await
                .map_err(blocked)?;
            Ok(Self {
                sender,
                task: tokio::spawn(async move {
                    let _ = connection.await;
                }),
            })
        }
        async fn get<T: serde::de::DeserializeOwned>(
            &mut self,
            path: &'static str,
        ) -> Result<T, Error> {
            let request = Request::builder()
                .uri(path)
                .header("Host", "localhost")
                .body(Empty::<Bytes>::new())
                .map_err(blocked)?;
            let response = self.sender.send_request(request).await.map_err(blocked)?;
            if response.status() != 200 {
                return Err(blocked(
                    "The legacy owner endpoint cannot verify its sessions",
                ));
            }
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(blocked)?;
                if let Some(data) = frame.data_ref() {
                    if bytes.len().saturating_add(data.len()) > MAX_BODY {
                        return Err(blocked(
                            "The legacy workspace exceeds the migration inspection limit",
                        ));
                    }
                    bytes.extend_from_slice(data);
                }
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| blocked("The legacy owner returned unsupported state"))
        }
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Status {
        server_mode: String,
        version: String,
    }

    fn active_sessions(state: &WorkspaceState) -> Result<BTreeSet<String>, Error> {
        let mut ids = BTreeSet::new();
        for screen in state.screens.values() {
            for (id, session) in &screen.sessions {
                if session.status != SessionStatus::Exited
                    && (id.as_str().len() > 256
                        || !ids.insert(id.as_str().to_owned())
                        || ids.len() > MAX_KEEPERS)
                {
                    return Err(blocked("The legacy session inventory cannot be verified"));
                }
            }
        }
        Ok(ids)
    }
    async fn kept_sessions() -> Result<BTreeMap<String, u32>, Error> {
        let dir = splitty_core::keeper::dir::keepers_dir();
        let mut paths = Vec::new();
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for (index, entry) in entries.enumerate() {
                    if index >= MAX_ENTRIES {
                        return Err(blocked(
                            "Too many keeper directory entries to verify migration",
                        ));
                    }
                    let path = entry.map_err(blocked)?.path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "sock")
                    {
                        if paths.len() >= MAX_KEEPERS {
                            return Err(blocked("Too many keeper sockets to verify migration"));
                        }
                        paths.push(path);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(blocked(error)),
        }
        let mut kept = BTreeMap::new();
        for path in paths {
            // No find()/take_over(): discovery must not remove stale sockets or
            // claim a keeper. Only its initial Hello is read before disconnect.
            let mut stream = crate::native::endpoint::connect(&path)
                .await
                .map_err(blocked)?;
            let size =
                usize::try_from(stream.read_u32().await.map_err(blocked)?).map_err(blocked)?;
            if size == 0 || size > splitty_core::keeper::wire::MAX_FRAME {
                return Err(blocked("Invalid keeper greeting size"));
            }
            let mut frame = vec![0; size];
            stream.read_exact(&mut frame).await.map_err(blocked)?;
            if frame[0] != 1 {
                return Err(blocked("Keeper did not provide its identity"));
            }
            let hello: Hello = serde_json::from_slice(&frame[1..])
                .map_err(|_| blocked("Invalid keeper identity"))?;
            if hello.protocol != splitty_core::keeper::wire::PROTOCOL
                || hello.session_id.is_empty()
                || hello.session_id.len() > 256
            {
                return Err(blocked(
                    "A session keeper uses an unsupported identity or protocol",
                ));
            }
            if hello.exited.is_none() {
                let pid = hello
                    .pid
                    .filter(|pid| *pid > 0 && i32::try_from(*pid).is_ok())
                    .ok_or_else(|| blocked("A keeper's running program cannot be verified"))?;
                if stat(pid)?.0 == 'Z' {
                    return Err(blocked(
                        "A keeper's program is still exiting; retry after it settles",
                    ));
                }
                if kept.insert(hello.session_id, pid).is_some() {
                    return Err(blocked("Two keepers claim the same session"));
                }
            }
        }
        Ok(kept)
    }
    fn coverage(active: &BTreeSet<String>, kept: &BTreeMap<String, u32>) -> Result<(), Error> {
        if active.iter().any(|id| !kept.contains_key(id)) {
            return Err(blocked("Some running sessions have no verified keeper. Migration was blocked to avoid ending them; handle those sessions on the host first."));
        }
        Ok(())
    }
    fn unchanged(info: &DaemonInfo) -> Result<(), Error> {
        if daemon::read_info_checked().map_err(blocked)?.as_ref() != Some(info) {
            return Err(Error::Changed);
        }
        Ok(())
    }
    fn replacement_absent(
        expected: &DaemonInfo,
        current: Option<&DaemonInfo>,
    ) -> Result<(), Error> {
        if current.is_some_and(|current| current != expected) {
            return Err(Error::Changed);
        }
        Ok(())
    }

    pub(super) struct Proof {
        process: Process,
        info: DaemonInfo,
        identity: Identity,
        pub(super) sessions: u32,
    }
    impl Proof {
        pub(super) fn token(&self) -> String {
            let bytes =
                serde_json::to_vec(&(&self.info, &self.identity, env!("CARGO_PKG_VERSION")))
                    .expect("scalar migration identity serializes");
            format!("{:x}", Sha256::digest(bytes))
        }
        pub(super) async fn stop(self, expected: &str) -> Result<(), Error> {
            if self.token() != expected {
                return Err(Error::Changed);
            }
            unchanged(&self.info)?;
            if identity(self.info.pid)? != self.identity {
                return Err(Error::Changed);
            }
            no_direct_ptys(self.info.pid)?;
            self.process.live()?;
            self.process.terminate()?;
            let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
            while !self.process.exited()? {
                if tokio::time::Instant::now() >= deadline {
                    return Err(blocked("The checked daemon has not exited. No replacement was started; refresh its status before trying again."));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            // Never overwrite a competing publication observed after this
            // generation exited. Normal native startup also holds its own
            // instance lock and rechecks live discovery before binding.
            let current = daemon::read_info_checked().map_err(blocked)?;
            replacement_absent(&self.info, current.as_ref())?;
            Ok(())
        }
    }
    pub(super) async fn inspect(info: &DaemonInfo) -> Result<Proof, Error> {
        tokio::time::timeout(PROBE_TIMEOUT, inspect_inner(info))
            .await
            .map_err(|_| {
                blocked("Legacy migration inspection timed out; the daemon was left running.")
            })?
    }
    async fn inspect_inner(info: &DaemonInfo) -> Result<Proof, Error> {
        let process = Process::open(info.pid)?;
        let identity = identity(info.pid)?;
        let mut owner = Owner::connect(info).await?;
        // A fresh response over this retained, peer-verified connection proves
        // that the pinned process still serves the selected legacy daemon.
        let status: Status = owner.get("/api/mux/status").await?;
        if status.server_mode != "daemon" || status.version != info.version {
            return Err(blocked(
                "The legacy daemon's owner identity does not match discovery",
            ));
        }
        no_direct_ptys(info.pid)?;
        let state: WorkspaceState = owner.get("/api/mux/state").await?;
        let active = active_sessions(&state)?;
        let kept = kept_sessions().await?;
        coverage(&active, &kept)?;
        // A second live snapshot catches creations during keeper inspection;
        // this remains preflight, not a freeze of the daemon or its programs.
        let latest: WorkspaceState = owner.get("/api/mux/state").await?;
        coverage(&active_sessions(&latest)?, &kept)?;
        no_direct_ptys(info.pid)?;
        unchanged(info)?;
        process.live()?;
        Ok(Proof {
            process,
            info: info.clone(),
            identity,
            sessions: u32::try_from(kept.len()).map_err(blocked)?,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        fn info(pid: u32) -> DaemonInfo {
            DaemonInfo {
                pid,
                format: None,
                client_socket: None,
                instance: None,
                transport_version: None,
                port: Some(9800),
                socket: "/run/example/control.sock".into(),
                started_at: "1".into(),
                version: "0.1.27".into(),
                protocol: 1,
                keeps_sessions: true,
            }
        }

        #[test]
        fn a_running_session_without_a_keeper_blocks_migration() {
            let active = BTreeSet::from(["kept".into(), "fallback".into()]);
            let kept = BTreeMap::from([("kept".into(), 10)]);
            assert!(coverage(&active, &kept).is_err());
            assert!(coverage(&BTreeSet::from(["kept".into()]), &kept).is_ok());
            assert!(coverage(&BTreeSet::new(), &BTreeMap::new()).is_ok());
        }

        #[tokio::test]
        async fn dropping_a_pinned_process_never_signals_it_and_exit_is_observed() {
            let mut child = Child(
                std::process::Command::new("/bin/sleep")
                    .arg("30")
                    .spawn()
                    .unwrap(),
            );
            let handle = Process::open(child.0.id()).unwrap();
            handle.live().unwrap();
            drop(handle);
            assert!(child.0.try_wait().unwrap().is_none());
            let handle = Process::open(child.0.id()).unwrap();
            handle.terminate().unwrap();
            child.0.wait().unwrap();
            assert!(handle.exited().unwrap());
            assert!(
                handle.terminate().is_err(),
                "dead handle cannot signal another process"
            );
        }

        #[tokio::test]
        async fn cancelled_preflight_and_a_stale_token_leave_the_process_running() {
            let mut child = Child(
                std::process::Command::new("/bin/sleep")
                    .arg("30")
                    .spawn()
                    .unwrap(),
            );
            let pid = child.0.id();
            let cancelled = tokio::time::timeout(Duration::from_millis(10), async {
                let _proof = Process::open(pid).unwrap();
                std::future::pending::<()>().await;
            })
            .await;
            assert!(cancelled.is_err());
            assert!(child.0.try_wait().unwrap().is_none());
            let proof = Proof {
                process: Process::open(pid).unwrap(),
                info: info(pid),
                identity: identity(pid).unwrap(),
                sessions: 0,
            };
            let token = proof.token();
            assert!(valid_migration_token(&token));
            assert!(matches!(
                proof.stop(&"0".repeat(64)).await,
                Err(Error::Changed)
            ));
            assert!(child.0.try_wait().unwrap().is_none());
        }

        #[test]
        fn an_unmapped_direct_terminal_blocks_without_signalling_its_owner() {
            use std::io::BufRead;
            let mut child = Child(
                std::process::Command::new("/bin/sh")
                    .args([
                        "-c",
                        "exec 3<>/dev/ptmx; printf 'ready\\n'; exec /bin/sleep 30",
                    ])
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let mut ready = String::new();
            std::io::BufReader::new(child.0.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready, "ready\n");
            let handle = Process::open(child.0.id()).unwrap();
            let error = no_direct_ptys(child.0.id()).unwrap_err();
            assert!(error.to_string().contains("terminal without a keeper"));
            handle.live().unwrap();
            assert!(child.0.try_wait().unwrap().is_none());
        }

        #[test]
        fn competing_publication_is_rejected_after_the_checked_generation_exits() {
            let original = info(100);
            assert!(replacement_absent(&original, None).is_ok());
            assert!(replacement_absent(&original, Some(&original)).is_ok());
            let mut replacement = original.clone();
            replacement.pid += 1;
            assert!(matches!(
                replacement_absent(&original, Some(&replacement)),
                Err(Error::Changed)
            ));
            replacement = original.clone();
            replacement.started_at = "next generation".into();
            assert!(matches!(
                replacement_absent(&original, Some(&replacement)),
                Err(Error::Changed)
            ));
            replacement = original.clone();
            replacement.format = Some(2);
            assert!(matches!(
                replacement_absent(&original, Some(&replacement)),
                Err(Error::Changed)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info() -> DaemonInfo {
        DaemonInfo {
            pid: 1,
            format: None,
            client_socket: None,
            instance: None,
            transport_version: None,
            port: Some(9800),
            socket: "/run/example/control.sock".into(),
            started_at: "1".into(),
            version: "0.1.27".into(),
            protocol: 1,
            keeps_sessions: true,
        }
    }
    #[test]
    fn only_older_compatible_keeper_backed_legacy_is_eligible() {
        assert!(eligibility(&info()).is_ok());
        for version in ["unknown", env!("CARGO_PKG_VERSION"), "999.0.0"] {
            let mut info = info();
            info.version = version.into();
            assert!(eligibility(&info).is_err());
        }
        let mut without = info();
        without.keeps_sessions = false;
        assert!(eligibility(&without).is_err());
        let mut native = info();
        native.format = Some(2);
        assert!(eligibility(&native).is_err());
        let mut protocol = info();
        protocol.protocol = 999;
        assert!(eligibility(&protocol).is_err());
    }
}
