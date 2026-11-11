//! The native desktop app.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod app;
mod appearance;
mod askpass;
mod document;
mod offscreen;
mod startup;
mod terminal;
mod terminal_engine;
mod workspace;

use gpui_kit::component::{ActiveTheme as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

/// Windows and macOS hide the system title bar of a workspace window and draw
/// it in the theme's colors; macOS keeps its native window buttons.
const CUSTOM_TITLEBAR: bool = cfg!(any(target_os = "windows", target_os = "macos"));
/// The web client's title bar height.
const MACOS_TITLE_BAR_HEIGHT: Pixels = px(36.);
/// Keeps the centered title clear of the window buttons on both sides.
const MACOS_TITLE_INSET: Pixels = px(80.);
/// Centers the native window buttons vertically in the 36 px bar.
const MACOS_TRAFFIC_LIGHTS: Point<Pixels> = Point {
    x: px(9.),
    y: px(10.),
};

struct RootView {
    content: AnyView,
    workspace: Option<WeakEntity<workspace::Workspace>>,
    title: String,
    _title_subscription: Option<Subscription>,
    /// The window draws its own title bar in the theme's colors.
    custom_titlebar: bool,
    waiting_for_appearance: bool,
    appearance_subscription: Option<Subscription>,
    reveal_timeout: Option<Task<()>>,
}
impl RootView {
    fn new(
        content: AnyView,
        workspace: Option<&Entity<workspace::Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Windows applies a hidden window's initial placement on activation.
        let windows_chrome = cfg!(target_os = "windows") && workspace.is_some();
        let custom_titlebar = CUSTOM_TITLEBAR && workspace.is_some();
        let title = workspace.map_or_else(
            || "Splitty".to_owned(),
            |workspace| workspace.read(cx).window_title(),
        );
        if workspace.is_some() {
            window.set_window_title(&title);
        }
        let title_subscription = workspace.map(|workspace| {
            cx.observe_in(workspace, window, |this, workspace, window, cx| {
                let title = workspace.read(cx).window_title();
                if this.title != title {
                    window.set_window_title(&title);
                    this.title = title;
                }
                cx.notify();
            })
        });
        let appearance_subscription = workspace.filter(|_| windows_chrome).map(|workspace| {
            cx.subscribe_in(
                workspace,
                window,
                |this, _, _: &workspace::AppearanceSettled, window, cx| {
                    this.reveal(window, cx);
                },
            )
        });
        let reveal_timeout = windows_chrome.then(|| {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update_in(cx, |this, window, cx| this.reveal(window, cx));
            })
        });
        Self {
            content,
            workspace: workspace.map(Entity::downgrade),
            title,
            _title_subscription: title_subscription,
            custom_titlebar,
            waiting_for_appearance: windows_chrome,
            appearance_subscription,
            reveal_timeout,
        }
    }

    fn reveal(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.waiting_for_appearance {
            return;
        }
        self.waiting_for_appearance = false;
        self.appearance_subscription = None;
        self.reveal_timeout = None;
        // A hidden Windows window need not receive paint callbacks. Draw once
        // after releasing the entity borrow, then show the prepared surface.
        // Later theme updates must not activate the window again.
        window.defer(cx, |window, cx| {
            let arena = window.draw(cx);
            arena.clear(cx);
            window.activate_window();
        });
        cx.notify();
    }
}
impl RootView {
    /// Matches the web client's custom title bar (`TitleBar.svelte`): the
    /// drawer's `surface-raised` background without a bottom divider, the title
    /// centered over the window in `text-secondary`, and platform window
    /// controls on the trailing edge (or native traffic lights on macOS).
    /// The centered title overlay has no hitbox, so dragging and double clicks
    /// reach the bar underneath it.
    fn title_bar(&self, cx: &App) -> AnyElement {
        let workspace = self.workspace.clone();
        let (bg, title_color, font_family) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.upgrade())
            .map(|workspace| {
                let appearance = workspace.read(cx).appearance();
                (
                    rgb(appearance.raised).into(),
                    rgb(appearance.secondary).into(),
                    SharedString::from(appearance.font_family.clone()),
                )
            })
            .unwrap_or_else(|| {
                (
                    cx.theme().sidebar,
                    cx.theme().muted_foreground,
                    if cfg!(target_os = "macos") {
                        ".SystemUIFont".into()
                    } else {
                        cx.theme().font_family.clone()
                    },
                )
            });
        let title_inset = if cfg!(target_os = "macos") {
            MACOS_TITLE_INSET
        } else {
            px(108.)
        };
        div()
            .relative()
            .flex_shrink_0()
            .child(
                TitleBar::new()
                    .h(MACOS_TITLE_BAR_HEIGHT)
                    .bg(bg)
                    .border_b_0()
                    .on_close_window(move |_, window, cx| {
                        if let Some(workspace) = workspace.as_ref() {
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.request_window_close(window, cx);
                            });
                        } else {
                            window.remove_window();
                        }
                    }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .px(title_inset)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("window-title")
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(font_family)
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(title_color)
                            .child(self.title.clone()),
                    ),
            )
            .into_any_element()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_titlebar = self.custom_titlebar
            || (self.workspace.is_some()
                && matches!(window.window_decorations(), Decorations::Client { .. }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_titlebar, |view| view.child(self.title_bar(cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.content.clone()),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod title_tests {
    use super::RootView;
    use gpui_kit::{AppContext, TestAppContext};
    use splitty_client_core::{native_workspace::Worker, workspace::WindowWorkspace};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use uuid::Uuid;

    #[test]
    fn root_observes_workspace_title_in_os_window() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let workspace_out = Rc::new(RefCell::new(None));
        let (_, window) = app.add_window_view({
            let workspace_out = workspace_out.clone();
            move |window, cx| {
                let workspace = cx.new(|cx| {
                    let (workspace, worker) = super::workspace::Workspace::new_with_worker(
                        false,
                        Arc::new(AtomicBool::new(false)),
                        window,
                        cx,
                        |_, _, _| Worker::ui_test_fixture(),
                    );
                    worker.shutdown().unwrap();
                    workspace
                });
                *workspace_out.borrow_mut() = Some(workspace.clone());
                let root = cx.new(|cx| {
                    RootView::new(workspace.clone().into(), Some(&workspace), window, cx)
                });
                gpui_kit::base::Root::new(root, window, cx)
            }
        });
        let workspace = workspace_out.borrow_mut().take().unwrap();
        assert_eq!(window.window_title().as_deref(), Some("Splitty"));
        window.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                let mut model = WindowWorkspace::new(Uuid::new_v4());
                model.ui_test_local_screen_fixture("Work").unwrap();
                workspace.ui_test_set_title_model(model, cx);
            });
            window.draw(cx).clear(cx);
        });
        window.run_until_parked();
        assert_eq!(
            window.window_title().as_deref(),
            Some("Work · Terminal · Splitty")
        );
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
    if std::env::var_os("SPLITTY_BROWSER_ASKPASS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        return splitty_client_core::ssh_askpass_helper().map_err(Into::into);
    }
    if std::env::var_os("SPLITTY_SSH_ASKPASS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        return askpass::run();
    }
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
                    "splitty-gpui [--wsl-distro NAME --wsl-backend PATH --wsl-config-dir PATH] [--workspace-smoke] [--probe | --smoke] [--web-spike] [--screenshot PATH]\nLocal workspace with terminal splits and tabs. On Windows, without WSL arguments the app uses the remembered or default WSL distribution and installs its backend there; the three WSL arguments together name an explicit target instead, with absolute Linux paths in an existing distribution, and install nothing. WSL itself is never installed. --workspace-smoke exercises its real sessions, layout and keyboard routing. --probe opens the legacy terminal/Markdown fixture; --smoke checks that fixture and exits. --web-spike selects the fixture with optional CEF.\nWorkspace shortcuts: Cmd on macOS, Alt elsewhere. Modifier+T creates a tab; modifier+D splits right; modifier+Shift+D splits below; modifier+W closes the focused pane; modifier+M toggles the focused group; modifier+Shift+arrow focuses the nearest split group; modifier+PageUp/PageDown selects the previous/next tab in that group. Ctrl+Shift+Space creates a Screen; modifier+Shift+1–9 selects a Screen on verified US/German XKB layouts. Drag a tab into the tab row for exact placement, or into the pane body for a split; the tab row scrolls horizontally. Ctrl+C and Ctrl+Shift+C/V remain terminal input and copy/paste.\n--screenshot requires --smoke and the capture feature; native Linux capture remains unavailable."
                );
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    splitty_client_core::set_askpass_program(std::env::current_exe()?);
    // Started without a WSL target, the Windows workspace chooses a
    // distribution and installs the backend that ships beside the executable.
    #[cfg(target_os = "windows")]
    let backend = match backend {
        splitty_client_core::owned_service::LocalBackend::Embedded if !probe => {
            match startup::wsl::prepare() {
                Ok(launch) => splitty_client_core::owned_service::LocalBackend::Wsl(launch),
                Err(error) => {
                    startup::wsl::report_failure(&error);
                    anyhow::bail!(error);
                }
            }
        }
        backend => backend,
    };
    let worker = Rc::new(RefCell::new(None));
    let launched_worker = worker.clone();
    let window_error = Rc::new(RefCell::new(None));
    let reported_window_error = window_error.clone();
    let outcome = Arc::new(AtomicBool::new(false));
    let reported_outcome = outcome.clone();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    application.run(move |cx| {
        gpui_kit::init(cx);
        #[cfg(not(target_os = "windows"))]
        askpass::install(cx);
        terminal::init(cx);
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
                    appears_transparent: CUSTOM_TITLEBAR && !probe,
                    traffic_light_position: (!probe).then_some(MACOS_TRAFFIC_LIGHTS),
                }),
                show: !cfg!(target_os = "windows") || probe,
                focus: !cfg!(target_os = "windows") || probe,
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
                let (view, workspace): (AnyView, Option<Entity<workspace::Workspace>>) = if probe {
                    let view = cx.new(|cx| {
                        let (probe, owned_worker) =
                            app::Probe::new(smoke, web, reported_outcome, screenshot, window, cx);
                        *launched_worker.borrow_mut() = Some(OwnedWorker::Probe(owned_worker));
                        probe
                    });
                    (view.into(), None)
                } else {
                    let workspace = cx.new(|cx| {
                        let (workspace, owned_worker) = workspace::Workspace::new(
                            workspace_smoke,
                            reported_outcome,
                            backend,
                            window,
                            cx,
                        );
                        *launched_worker.borrow_mut() = Some(OwnedWorker::Workspace(owned_worker));
                        workspace
                    });
                    (workspace.clone().into(), Some(workspace))
                };
                cx.new(|cx| RootView::new(view, workspace.as_ref(), window, cx))
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
