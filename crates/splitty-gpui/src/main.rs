//! The native desktop app.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
mod app;
mod appearance;
mod askpass;
mod chrome;
mod document;
mod keys;
mod offscreen;
mod startup;
mod terminal;
mod terminal_engine;
mod workspace;

use appearance::titlebar::{
    DecorationProfile, DesktopAppearance, DesktopWindowIcons, SystemFont, TitleBarAppearance,
    TitleBarStyle, WindowDecoration,
};
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

/// Centers native macOS traffic lights in the themed 36 px header.
const MACOS_TRAFFIC_LIGHTS: Point<Pixels> = Point {
    x: px(9.),
    y: px(10.),
};

/// The traffic lights centered in a header of the given height.
#[cfg(target_os = "macos")]
fn macos_traffic_lights(bar: f32) -> Point<Pixels> {
    point(
        MACOS_TRAFFIC_LIGHTS.x,
        px(((bar - 16.) / 2.).max(0.).round()),
    )
}

struct RootView {
    content: AnyView,
    workspace: Option<WeakEntity<workspace::Workspace>>,
    title: String,
    _title_subscription: Option<Subscription>,
    /// The window draws its own title bar in the theme's colors.
    custom_titlebar: bool,
    startup_title_bar_style: TitleBarStyle,
    startup_window_decoration: WindowDecoration,
    requested_decoration: Option<WindowDecorations>,
    gnome: bool,
    system_font: SystemFont,
    _desktop_observers: Vec<Subscription>,
    #[cfg(target_os = "linux")]
    _desktop_settings_task: Task<()>,
    waiting_for_appearance: bool,
    appearance_subscription: Option<Subscription>,
    reveal_timeout: Option<Task<()>>,
    /// The header height the traffic lights were last centered in.
    #[cfg(target_os = "macos")]
    traffic_lights_bar: f32,
}
impl RootView {
    fn new(
        content: AnyView,
        workspace: Option<&Entity<workspace::Workspace>>,
        title_bar_style: TitleBarStyle,
        window_decoration: WindowDecoration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Windows applies a hidden window's initial placement on activation.
        let windows_chrome = cfg!(target_os = "windows") && workspace.is_some();
        let custom_titlebar = cfg!(any(target_os = "windows", target_os = "macos"))
            && !window_decoration.native(title_bar_style)
            && workspace.is_some();
        let desktop = cx
            .try_global::<DesktopAppearance>()
            .cloned()
            .unwrap_or_else(|| DesktopAppearance::detect(cx));
        let gnome = desktop.gnome;
        let system_font = desktop.font;
        let desktop_observers = vec![
            window.observe_button_layout_changed(|window, _| window.refresh()),
            window.observe_window_appearance(|window, _| window.refresh()),
        ];
        #[cfg(target_os = "linux")]
        let desktop_settings_task = {
            let renderer = cx.svg_renderer();
            let background = cx.background_executor().clone();
            cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt as _;
                let settings = ashpd::desktop::settings::Settings::new().await.ok();
                // Subscribe before reading so concurrent desktop changes are not lost.
                let changes = match &settings {
                    Some(settings) => settings.receive_setting_changed().await.ok(),
                    None => None,
                };
                let read_font = || async {
                    let settings = settings.as_ref()?;
                    let value = settings
                        .read::<String>("org.gnome.desktop.interface", "font-name")
                        .await
                        .ok()?;
                    let mut font = SystemFont::parse(&value)?;
                    let scale = settings
                        .read::<f64>("org.gnome.desktop.interface", "text-scaling-factor")
                        .await
                        .unwrap_or(1.);
                    if scale.is_finite() && (0.5..=4.).contains(&scale) {
                        font.size *= scale as f32;
                    }
                    Some(font)
                };
                let read_icons = || async {
                    let theme = match &settings {
                        Some(settings) => settings
                            .read::<String>("org.gnome.desktop.interface", "icon-theme")
                            .await
                            .unwrap_or_default(),
                        None => String::new(),
                    };
                    let renderer = renderer.clone();
                    background
                        .spawn(async move { DesktopWindowIcons::load(&theme, &renderer) })
                        .await
                };
                let apply_icons = |icons, cx: &mut AsyncWindowContext| {
                    let _ = this.update(cx, |_, cx| {
                        cx.set_global(icons);
                        cx.refresh_windows();
                    });
                };
                let apply = |font: Option<SystemFont>, cx: &mut AsyncWindowContext| {
                    if let Some(font) = font {
                        let _ = this.update(cx, |this, cx| {
                            this.system_font = font.clone();
                            cx.set_global(DesktopAppearance {
                                font,
                                gnome: this.gnome,
                            });
                            if let Some(workspace) = &this.workspace {
                                let _ = workspace.update(cx, |workspace, cx| {
                                    workspace.refresh_desktop_appearance(cx)
                                });
                            }
                            cx.notify();
                        });
                    }
                };
                apply(read_font().await, cx);
                if gnome {
                    apply_icons(read_icons().await, cx);
                }
                if let Some(mut changes) = changes {
                    while let Some(value) = changes.next().await {
                        if value.namespace() == "org.gnome.desktop.interface"
                            && matches!(value.key(), "font-name" | "text-scaling-factor")
                        {
                            apply(read_font().await, cx);
                        }
                        if gnome
                            && value.namespace() == "org.gnome.desktop.interface"
                            && value.key() == "icon-theme"
                        {
                            apply_icons(read_icons().await, cx);
                        }
                    }
                }
            })
        };
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
            startup_title_bar_style: title_bar_style,
            startup_window_decoration: window_decoration,
            requested_decoration: None,
            gnome,
            system_font,
            _desktop_observers: desktop_observers,
            #[cfg(target_os = "linux")]
            _desktop_settings_task: desktop_settings_task,
            waiting_for_appearance: windows_chrome,
            appearance_subscription,
            reveal_timeout,
            #[cfg(target_os = "macos")]
            traffic_lights_bar: 36.,
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
    fn title_bar_appearance(&self, window: &Window, cx: &App) -> TitleBarAppearance {
        let fallback = appearance::NativeAppearance::fallback(&self.system_font.family);
        let appearance = self.workspace.as_ref().and_then(|w| w.upgrade());
        let appearance = appearance
            .as_ref()
            .map(|w| w.read(cx).appearance())
            .unwrap_or(&fallback);
        // Native decoration is selected when opening on macOS/Windows. Keep
        // the current window internally consistent until the next launch.
        let style = if cfg!(target_os = "linux") {
            appearance.title_bar_style
        } else {
            self.startup_title_bar_style
        };
        let mut app = appearance.clone();
        if !cfg!(target_os = "linux") {
            app.window_decoration = self.startup_window_decoration;
        }
        let mut resolved = TitleBarAppearance::resolve(
            style,
            &app,
            window.appearance(),
            self.gnome,
            self.system_font.clone(),
        );
        if resolved.profile == DecorationProfile::Adwaita {
            resolved.icons = cx
                .try_global::<DesktopWindowIcons>()
                .cloned()
                .unwrap_or_default();
        }
        resolved
    }

    fn title_bar(&self, window: &Window, cx: &App) -> AnyElement {
        let workspace = self.workspace.clone();
        chrome::WindowChrome {
            title: self.title.clone(),
            appearance: self.title_bar_appearance(window, cx),
            on_close: Rc::new(move |window, cx| {
                if let Some(workspace) = &workspace {
                    let _ = workspace.update(cx, |workspace, cx| {
                        workspace.request_window_close(window, cx)
                    });
                } else {
                    window.remove_window();
                }
            }),
        }
        .into_any_element()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if cfg!(target_os = "linux") && self.workspace.is_some() {
            let decoration = self
                .workspace
                .as_ref()
                .and_then(|w| w.upgrade())
                .map(|w| {
                    let a = w.read(cx).appearance();
                    if a.window_decoration.native(a.title_bar_style) {
                        WindowDecorations::Server
                    } else {
                        WindowDecorations::Client
                    }
                })
                .unwrap_or(WindowDecorations::Client);
            if self.requested_decoration != Some(decoration) {
                window.request_decorations(decoration);
                self.requested_decoration = Some(decoration);
            }
            let radius = self.title_bar_appearance(window, cx).radius;
            gpui_kit::component::set_window_corner_radius(window, cx, px(radius));
        }
        // The theme sets the header height; keep the lights centered in it.
        #[cfg(target_os = "macos")]
        if self.custom_titlebar {
            let bar = self.title_bar_appearance(window, cx).height;
            if self.traffic_lights_bar != bar {
                self.traffic_lights_bar = bar;
                window.set_traffic_light_position(macos_traffic_lights(bar));
            }
        }
        let show_titlebar = !window.is_fullscreen()
            && (self.custom_titlebar
                || (self.workspace.is_some()
                    && matches!(window.window_decorations(), Decorations::Client { .. })));
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_titlebar, |view| view.child(self.title_bar(window, cx)))
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
    use super::{RootView, TitleBarStyle};
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
                    RootView::new(
                        workspace.clone().into(),
                        Some(&workspace),
                        TitleBarStyle::Theme,
                        super::WindowDecoration::System,
                        window,
                        cx,
                    )
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

