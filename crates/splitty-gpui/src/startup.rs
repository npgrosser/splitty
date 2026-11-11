//! Process startup configuration, parsed before creating a window.
use splitty_client_core::owned_service::{LocalBackend, WslLaunch};
use std::path::PathBuf;

pub struct Options {
    pub smoke: bool,
    pub probe: bool,
    pub workspace_smoke: bool,
    pub web: bool,
    pub screenshot: Option<PathBuf>,
    pub backend: LocalBackend,
    pub help: bool,
}

#[cfg(any(target_os = "windows", test))]
pub mod wsl;

static WSL_DISTRO: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Records the WSL distribution the local backend runs in.
pub fn set_wsl_distro(distro: &str) {
    let _ = WSL_DISTRO.set(distro.to_owned());
}

/// The WSL distribution of the local backend, when it runs in one; local
/// paths are then paths of that distribution.
pub fn wsl_distro() -> Option<&'static str> {
    WSL_DISTRO.get().map(String::as_str)
}

impl Options {
    pub fn parse(
        arguments: impl IntoIterator<Item = String>,
        is_windows: bool,
        capture_enabled: bool,
    ) -> anyhow::Result<Self> {
        let mut result = Self {
            smoke: false,
            probe: false,
            workspace_smoke: false,
            web: false,
            screenshot: None,
            backend: LocalBackend::Embedded,
            help: false,
        };
        let mut distro = None;
        let mut backend = None;
        let mut config = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--smoke" => {
                    result.smoke = true;
                    result.probe = true;
                }
                "--probe" => result.probe = true,
                "--workspace-smoke" => result.workspace_smoke = true,
                "--web-spike" => {
                    result.web = true;
                    result.probe = true;
                }
                "--screenshot" => {
                    result.screenshot = Some(PathBuf::from(value(&mut arguments, &argument)?));
                }
                "--wsl-distro" => {
                    assign(&mut distro, value(&mut arguments, &argument)?, &argument)?
                }
                "--wsl-backend" => {
                    assign(&mut backend, value(&mut arguments, &argument)?, &argument)?
                }
                "--wsl-config-dir" => {
                    assign(&mut config, value(&mut arguments, &argument)?, &argument)?
                }
                "--help" | "-h" => {
                    result.help = true;
                    return Ok(result);
                }
                other => anyhow::bail!("Unknown native preview argument: {other}"),
            }
        }
        if result.workspace_smoke && result.probe {
            anyhow::bail!("--workspace-smoke cannot be combined with probe flags");
        }
        if result.screenshot.is_some() && (!result.smoke || !capture_enabled) {
            anyhow::bail!("--screenshot requires --smoke and a build with --features capture");
        }
        let configured = distro.is_some() || backend.is_some() || config.is_some();
        if configured && (!is_windows || result.probe) {
            anyhow::bail!("WSL options are supported only by the Windows workspace");
        }
        // Without WSL options the Windows workspace chooses a distribution and
        // installs its backend (`wsl::prepare`); the backend stays `Embedded` here.
        if configured {
            let (Some(distro), Some(backend), Some(config)) = (distro, backend, config) else {
                anyhow::bail!(
                    "An explicit WSL target requires --wsl-distro NAME --wsl-backend /absolute/path/to/splitty --wsl-config-dir /absolute/config"
                );
            };
            result.backend = LocalBackend::Wsl(WslLaunch::new(distro, backend, Some(config))?);
        }
        Ok(result)
    }
}

fn value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
    let value = arguments
        .next()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))?;
    anyhow::ensure!(!value.starts_with("--"), "{flag} requires a value");
    Ok(value)
}

fn assign(target: &mut Option<String>, value: String, flag: &str) -> anyhow::Result<()> {
    anyhow::ensure!(target.is_none(), "{flag} may only be specified once");
    *target = Some(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Options;
    use splitty_client_core::owned_service::LocalBackend;

    fn parse(args: &[&str], windows: bool) -> anyhow::Result<Options> {
        Options::parse(args.iter().map(|arg| (*arg).to_owned()), windows, false)
    }

    #[test]
    fn an_explicit_windows_target_is_complete_and_no_options_leave_the_choice_to_startup() {
        assert!(matches!(
            parse(&[], true).unwrap().backend,
            LocalBackend::Embedded
        ));
        assert!(parse(&["--wsl-distro", "Test Linux"], true).is_err());
        let args = [
            "--wsl-distro",
            "Test Linux",
            "--wsl-backend",
            "/tmp/preview backend/splitty",
            "--wsl-config-dir",
            "/tmp/preview configuration",
        ];
        assert!(matches!(
            parse(&args, true).unwrap().backend,
            LocalBackend::Wsl(_)
        ));
        assert!(parse(&args, false).is_err());
        let mut repeated = args.to_vec();
        repeated.extend(["--wsl-distro", "Another Linux"]);
        assert!(parse(&repeated, true).is_err());
        let mut relative = args;
        relative[3] = "relative/splitty";
        assert!(parse(&relative, true).is_err());
    }

    #[test]
    fn help_and_unix_defaults_work_without_wsl_and_probe_flags_remain_scoped() {
        assert!(parse(&["--help"], true).unwrap().help);
        assert!(matches!(
            parse(&[], false).unwrap().backend,
            LocalBackend::Embedded
        ));
        assert!(
            parse(&["--workspace-smoke"], false)
                .unwrap()
                .workspace_smoke
        );
        assert!(parse(&["--workspace-smoke", "--probe"], false).is_err());
        assert!(parse(&["--screenshot", "capture.png"], false).is_err());
        assert!(parse(&["--unknown"], false).is_err());
        assert!(parse(&["--wsl-distro", "--wsl-backend"], true).is_err());
    }
}
