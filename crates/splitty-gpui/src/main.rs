//! Explicitly invoked native UI experiment; released desktop clients are unchanged.
mod app;
mod document;
mod offscreen;
mod terminal;
mod terminal_engine;
mod workspace;

use gpui_kit::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

struct RootView(AnyView);
impl Render for RootView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}
enum OwnedWorker {
    Probe(app::Worker),
    Workspace(splitty_client_core::native_workspace::Worker),
}
impl OwnedWorker {
    fn shutdown(self) -> anyhow::Result<()> {
        match self {
            Self::Probe(worker) => worker.shutdown(),
            Self::Workspace(worker) => worker.shutdown(),
        }
    }
}

fn main() -> anyhow::Result<()> {
    if let Some(code) = offscreen::subprocess_exit() {
        std::process::exit(code);
    }
    let mut smoke = false;
    let mut probe = false;
    let mut workspace_smoke = false;
    let mut web = false;
    let mut screenshot = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--smoke" => {
                smoke = true;
                probe = true;
            }
            "--probe" => probe = true,
            "--workspace-smoke" => workspace_smoke = true,
            "--web-spike" => {
                web = true;
                probe = true;
            }
            "--screenshot" => {
                screenshot =
                    Some(std::path::PathBuf::from(arguments.next().ok_or_else(
                        || anyhow::anyhow!("--screenshot requires a path"),
                    )?))
            }
            "--help" | "-h" => {
                println!(
                    "splitty-gpui [--workspace-smoke] [--probe | --smoke] [--web-spike] [--screenshot PATH]\nExperimental isolated local workspace with terminal splits and tabs. --workspace-smoke exercises its real sessions, layout and keyboard routing. --probe opens the legacy terminal/Markdown fixture; --smoke checks that fixture and exits. --web-spike selects the fixture with optional CEF.\nWorkspace shortcuts: Cmd on macOS, Alt elsewhere. Modifier+T creates a tab; modifier+D splits right; modifier+Shift+D splits below; modifier+W closes the focused pane; modifier+Shift+arrow focuses the nearest split group. Ctrl+C and Ctrl+Shift+C/V remain terminal input and copy/paste.\n--screenshot requires --smoke and the capture feature; native Linux capture remains unavailable. Manual IME and clipboard remain unverified. Released desktop clients are unchanged."
                );
                return Ok(());
            }
            other => anyhow::bail!("Unknown probe argument: {other}"),
        }
    }
    if workspace_smoke && probe {
        anyhow::bail!("--workspace-smoke cannot be combined with probe flags");
    }
    if screenshot.is_some() && (!smoke || !cfg!(feature = "capture")) {
        anyhow::bail!("--screenshot requires --smoke and a build with --features capture");
    }
    let worker = Rc::new(RefCell::new(None));
    let launched_worker = worker.clone();
    let window_error = Rc::new(RefCell::new(None));
    let reported_window_error = window_error.clone();
    let outcome = Arc::new(AtomicBool::new(false));
    let reported_outcome = outcome.clone();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let theme = gpui_kit::base::Theme::global_mut(cx);
        theme.appearance = gpui_kit::base::ThemeAppearance::Dark;
        theme.tokens.colors = gpui_kit::base::ColorTokens::dark();
        let result = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1200.), px(760.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(
                        if probe {
                            "Splitty native UI probe"
                        } else {
                            "Splitty · Local"
                        }
                        .into(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                if let Some(gpu) = window.gpu_specs() {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "nativeGraphics": {
                                "device": gpu.device_name,
                                "software": gpu.is_software_emulated,
                                "driver": gpu.driver_name,
                                "driverInfo": gpu.driver_info,
                            }
                        })
                    );
                }
                let view: AnyView = if probe {
                    cx.new(|cx| {
                        let (probe, owned_worker) =
                            app::Probe::new(smoke, web, reported_outcome, screenshot, window, cx);
                        *launched_worker.borrow_mut() = Some(OwnedWorker::Probe(owned_worker));
                        probe
                    })
                    .into()
                } else {
                    cx.new(|cx| {
                        let (workspace, owned_worker) = workspace::Workspace::new(
                            workspace_smoke,
                            reported_outcome,
                            window,
                            cx,
                        );
                        *launched_worker.borrow_mut() = Some(OwnedWorker::Workspace(owned_worker));
                        workspace
                    })
                    .into()
                };
                cx.new(|_| RootView(view))
            },
        );
        if let Err(error) = result {
            eprintln!("Cannot open native GPUI window: {error:#}");
            *reported_window_error.borrow_mut() = Some(error);
            cx.quit();
        }
        cx.activate(true);
    });
    if let Some(worker) = worker.borrow_mut().take() {
        worker.shutdown()?;
        if smoke || workspace_smoke {
            eprintln!("{{\"probeShutdown\":\"complete\"}}");
        }
    }
    if let Some(error) = window_error.borrow_mut().take() {
        return Err(error);
    }
    if (smoke || workspace_smoke) && !outcome.load(Ordering::Acquire) {
        anyhow::bail!("Native smoke did not satisfy its evidence checks");
    }
    Ok(())
}
