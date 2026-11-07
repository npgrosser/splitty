//! Explicitly invoked native UI experiment; released desktop clients are unchanged.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod app;
mod appearance;
mod document;
mod offscreen;
mod startup;
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
    let startup::Options {
        smoke,
        probe,
        workspace_smoke,
        web,
        screenshot,
        backend,
        help,
    } = startup::Options::parse(
        std::env::args().skip(1),
        cfg!(target_os = "windows"),
        cfg!(feature = "capture"),
    )?;
    if help {
        println!(
                    "splitty-gpui [--wsl-distro NAME --wsl-backend PATH --wsl-config-dir PATH] [--workspace-smoke] [--probe | --smoke] [--web-spike] [--screenshot PATH]\nExperimental isolated local workspace with terminal splits and tabs. Windows workspace startup requires all three WSL arguments; backend and config paths are absolute Linux paths in the named existing distribution. No WSL installation or daemon discovery is performed. --workspace-smoke exercises its real sessions, layout and keyboard routing. --probe opens the legacy terminal/Markdown fixture; --smoke checks that fixture and exits. --web-spike selects the fixture with optional CEF.\nWorkspace shortcuts: Cmd on macOS, Alt elsewhere. Modifier+T creates a tab; modifier+D splits right; modifier+Shift+D splits below; modifier+W closes the focused pane; modifier+M toggles the focused group; modifier+Shift+arrow focuses the nearest split group; modifier+PageUp/PageDown selects the previous/next tab in that group. Ctrl+Shift+Space creates a Screen; modifier+Shift+1–9 selects a Screen on verified US/German XKB layouts. Drag a tab into the tab row for exact placement, or into the pane body for a split; the tab row scrolls horizontally. Ctrl+C and Ctrl+Shift+C/V remain terminal input and copy/paste.\n--screenshot requires --smoke and the capture feature; native Linux capture remains unavailable. Manual IME and clipboard remain unverified. Released desktop clients are unchanged."
                );
        return Ok(());
    }
    let worker = Rc::new(RefCell::new(None));
    let launched_worker = worker.clone();
    let window_error = Rc::new(RefCell::new(None));
    let reported_window_error = window_error.clone();
    let outcome = Arc::new(AtomicBool::new(false));
    let reported_outcome = outcome.clone();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    application.run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::component::theme::Theme::change(
            gpui_kit::component::theme::ThemeMode::Dark,
            None,
            cx,
        );
        let result = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1200.), px(760.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(
                        if probe {
                            "Splitty native UI probe"
                        } else if cfg!(target_os = "windows") {
                            "Splitty · Windows preview"
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
                            backend,
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
