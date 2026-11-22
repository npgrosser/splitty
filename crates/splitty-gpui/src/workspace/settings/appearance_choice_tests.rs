//! settings, Native Appearance Choices Before Confirmation: the window
//! shows an appearance choice of the editor before the local service
//! confirms it, through the editor's request and the workspace's updates.
use super::{SettingsField, SettingsValue};
use crate::appearance::{Choices, NativeAppearance};
use crate::workspace::{
    chrome::view_tests::with_view, Command, Destination, Update, WorkerCommand, Workspace,
};
use gpui_kit::{Entity, VisualTestContext};
use serde_json::{json, Value};
use splitty_client_core::{
    appearance::{
        test_support::{settings_snapshot_for_scheme, settings_snapshot_with_terminal},
        AppearanceMemory, AppearanceSnapshot, ColorScheme,
    },
    local_service::{SettingsWriteOutcome, SettingsWriteResult},
    workspace::WindowWorkspace,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
    let (_, mut updates) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(Some(update), &mut updates, window, cx)
        });
        window.draw(cx).clear(cx);
    });
}

/// A window whose local Screen shows one terminal, with the local
/// service's `first` settings applied; its connection and the commands it
/// sends.
fn setup(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    first: impl FnOnce(Uuid) -> Arc<AppearanceSnapshot>,
) -> (Uuid, mpsc::Receiver<WorkerCommand>) {
    let mut model = WindowWorkspace::new(Uuid::new_v4());
    model.ui_test_local_screen_fixture("Work").unwrap();
    let source = model.lifecycle_token();
    let owner = source.connection_id();
    model
        .source_mut(source.source_id())
        .unwrap()
        .apply_snapshot(
            owner,
            &json!({"type":"mux.screen_snapshot","screen":{
                "id":"work","revision":2,
                "root":{"type":"slot","id":"only","paneIds":["p0"],"activePaneId":"p0"},
                "panes":{"p0":{"id":"p0","sessionId":"s0"}},
                "sessions":{"s0":{"id":"s0","status":"running"}}
            }}),
        )
        .unwrap();
    let (commands, mut receiver) = mpsc::channel(64);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.commands = commands.into();
            workspace.destination = Destination::Screens;
            workspace.reconcile(model, window, cx);
        });
        window.draw(cx).clear(cx);
    });
    deliver(window, view, Update::Appearance(first(owner)));
    window.update(|_, cx| assert!(!view.read(cx).panes.is_empty(), "a terminal is shown"));
    while receiver.try_recv().is_ok() {}
    (owner, receiver)
}

fn snapshot(owner: Uuid, generation: u64, terminal: Value, appearance: Value) -> Update {
    Update::Appearance(settings_snapshot_with_terminal(
        owner, generation, terminal, appearance,
    ))
}

/// Choose `value` for `field` in the editor; the request it sends.
fn choose(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    commands: &mut mpsc::Receiver<WorkerCommand>,
    field: SettingsField,
    value: SettingsValue,
) -> Uuid {
    window.update(|window, cx| {
        let settings = view.read(cx).settings.clone();
        settings.update(cx, |settings, cx| {
            settings.request(field, value.clone(), cx)
        });
        window.draw(cx).clear(cx);
    });
    // Terminal work of the window may come first.
    let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok())
        .filter_map(|command| match command.kind() {
            Command::UpdateSettings {
                request_id, edit, ..
            } => Some((*request_id, edit.field(), edit.value().clone())),
            _ => None,
        })
        .collect();
    match sent.as_slice() {
        [(request_id, sent_field, sent_value)] => {
            assert_eq!((*sent_field, sent_value), (field, &value));
            *request_id
        }
        _ => panic!("expected one settings change, got {sent:?}"),
    }
}

fn result(
    owner: Uuid,
    request_id: Uuid,
    field: SettingsField,
    generation: u64,
    outcome: SettingsWriteOutcome,
) -> Update {
    Update::SettingsWrite(SettingsWriteResult::ui_test_fixture(
        owner, request_id, field, generation, outcome,
    ))
}

/// What the window draws for `snapshot` alone, without choices.
fn confirmed(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    snapshot: &AppearanceSnapshot,
) -> NativeAppearance {
    window.update(|window, cx| {
        let workspace = view.read(cx);
        NativeAppearance::from_snapshot(
            snapshot,
            &Choices::new(),
            &workspace.themes,
            &window.text_system().all_font_names(),
            &workspace.ui_family,
            &workspace.mono_family,
        )
    })
}

/// The chrome surface, terminal colors and terminal text size the window
/// shows, with the colors and sizes of every terminal it holds.
fn shown(window: &mut VisualTestContext, view: &Entity<Workspace>) -> (u32, u32, f32) {
    window.update(|_, cx| {
        let workspace = view.read(cx);
        for pane in workspace.panes.values() {
            let terminal = pane.terminal.read(cx);
            assert_eq!(terminal.ui_test_colors(), workspace.appearance.terminal);
            assert_eq!(
                f32::from(terminal.ui_test_font().1),
                workspace.appearance.terminal_font_size
            );
        }
        (
            workspace.appearance.surface,
            workspace.appearance.terminal.background,
            workspace.appearance.terminal_font_size,
        )
    })
}

