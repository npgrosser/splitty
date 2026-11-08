//! The window shell reads one title projection from the authoritative workspace.
use super::*;

impl Workspace {
    pub(crate) fn window_title(&self) -> String {
        let Some(model) = self.model.as_ref() else {
            return "Splitty".into();
        };
        project_window_title(model, |target| {
            self.hosts
                .accepted_alias(model.lifecycle_token(), target)
                .map(str::to_owned)
        })
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn ui_test_set_title_model(
        &mut self,
        model: WindowWorkspace,
        cx: &mut Context<Self>,
    ) {
        self.model = Some(model);
        cx.notify();
    }
}

fn project_window_title(model: &WindowWorkspace, alias: impl Fn(&str) -> Option<String>) -> String {
    let Some(key) = model.shown_key() else {
        return "Splitty".into();
    };
    let screen = model
        .opened()
        .into_iter()
        .find(|entry| entry.key() == &key)
        .and_then(|entry| entry.name().map(str::to_owned))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| key.screen_id().to_owned());
    let host = model.source_target(key.source_id()).map(|target| {
        alias(target)
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| target.to_owned())
    });
    let screen = hosts::display(&screen);
    let screen = host.map_or(screen.clone(), |host| {
        format!("{screen} @{}", hosts::display(&host))
    });
    let pane = model
        .source(key.source_id())
        .and_then(SourceWorkspace::focused_pane)
        .and_then(|pane| {
            model
                .source(key.source_id())?
                .snapshot()?
                .panes
                .iter()
                .find(|(id, _)| id.as_str() == pane)
                .and_then(|(_, state)| state.title.as_deref())
        })
        .filter(|title| !title.is_empty())
        .map(hosts::display)
        .unwrap_or_else(|| "Terminal".into());
    format!("{screen} · {pane} · Splitty")
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::project_window_title;
    use serde_json::json;
    use splitty_client_core::workspace::{ScreenKey, WindowWorkspace};

    fn set_pane_title(model: &mut WindowWorkspace, key: &ScreenKey, revision: u64, title: &str) {
        let owner = model.source(key.source_id()).unwrap().source_token();
        model
            .source_mut(key.source_id())
            .unwrap()
            .apply_snapshot(
                owner.connection_id(),
                &json!({
                    "type":"mux.screen_snapshot", "screen":{
                        "id":key.screen_id(), "revision":revision,
                        "root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},
                        "panes":{"p0":{"id":"p0","title":title}},
                        "sessions":{}
                    }
                }),
            )
            .unwrap();
    }

    #[test]
    fn window_title_tracks_local_screen_and_live_pane_metadata() {
        let mut model = WindowWorkspace::new(uuid::Uuid::new_v4());
        let local = model.ui_test_local_screen_fixture("Work").unwrap();
        assert_eq!(
            project_window_title(&model, |_| None),
            "Work · Terminal · Splitty"
        );
        set_pane_title(&mut model, &local, 2, "Editor");
        assert_eq!(
            project_window_title(&model, |_| None),
            "Work · Editor · Splitty"
        );
        set_pane_title(&mut model, &local, 3, "Build log");
        assert_eq!(
            project_window_title(&model, |_| None),
            "Work · Build log · Splitty"
        );
        model
            .ui_test_screen_order_fixture(&[("work", "Renamed")], None)
            .unwrap();
        assert_eq!(
            project_window_title(&model, |_| None),
            "Renamed · Build log · Splitty"
        );
        let owner = model.local_source().source_token();
        model
            .source_mut(local.source_id())
            .unwrap()
            .apply_snapshot(
                owner.connection_id(),
                &json!({
                    "type":"mux.screen_snapshot", "screen":{
                        "id":"work", "revision":4,
                        "root":{"type":"slot","id":"slot","paneIds":["p0","p1"],"activePaneId":"p0"},
                        "panes":{"p0":{"id":"p0","title":"Build log"},"p1":{"id":"p1","title":"Editor"}},
                        "sessions":{}
                    }
                }),
            )
            .unwrap();
        assert!(model
            .source_mut(local.source_id())
            .unwrap()
            .select_tab("p1"));
        assert_eq!(
            project_window_title(&model, |_| None),
            "Renamed · Editor · Splitty"
        );
    }

    #[test]
    fn window_title_uses_selected_source_with_colliding_screen_and_pane_ids() {
        let mut model = WindowWorkspace::new(uuid::Uuid::new_v4());
        let local = model.ui_test_local_screen_fixture("Local").unwrap();
        set_pane_title(&mut model, &local, 2, "Local shell");
        let (remote, _) = model
            .ui_test_remote_terminal_fixture("demo@example.test")
            .unwrap();
        set_pane_title(&mut model, &remote, 2, "Remote shell");
        let owner = model.lifecycle_token();
        model.select(owner, &remote).unwrap();
        assert_eq!(
            project_window_title(&model, |_| None),
            "work @demo@example.test · Remote shell · Splitty"
        );
        assert_eq!(
            project_window_title(&model, |_| Some("Staging".into())),
            "work @Staging · Remote shell · Splitty"
        );
        assert_eq!(
            project_window_title(&model, |_| Some("Production".into())),
            "work @Production · Remote shell · Splitty"
        );
        model.select(owner, &local).unwrap();
        assert_eq!(
            project_window_title(&model, |_| Some("Staging".into())),
            "Local · Local shell · Splitty"
        );
    }
}
