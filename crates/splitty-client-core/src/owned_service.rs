//! Concrete app-owned local backends. The workspace algorithm is platform-neutral;
//! only creation and final ownership cleanup differ.
use crate::{
    api_stream::ApiStream, appearance::ColorScheme, connection::Client,
    local_service::LocalServiceClient,
};
use splitty_remote::owner::Owner;
use splitty_remote::ssh::SshTarget;
use splitty_transport::{budget::Charged, queue, wire};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use uuid::Uuid;

/// Startup target of the Windows workspace. Paths are Linux paths in this
/// distro, never host paths or shell fragments. Without a configuration
/// directory the backend keeps the distribution's default one.
#[derive(Clone, Debug)]
pub struct WslLaunch {
    distro: String,
    backend: String,
    config_dir: Option<String>,
}
impl WslLaunch {
    pub fn new(distro: String, backend: String, config_dir: Option<String>) -> io::Result<Self> {
        if distro.is_empty()
            || distro.len() > 256
            || distro.chars().any(char::is_control)
            || std::iter::once(&backend).chain(&config_dir).any(|path| {
                path.len() > 4096
                    || !path.starts_with('/')
                    || path.len() == 1
                    || path.chars().any(char::is_control)
            })
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "WSL startup requires a nonempty distribution and absolute Linux backend/configuration paths"));
        }
        Ok(Self {
            distro,
            backend,
            config_dir,
        })
    }
    pub fn distro(&self) -> &str {
        &self.distro
    }
    /// The backend asks its SSH questions through its own connection, so it
    /// needs no askpass program from Windows.
    fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "-d".into(),
            self.distro.clone(),
            "--exec".into(),
            "env".into(),
        ];
        if let Some(config_dir) = &self.config_dir {
            arguments.push(format!("SPLITTY_CONFIG_DIR={config_dir}"));
        }
        arguments.extend(
            [
                self.backend.as_str(),
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "embedded",
                "--owner",
            ]
            .map(String::from),
        );
        arguments
    }
}

#[derive(Debug, Default)]
pub enum LocalBackend {
    /// Existing Unix default. Windows must supply WSL explicitly.
    #[default]
    Embedded,
    Wsl(WslLaunch),
    /// A WSL target still being prepared on its own thread.
    Preparing(WslPreparation),
}

/// A phase of a window's start before its workspace is ready, in their order.
/// The WSL phases occur only on Windows without an explicit target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupPhase {
    /// Listing the WSL distributions to choose one.
    FindingDistribution,
    /// Starting the named distribution (WSL boots first when it is not
    /// running) and checking that it can run the backend.
    StartingDistribution(String),
    /// Installing the backend that ships with the app into the distribution.
    InstallingBackend(String),
    /// Starting the local backend, in the named WSL distribution if any.
    StartingBackend(Option<String>),
    /// Opening the workspace's connection to the started backend.
    Connecting,
}

enum PreparationEvent {
    Phase(StartupPhase),
    Done(Result<WslLaunch, String>),
}

/// What a running preparation reports and reads.
pub struct PreparationProgress {
    events: tokio::sync::mpsc::UnboundedSender<PreparationEvent>,
    stop: Arc<AtomicBool>,
}
impl PreparationProgress {
    /// Reports the phase the preparation enters.
    pub fn phase(&self, phase: StartupPhase) {
        let _ = self.events.send(PreparationEvent::Phase(phase));
    }
    /// Set once nobody waits any more: the preparation ends its child
    /// processes and returns.
    pub fn stop(&self) -> &AtomicBool {
        &self.stop
    }
}

/// The WSL launch target, prepared on a thread of its own. It is started
/// before the window opens, so WSL boots while the window is created; its
/// phases reach the window through the workspace worker.
pub struct WslPreparation {
    events: tokio::sync::mpsc::UnboundedReceiver<PreparationEvent>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl std::fmt::Debug for WslPreparation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WslPreparation")
            .finish_non_exhaustive()
    }
}

/// One step of a preparation as the worker sees it.
#[derive(Debug)]
pub(crate) enum Prepared {
    Phase(StartupPhase),
    Ready(WslLaunch),
}