/// The default keys of every command, one line per category, as `--help`
/// lists them. `keybindings.json` in the configuration directory changes them.
fn key_help(platform: splitty_client_core::keymap::Platform) -> String {
    use splitty_client_core::keymap::{Command, Keymap};
    let keymap = Keymap::defaults(platform);
    let mut lines = vec![format!(
        "Default keys (change them in {}):",
        splitty_client_core::keymap::KeybindingsFile::default_path().display()
    )];
    let mut category = None;
    let mut entries: Vec<String> = Vec::new();
    for command in Command::all() {
        if category != Some(command.category()) {
            if let Some(category) = category.replace(command.category()) {
                lines.push(format!("  {}: {}", category.title(), entries.join("; ")));
                entries.clear();
            }
        }
        let keys: Vec<_> = keymap
            .bindings(command)
            .iter()
            .map(|chord| chord.label(platform))
            .collect();
        match command {
            Command::SelectScreen(1) => {
                // The nine Screen keys differ only in their digit.
                let keys: Vec<_> = keys.iter().map(|key| format!("{key}–9")).collect();
                entries.push(format!("Switch to Screen 1–9 {}", keys.join(", ")));
            }
            Command::SelectScreen(_) => {}
            _ if keys.is_empty() => {}
            _ => entries.push(format!("{} {}", command.title(), keys.join(", "))),
        }
    }
    if let Some(category) = category {
        lines.push(format!("  {}: {}", category.title(), entries.join("; ")));
    }
    lines.join("\n")
}