fn status(
    window: &mut VisualTestContext,
    view: &Entity<Workspace>,
    field: SettingsField,
) -> Option<String> {
    window.update(|_, cx| view.read(cx).settings.read(cx).status(field))
}

fn text(value: &str) -> SettingsValue {
    SettingsValue::Text(value.into())
}

/// Scenarios "Choice shown before the write result", "Read while the
/// choice is outstanding" and "Saved value wins": a chosen UI theme shows
/// before any answer and through a read that does not have it yet, without
/// advancing the appearance generation or reaching `last-appearance.json`;
/// the readback shows what the service saved, here another theme.
#[test]
fn a_chosen_theme_shows_at_once_and_gives_way_to_the_saved_value() {
    with_view(|window, view| {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("last-appearance.json");
        window.update(|_, cx| {
            view.update(cx, |workspace, _| {
                workspace.appearance_memory = Some(AppearanceMemory::open(file.clone()));
            })
        });
        let remembered = |window: &mut VisualTestContext| {
            let stored: Value =
                serde_json::from_slice(&std::fs::read(&file).expect("the file is written"))
                    .unwrap();
            let held = window.update(|_, cx| {
                let workspace = view.read(cx);
                let memory = workspace.appearance_memory.as_ref().unwrap();
                memory.stored().unwrap().theme.clone()
            });
            assert_eq!(stored["theme"].as_str(), Some(held.as_str()));
            held
        };
        let (owner, mut commands) = setup(window, &view, |owner| {
            settings_snapshot_with_terminal(owner, 1, json!({}), json!({}))
        });
        assert_eq!(remembered(window), "splitty-salon");
        let light = confirmed(
            window,
            &view,
            &settings_snapshot_with_terminal(
                owner,
                9,
                json!({}),
                json!({"theme": "splitty-light"}),
            ),
        );
        assert_eq!(light.scheme, ColorScheme::Light);

        let field = SettingsField::Theme;
        let request = choose(window, &view, &mut commands, field, text("splitty-light"));
        assert_eq!(
            shown(window, &view).0,
            light.surface,
            "shown before any answer"
        );
        window.update(|_, cx| {
            let workspace = view.read(cx);
            assert_eq!(workspace.appearance.scheme, ColorScheme::Light);
            assert_eq!(workspace.appearance_generation, 1);
        });
        assert!(status(window, &view, field)
            .unwrap()
            .starts_with("Applying"));
        assert_eq!(remembered(window), "splitty-salon");

        // A read that was in flight before the write carries other changes
        // but not the choice: the choice stays drawn over it.
        deliver(
            window,
            &view,
            snapshot(owner, 2, json!({"fontSize": 16}), json!({})),
        );
        // The default palette follows the chosen UI theme.
        assert_eq!(
            shown(window, &view),
            (light.surface, light.terminal.background, 16.)
        );
        assert_eq!(remembered(window), "splitty-salon");
        window.update(|_, cx| assert_eq!(view.read(cx).appearance_generation, 2));

        deliver(
            window,
            &view,
            result(owner, request, field, 3, SettingsWriteOutcome::Confirmed),
        );
        assert_eq!(shown(window, &view).0, light.surface);
        assert_eq!(
            status(window, &view, field).as_deref(),
            Some("Checking saved value…")
        );

        // The readback holds what the service saved, which wins.
        let saved = settings_snapshot_with_terminal(
            owner,
            3,
            json!({"fontSize": 16}),
            json!({"theme": "splitty-classic"}),
        );
        let classic = confirmed(window, &view, &saved);
        assert_ne!(classic.surface, light.surface);
        deliver(window, &view, Update::Appearance(saved));
        assert_eq!(shown(window, &view).0, classic.surface);
        assert_eq!(status(window, &view, field), None);
        assert_eq!(remembered(window), "splitty-classic");
    });
}

