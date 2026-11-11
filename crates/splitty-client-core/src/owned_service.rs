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
    /// `askpass` is the Linux path of the SSH dialog executable, when there is one.
    fn arguments(&self, askpass: Option<&str>) -> Vec<String> {
        let mut arguments = vec![
            "-d".into(),
            self.distro.clone(),
            "--exec".into(),
            "env".into(),
        ];
        if let Some(config_dir) = &self.config_dir {
            arguments.push(format!("SPLITTY_CONFIG_DIR={config_dir}"));
        }
        if let Some(askpass) = askpass {
            arguments.push(format!("SPLITTY_ASKPASS_PROGRAM={askpass}"));
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

#[derive(Clone, Debug, Default)]
pub enum LocalBackend {
    /// Existing Unix default. Windows must supply WSL explicitly.
    #[default]
    Embedded,
    Wsl(WslLaunch),
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
            #[cfg(not(target_os = "windows"))]
            LocalBackend::Embedded => Ok(crate::EmbeddedService::start().into()),
            #[cfg(target_os = "windows")]
            LocalBackend::Embedded => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows workspace requires a WSL backend",
            )),
            LocalBackend::Wsl(launch) => {
                let mut askpass = None;
                if let Some(program) = splitty_remote::ssh::askpass_program() {
                    // Resolve using the selected distro's mount configuration,
                    // rather than assuming Windows drives are under /mnt.
                    let mut translate = tokio::process::Command::new("wsl.exe");
                    #[cfg(target_os = "windows")]
                    translate.creation_flags(0x08000000);
                    let output = translate
                        .args(["-d", &launch.distro, "--exec", "wslpath", "-u"])
                        .arg(program)
                        .output()
                        .await?;
                    if !output.status.success() {
                        return Err(io::Error::other(
                            "Could not resolve the SSH dialog executable in WSL",
                        ));
                    }
                    let path = std::str::from_utf8(&output.stdout)
                        .map_err(|_| io::Error::other("Invalid WSL SSH dialog path"))?
                        .trim();
                    if !path.starts_with('/') || path.chars().any(char::is_control) {
                        return Err(io::Error::other("Invalid WSL SSH dialog path"));
                    }
                    askpass = Some(path.to_owned());
                }
                let arguments = launch.arguments(askpass.as_deref());
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
            launch.arguments(None),
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
        assert_eq!(
            default_config.arguments(Some("/mnt/c/app/splitty.exe"))[..6],
            [
                "-d",
                "D",
                "--exec",
                "env",
                "SPLITTY_ASKPASS_PROGRAM=/mnt/c/app/splitty.exe",
                "/home/a/splitty"
            ]
        );
        assert_eq!(
            default_config.arguments(None)[3..5],
            ["env", "/home/a/splitty"]
        );
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