fn main() -> anyhow::Result<()> {
    // The embedded service runs this executable as OpenSSH's askpass helper.
    #[cfg(not(target_os = "windows"))]
    if let Some(result) = splitty_client_core::embedded::askpass_helper() {
        return result.map_err(Into::into);
    }
    // An askpass call meant for an earlier version: exit unanswered.
    if startup::stale_askpass_call(|key| std::env::var_os(key)) {
        std::process::exit(1);
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
            "splitty-gpui [--wsl-distro NAME --wsl-backend PATH --wsl-config-dir PATH] [--workspace-smoke] [--probe | --smoke] [--web-spike] [--screenshot PATH]\nLocal workspace with terminal splits and tabs. On Windows, without WSL arguments the app uses the remembered or default WSL distribution and installs its backend there; the three WSL arguments together name an explicit target instead, with absolute Linux paths in an existing distribution, and install nothing. WSL itself is never installed. --workspace-smoke exercises its real sessions, layout and keyboard routing. --probe opens the legacy terminal/Markdown fixture; --smoke checks that fixture and exits. --web-spike selects the fixture with optional CEF.\nDrag a tab into the tab row for exact placement, or into the pane body for a split; the tab row scrolls horizontally. Keys that run no command, Ctrl+C among them, remain terminal input.\n--screenshot requires --smoke and the capture feature; native Linux capture remains unavailable.\n\n{}",
            key_help(splitty_client_core::keymap::Platform::current())
        );
        return Ok(());
    }
    // Started without a WSL target, the Windows workspace chooses a
    // distribution and installs the backend that ships beside the executable.
    // That starts now, on a thread of its own, so WSL boots while the window
    // opens; the window shows its phases and a failure.
    #[cfg(target_os = "windows")]
    let backend = match backend {
        splitty_client_core::owned_service::LocalBackend::Embedded if !probe => {
            splitty_client_core::owned_service::LocalBackend::Preparing(
                splitty_client_core::owned_service::WslPreparation::spawn(|progress| {
                    let launch = startup::wsl::prepare(progress)?;
                    startup::set_wsl_distro(launch.distro());
                    Ok(launch)
                })?,
            )
        }
        backend => backend,
    };
    if let splitty_client_core::owned_service::LocalBackend::Wsl(launch) = &backend {
        startup::set_wsl_distro(launch.distro());
    }
    let (startup_title_bar_style, startup_window_decoration) =
        if !probe && !workspace_smoke && workspace::remembers_for(&backend) {
            let memory = splitty_client_core::appearance::AppearanceMemory::open(
                splitty_client_core::appearance::AppearanceMemory::default_path(),
            );
            memory
                .stored()
                .map(|saved| {
                    (
                        TitleBarStyle::from_setting(&saved.title_bar_style),
                        WindowDecoration::from_setting(&saved.window_decoration),
                    )
                })
                .unwrap_or_default()
        } else {
            (TitleBarStyle::default(), WindowDecoration::default())
        };
    let custom_titlebar = !probe
        && cfg!(any(target_os = "windows", target_os = "macos"))
        && !startup_window_decoration.native(startup_title_bar_style);
    let worker = Rc::new(RefCell::new(None));
    let launched_worker = worker.clone();
    let window_error = Rc::new(RefCell::new(None));
    let reported_window_error = window_error.clone();
    let outcome = Arc::new(AtomicBool::new(false));
    let reported_outcome = outcome.clone();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    application.run(move |cx| {
        gpui_kit::init(cx);
        cx.set_global(DesktopAppearance::detect(cx));
        terminal::init(cx);
        gpui_kit::component::theme::Theme::change(
            gpui_kit::component::theme::ThemeMode::Dark,
            None,
            cx,
        );
        let result = gpui_kit::open_window(
            WindowOptions {
                // Match the installed splitty.desktop entry on Wayland and X11.
                app_id: cfg!(target_os = "linux").then(|| "splitty".into()),
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
                    appears_transparent: custom_titlebar,
                    traffic_light_position: custom_titlebar.then_some(MACOS_TRAFFIC_LIGHTS),
                }),
                app_owns_titlebar_drag: custom_titlebar,
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
                cx.new(|cx| {
                    RootView::new(
                        view,
                        workspace.as_ref(),
                        startup_title_bar_style,
                        startup_window_decoration,
                        window,
                        cx,
                    )
                })
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

#[cfg(test)]
mod help_tests {
    use super::key_help;
    use splitty_client_core::keymap::Platform;

    #[test]
    fn the_help_lists_the_default_keys_of_the_platform() {
        let linux = key_help(Platform::Linux);
        assert!(linux.contains("  Screens: New Screen Ctrl+Shift+Space; "));
        assert!(linux.contains("Switch to Screen 1–9 Alt+1–9, Alt+Shift+1–9;"));
        assert!(linux.contains("New Tab Alt+T"));
        assert!(linux.contains("Copy Ctrl+Shift+C, Super+C"));
        assert!(
            !linux.contains("Connect to Host"),
            "unbound commands stay out"
        );
        assert!(!linux.contains("Switch to Screen 2"));
        let mac = key_help(Platform::Mac);
        assert!(mac.contains("Switch to Screen 1–9 ⌘1–9;"));
        assert!(mac.contains("Quit ⌘Q"));
        assert_eq!(
            mac.lines().count(),
            7,
            "a heading and one line per category"
        );
    }
}