/// Scenario "Rejected or uncertain choice": a rejected palette and an
/// uncertain theme return the window and its terminals to the last
/// confirmed appearance, while the editor keeps its note.
#[test]
fn a_rejected_or_uncertain_choice_returns_to_the_confirmed_appearance() {
    with_view(|window, view| {
        let (owner, mut commands) = setup(window, &view, |owner| {
            settings_snapshot_with_terminal(owner, 1, json!({}), json!({}))
        });
        let before = shown(window, &view);
        let palette = window.update(|_, cx| {
            let workspace = view.read(cx);
            workspace
                .themes
                .palettes()
                .iter()
                .find(|palette| palette.colors != workspace.appearance.terminal)
                .expect("another palette")
                .clone()
        });

        let field = SettingsField::TerminalTheme;
        let request = choose(window, &view, &mut commands, field, text(&palette.id));
        let chosen = shown(window, &view);
        assert_eq!(chosen.1, palette.colors.background);
        assert_eq!(chosen.0, before.0, "the palette leaves the chrome alone");
        deliver(
            window,
            &view,
            result(
                owner,
                request,
                field,
                0,
                SettingsWriteOutcome::Rejected {
                    reason: "refused".into(),
                },
            ),
        );
        assert_eq!(shown(window, &view), before);
        assert_eq!(
            status(window, &view, field).as_deref(),
            Some("Couldn’t save: refused")
        );

        let field = SettingsField::Theme;
        let request = choose(window, &view, &mut commands, field, text("splitty-light"));
        assert_ne!(shown(window, &view).0, before.0);
        deliver(
            window,
            &view,
            result(
                owner,
                request,
                field,
                0,
                SettingsWriteOutcome::Uncertain {
                    reason: "reply lost".into(),
                },
            ),
        );
        assert_eq!(shown(window, &view), before);
        assert!(status(window, &view, field)
            .unwrap()
            .starts_with("Save status unknown"));
        window.update(|_, cx| assert_eq!(view.read(cx).appearance_generation, 1));
    });
}

/// Scenario "Terminal font before confirmation": a chosen terminal size
/// measures every terminal at once, and a rejection measures them again
/// with the confirmed size.
#[test]
fn a_terminal_size_choice_measures_the_terminals_and_a_rejection_measures_them_back() {
    with_view(|window, view| {
        let (owner, mut commands) = setup(window, &view, |owner| {
            settings_snapshot_with_terminal(owner, 1, json!({"fontSize": 14}), json!({}))
        });
        assert_eq!(shown(window, &view).2, 14.);
        let field = SettingsField::TerminalFontSize;
        let request = choose(
            window,
            &view,
            &mut commands,
            field,
            SettingsValue::Number(18.),
        );
        assert_eq!(shown(window, &view).2, 18.);
        deliver(
            window,
            &view,
            result(
                owner,
                request,
                field,
                0,
                SettingsWriteOutcome::Rejected {
                    reason: "refused".into(),
                },
            ),
        );
        assert_eq!(shown(window, &view).2, 14.);
    });
}

/// Scenario "OS preference changes while a choice is outstanding": a light
/// theme chosen for automatic mode shows once the read for a light OS
/// preference arrives, in place of the confirmed light theme.
#[test]
fn a_read_for_the_other_os_preference_resolves_the_choice_for_it() {
    with_view(|window, view| {
        let appearance = json!({
            "themeMode": "auto", "themeDark": "splitty-dark", "themeLight": "splitty-light"
        });
        let first = appearance.clone();
        let (owner, mut commands) = setup(window, &view, move |owner| {
            settings_snapshot_for_scheme(owner, 1, ColorScheme::Dark, json!({}), first)
        });
        let dark = shown(window, &view);
        let field = SettingsField::ThemeLight;
        choose(
            window,
            &view,
            &mut commands,
            field,
            text("splitty-light-gray"),
        );
        assert_eq!(
            shown(window, &view),
            dark,
            "the dark preference shows the dark theme"
        );

        let read =
            settings_snapshot_for_scheme(owner, 2, ColorScheme::Light, json!({}), appearance);
        let chosen = json!({
            "themeMode": "auto", "themeDark": "splitty-dark", "themeLight": "splitty-light-gray"
        });
        let gray = confirmed(
            window,
            &view,
            &settings_snapshot_for_scheme(owner, 9, ColorScheme::Light, json!({}), chosen),
        );
        let light = confirmed(window, &view, &read);
        assert_ne!(gray.surface, light.surface);
        deliver(window, &view, Update::Appearance(read));
        assert_eq!(shown(window, &view).0, gray.surface);
    });
}

/// Scenario "Settings outside the choices": an interface style or zoom
/// scope change waits for confirmation.
#[test]
fn other_settings_wait_for_confirmation() {
    with_view(|window, view| {
        let (_, mut commands) = setup(window, &view, |owner| {
            settings_snapshot_with_terminal(owner, 1, json!({}), json!({}))
        });
        let before = window.update(|_, cx| {
            let appearance = &view.read(cx).appearance;
            (appearance.system_style, appearance.zoom)
        });
        choose(
            window,
            &view,
            &mut commands,
            SettingsField::InterfaceStyle,
            text("system"),
        );
        choose(
            window,
            &view,
            &mut commands,
            SettingsField::ZoomScope,
            text("terminals"),
        );
        window.update(|_, cx| {
            let workspace = view.read(cx);
            let appearance = &workspace.appearance;
            assert_eq!((appearance.system_style, appearance.zoom), before);
            assert!(workspace.appearance_choices.values().is_empty());
        });
    });
}