impl WslPreparation {
    /// Runs `prepare` on a new thread. It may block on `wsl.exe` and on
    /// dialogs; it reports its phases and watches the stop flag.
    pub fn spawn(
        prepare: impl FnOnce(&PreparationProgress) -> Result<WslLaunch, String> + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let progress = PreparationProgress {
            events: sender,
            stop: stop.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("splitty-wsl-preparation".into())
            .spawn(move || {
                let result = prepare(&progress);
                let _ = progress.events.send(PreparationEvent::Done(result));
            })?;
        Ok(Self {
            events,
            stop,
            thread: Some(thread),
        })
    }

    /// The next phase or the prepared target. A failed preparation returns
    /// its message as the error. `stop` ends the preparation and waits for
    /// its thread, so no child process of it outlives the worker.
    pub(crate) async fn next(&mut self, stop: &AtomicBool) -> io::Result<Prepared> {
        loop {
            if stop.load(Ordering::Acquire) {
                self.stop.store(true, Ordering::Release);
                self.join().await;
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "workspace startup cancelled",
                ));
            }
            tokio::select! {
                event = self.events.recv() => {
                    return match event {
                        Some(PreparationEvent::Phase(phase)) => Ok(Prepared::Phase(phase)),
                        Some(PreparationEvent::Done(result)) => {
                            self.join().await;
                            result.map(Prepared::Ready).map_err(io::Error::other)
                        }
                        None => {
                            self.join().await;
                            Err(io::Error::other("The WSL preparation ended without a result"))
                        }
                    };
                }
                _ = tokio::time::sleep(Duration::from_millis(16)) => {}
            }
        }
    }

    /// Ends the preparation and waits for its thread.
    pub(crate) async fn cancel(mut self) {
        self.stop.store(true, Ordering::Release);
        self.join().await;
    }

    async fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
    }
}
impl Drop for WslPreparation {
    fn drop(&mut self) {
        // Not joined here: a drop may happen on the UI thread.
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) enum OwnedService {
    #[cfg(not(target_os = "windows"))]
    Embedded(crate::EmbeddedService),
    Owned(Arc<Owner>),
}
#[cfg(not(target_os = "windows"))]
impl From<crate::EmbeddedService> for OwnedService {
    fn from(service: crate::EmbeddedService) -> Self {
        Self::Embedded(service)
    }
}
impl OwnedService {
    pub async fn start(backend: LocalBackend, stop: &AtomicBool) -> io::Result<Self> {
        match backend {
            LocalBackend::Preparing(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the WSL target is still being prepared",
            )),
            #[cfg(not(target_os = "windows"))]
            LocalBackend::Embedded => Ok(crate::EmbeddedService::start().into()),
            #[cfg(target_os = "windows")]
            LocalBackend::Embedded => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows workspace requires a WSL backend",
            )),
            LocalBackend::Wsl(launch) => {
                let arguments = launch.arguments();
                let mut command = tokio::process::Command::new("wsl.exe");
                #[cfg(target_os = "windows")]
                command.creation_flags(0x08000000); // CREATE_NO_WINDOW, no extra console
                command.args(arguments);
                Self::start_owner(command, stop).await
            }
        }
    }
    async fn start_owner(command: tokio::process::Command, stop: &AtomicBool) -> io::Result<Self> {
        let context = queue::Context::default();
        if stop.load(Ordering::Acquire) {
            context.close();
        }
        let opening = Owner::spawn_context(
            command,
            splitty_core::protocol::PROTOCOL_VERSION,
            context.clone(),
        );
        tokio::pin!(opening);
        // Closing the preparation context wakes the existing bridge machinery;
        // retain and await the opening future so child reaping cannot be abandoned.
        loop {
            tokio::select! {
                result = &mut opening => {
                    let owner = result?;
                    if stop.load(Ordering::Acquire) {
                        owner.close_result().await?;
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "workspace startup cancelled"));
                    }
                    return Ok(Self::Owned(owner));
                }
                _ = tokio::time::sleep(Duration::from_millis(16)) => {
                    if stop.load(Ordering::Acquire) { context.close(); }
                }
            }
        }
    }
    pub async fn connect(&self) -> io::Result<Client> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(service) => service.connect().await,
            Self::Owned(owner) => owner
                .open_context(None, false, queue::Context::default(), |_| {})
                .await
                .map(Client::from_owned),
        }
    }
    pub async fn host_stream(
        &self,
        target: SshTarget,
        update: bool,
        context: queue::Context,
        progress: impl FnMut(Charged<wire::Progress>),
    ) -> io::Result<Client> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(service) => service.host_stream(target, update, context, progress).await,
            Self::Owned(owner) => owner
                .open_context(Some(target), update, context, progress)
                .await
                .map(Client::from_owned),
        }
    }
    pub async fn local_service_client(
        &self,
        connection_id: Uuid,
        scheme: ColorScheme,
    ) -> io::Result<LocalServiceClient> {
        Ok(LocalServiceClient::new(
            self.api_stream().await?,
            connection_id,
            scheme,
        ))
    }
    pub(crate) async fn api_stream(&self) -> io::Result<ApiStream> {
        self.api_stream_with_context(queue::Context::default())
            .await
    }
    pub(crate) async fn api_stream_with_context(
        &self,
        context: queue::Context,
    ) -> io::Result<ApiStream> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(service) => service.api_stream_with_context(context).await,
            Self::Owned(owner) => owner
                .open_context(None, false, context, |_| {})
                .await
                .map(ApiStream::Owned),
        }
    }
    pub async fn close(self) -> io::Result<()> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::Embedded(service) => service.close().await,
            Self::Owned(owner) => owner.close_result().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_arguments_preserve_values_without_shell_interpolation() {
        let launch = WslLaunch::new(
            "Distro with spaces".into(),
            "/home/a/backend $x".into(),
            Some("/tmp/settings ' preview".into()),
        )
        .unwrap();
        assert_eq!(
            launch.arguments(),
            [
                "-d",
                "Distro with spaces",
                "--exec",
                "env",
                "SPLITTY_CONFIG_DIR=/tmp/settings ' preview",
                "/home/a/backend $x",
                "desktop-backend",
                "--transport",
                "native-v1",
                "--backend",
                "embedded",
                "--owner"
            ]
        );
        let default_config = WslLaunch::new("D".into(), "/home/a/splitty".into(), None).unwrap();
        assert_eq!(default_config.arguments()[3..5], ["env", "/home/a/splitty"]);
        assert!(!default_config
            .arguments()
            .iter()
            .any(|argument| argument.contains("ASKPASS")));
    }

    fn sample_launch() -> WslLaunch {
        WslLaunch::new("Ubuntu".into(), "/home/a/splitty".into(), None).unwrap()
    }

    #[tokio::test]
    async fn a_preparation_reports_its_phases_then_its_target_or_its_failure() {
        let stop = AtomicBool::new(false);
        let mut preparation = WslPreparation::spawn(|progress| {
            progress.phase(StartupPhase::StartingDistribution("Ubuntu".into()));
            progress.phase(StartupPhase::InstallingBackend("Ubuntu".into()));
            Ok(sample_launch())
        })
        .unwrap();
        let mut phases = Vec::new();
        let launch = loop {
            match preparation.next(&stop).await.unwrap() {
                Prepared::Phase(phase) => phases.push(phase),
                Prepared::Ready(launch) => break launch,
            }
        };
        assert_eq!(launch.distro(), "Ubuntu");
        assert_eq!(
            phases,
            [
                StartupPhase::StartingDistribution("Ubuntu".into()),
                StartupPhase::InstallingBackend("Ubuntu".into())
            ]
        );
        assert!(preparation.thread.is_none(), "the thread was joined");

        let mut failing =
            WslPreparation::spawn(|_| Err("WSL is not installed.".to_owned())).unwrap();
        let error = failing.next(&stop).await.unwrap_err();
        assert_eq!(error.to_string(), "WSL is not installed.");
    }

    #[tokio::test]
    async fn a_stopped_worker_stops_the_preparation_and_waits_for_it() {
        let (started, ready) = std::sync::mpsc::channel();
        let ended = Arc::new(AtomicBool::new(false));
        let observed = ended.clone();
        let mut preparation = WslPreparation::spawn(move |progress| {
            let _ = started.send(());
            while !progress.stop().load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            observed.store(true, Ordering::Release);
            Err("The start was cancelled.".to_owned())
        })
        .unwrap();
        ready.recv().unwrap();
        let stop = AtomicBool::new(true);
        let error = tokio::time::timeout(Duration::from_secs(5), preparation.next(&stop))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(ended.load(Ordering::Acquire), "joined before returning");
    }

    #[test]
    fn dropping_a_preparation_tells_it_to_stop() {
        let (sender, stopped) = std::sync::mpsc::channel();
        let preparation = WslPreparation::spawn(move |progress| {
            while !progress.stop().load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = sender.send(());
            Err("stopped".to_owned())
        })
        .unwrap();
        drop(preparation);
        stopped.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn incomplete_or_ambiguous_targets_are_rejected_before_spawn() {
        for (distro, binary, config) in [
            ("", "/bin/s", "/tmp/c"),
            ("D", "bin/s", "/tmp/c"),
            ("D", "/bin/s", ""),
            ("D", "/bin/s", "C:\\tmp"),
            ("D\0", "/bin/s", "/tmp/c"),
            ("D", "/bin/s\n", "/tmp/c"),
        ] {
            assert!(WslLaunch::new(distro.into(), binary.into(), Some(config.into())).is_err());
        }
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn worker_stop_during_startup_joins_only_its_owned_child() {
        let marker = std::env::temp_dir().join(format!("splitty-owned-start-{}", Uuid::new_v4()));
        let mut command = tokio::process::Command::new("python3");
        command.args(["-c", "import os,sys,time; open(sys.argv[1], 'w').write(str(os.getpid())); time.sleep(30)"]).arg(&marker);
        let stop = Arc::new(AtomicBool::new(false));
        let request_stop = stop.clone();
        let observed_marker = marker.clone();
        let cancel = tokio::spawn(async move {
            loop {
                if tokio::fs::read_to_string(&observed_marker).await.is_ok() {
                    request_stop.store(true, Ordering::Release);
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            OwnedService::start_owner(command, &stop),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        cancel.await.unwrap();
        let pid = std::fs::read_to_string(&marker).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "startup completion includes child reaping"
        );
        std::fs::remove_file(marker).unwrap();
    }
}
