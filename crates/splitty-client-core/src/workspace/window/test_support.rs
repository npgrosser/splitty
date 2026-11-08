//! In-memory canonical Screen fixtures for selected-backend view tests.
use super::*;
use serde_json::json;

impl WindowWorkspace {
    #[doc(hidden)]
    pub fn ui_test_local_screen_fixture(&mut self, name: &str) -> io::Result<ScreenKey> {
        let owner = self.lifecycle_token();
        {
            let mut source = self
                .source_mut(owner.source_id())
                .ok_or_else(|| invalid("local fixture source missing"))?;
            source.set_catalog_supported(true);
            source.mark_catalog_dirty();
            source.flush_catalog_request(|| Ok(()))?;
            source.apply_catalog(
                owner.connection_id(),
                &json!({
                    "type": "mux.workspace_snapshot",
                    "screens": [{"id": "work", "name": name}],
                    "activeScreenId": "work"
                }),
            )?;
            source.apply_snapshot(
                owner.connection_id(),
                &json!({"type":"mux.screen_snapshot","screen":{
                    "id":"work","revision":1,
                    "root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},
                    "panes":{"p0":{"id":"p0"}},
                    "sessions":{}
                }}),
            )?;
        }
        Ok(self.local_source().screen_key("work"))
    }

    #[doc(hidden)]
    pub fn ui_test_remote_terminal_fixture(
        &mut self,
        target: &str,
    ) -> io::Result<(ScreenKey, TerminalToken)> {
        let id = if let Some(id) = self.source_for_target(target) {
            self.disconnect(id)?;
            self.finish_attempt(id)?;
            self.retry(id)?;
            id
        } else {
            self.admit_remote(self.lifecycle_token(), target)?
        };
        self.bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())?;
        let owner = self
            .source(id)
            .ok_or_else(|| invalid("remote fixture source missing"))?
            .source_token();
        self.confirm_ready(owner)?;
        {
            let mut source = self
                .source_mut(id)
                .ok_or_else(|| invalid("remote fixture source missing"))?;
            source.set_catalog_supported(true);
            source.mark_catalog_dirty();
            source.flush_catalog_request(|| Ok(()))?;
            source.apply_catalog(
                owner.connection_id(),
                &json!({
                    "type":"mux.workspace_snapshot", "screens":[{"id":"work","name":"work"}],
                    "activeScreenId":"work"
                }),
            )?;
        }
        let action = self
            .host(id)
            .ok_or_else(|| invalid("remote fixture host missing"))?
            .action_target()
            .clone();
        self.host_mut(&action)
            .ok_or_else(|| invalid("remote fixture host missing"))?
            .catalog = CatalogState::Ready;
        let key = self
            .source(id)
            .ok_or_else(|| invalid("remote fixture source missing"))?
            .screen_key("work");
        self.open_existing(owner, &key)?;
        let event = json!({"type":"mux.screen_snapshot","screen":{
            "id":"work","revision":1,
            "root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},
            "panes":{"p0":{"id":"p0","sessionId":"s-p0"}},
            "sessions":{"s-p0":{"id":"s-p0","status":"running"}}
        }});
        let terminal = {
            let mut source = self
                .source_mut(id)
                .ok_or_else(|| invalid("remote fixture source missing"))?;
            source.apply_snapshot(owner.connection_id(), &event)?;
            let terminal = source
                .terminal_token("p0")
                .ok_or_else(|| invalid("remote fixture terminal missing"))?;
            source.observe_attached(owner.connection_id(), terminal.session_id());
            terminal
        };
        Ok((key, terminal))
    }
}
