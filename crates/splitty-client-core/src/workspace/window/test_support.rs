//! In-memory canonical Screen fixtures for selected-backend view tests.
use super::*;
use serde_json::json;

impl WindowWorkspace {
    /// Feed bounded discovery through the production observation validator.
    #[doc(hidden)]
    pub fn ui_test_port_observation(
        &mut self,
        id: SourceId,
        ports: &[(u16, &str)],
    ) -> io::Result<()> {
        let mut source = self
            .source_mut(id)
            .ok_or_else(|| invalid("fixture source missing"))?;
        let owner = source.source_token();
        source.observe_ports(owner, &json!({"ports": ports.iter().map(|(port, process)|
            json!({"port":port,"process":process,"sessionId":"fixture-session"})).collect::<Vec<_>>()
        }), &ConnectionBudget::default())?;
        Ok(())
    }

    /// Ordered canonical catalogs for production Drawer interaction tests.
    #[doc(hidden)]
    pub fn ui_test_screen_order_fixture(
        &mut self,
        screens: &[(&str, &str)],
        remote_target: Option<&str>,
    ) -> io::Result<Vec<ScreenKey>> {
        let id = if let Some(target) = remote_target {
            let id = self.admit_remote(self.lifecycle_token(), target)?;
            self.bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())?;
            let owner = self
                .source(id)
                .ok_or_else(|| invalid("fixture source missing"))?
                .source_token();
            self.confirm_ready(owner)?;
            id
        } else {
            self.lifecycle_token().source_id()
        };
        let owner = {
            let mut source = self
                .source_mut(id)
                .ok_or_else(|| invalid("fixture source missing"))?;
            source.set_catalog_supported(true);
            source.set_screen_placement_supported(true);
            source.mark_catalog_dirty();
            source.flush_catalog_request(|| Ok(()))?;
            let owner = source.source_token();
            source.apply_catalog(owner.connection_id(), &json!({
                "type":"mux.workspace_snapshot",
                "screens":screens.iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>(),
                "activeScreenId":screens.first().map(|(id,_)|id)
            }))?;
            owner
        };
        if let Some(action) = self.host(id).map(|host| host.action_target().clone()) {
            self.host_mut(&action)
                .ok_or_else(|| invalid("fixture host missing"))?
                .catalog = CatalogState::Ready;
        }
        let keys = screens
            .iter()
            .map(|(screen, _)| self.source(id).expect("fixture source").screen_key(screen))
            .collect::<Vec<_>>();
        if remote_target.is_some() {
            for key in &keys {
                self.open_existing(owner, key)?;
            }
        }
        Ok(keys)
    }

    #[doc(hidden)]
    pub fn ui_test_host_update_fixture(
        &mut self,
        target: &str,
        legacy: bool,
    ) -> io::Result<HostActionTarget> {
        self.ui_test_remote_terminal_fixture(target)?;
        let id = self
            .source_for_target(target)
            .ok_or_else(|| invalid("host update fixture source missing"))?;
        let action = self
            .host(id)
            .ok_or_else(|| invalid("host update fixture host missing"))?
            .action_target()
            .clone();
        let budget = ConnectionBudget::default();
        let offer = if legacy {
            let migration = serde_json::from_value(json!({
                "available": true,
                "fromVersion": "0.1.0",
                "toVersion": "0.2.4",
                "sessions": 3
            }))
            .map_err(io::Error::other)?;
            let offer =
                host_update::from_failure(Some("0.1.0".into()), Some(true), None, Some(migration))?
                    .ok_or_else(|| invalid("legacy update fixture missing offer"))?;
            let lease = budget
                .reserve(offer.retained_bytes(), false)
                .map_err(io::Error::other)?;
            Arc::new(lease.charge(offer))
        } else {
            let backend = "0.2.4".to_owned();
            let lease = budget
                .reserve(backend.len(), false)
                .map_err(io::Error::other)?;
            self.set_backend_version(
                self.lifecycle_token().connection_id(),
                Ok(Arc::new(lease.charge(backend))),
            );
            HostUpdateOffer::from_ready(
                &json!({
                    "update": {"available": true, "keepsSessions": true},
                    "daemon": {"version": "0.2.3", "keepsSessions": true}
                }),
                &budget,
            )?
            .ok_or_else(|| invalid("ordinary update fixture missing offer"))?
        };
        self.host_mut(&action)
            .ok_or_else(|| invalid("host update fixture host changed"))?
            .update_offer = Some(offer);
        Ok(action)
    }

    #[doc(hidden)]
    pub fn ui_test_host_update_plan(
        &self,
        request_id: Uuid,
        action: &HostActionTarget,
    ) -> io::Result<Arc<HostUpdatePlan>> {
        let offer = self
            .host(action.source_id())
            .filter(|host| host.action_target() == action)
            .and_then(|host| host.update_offer.clone())
            .ok_or_else(|| invalid("host update fixture offer missing"))?;
        Ok(Arc::new(HostUpdatePlan::new(
            request_id,
            action.clone(),
            offer,
            self.backend_version(),
            &ConnectionBudget::default(),
        )?))
    }

    #[doc(hidden)]
    pub fn ui_test_revoke_host_update_offer(
        &mut self,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        self.host_mut(action)
            .ok_or_else(|| invalid("host update fixture host changed"))?
            .update_offer = None;
        Ok(())
    }

    #[doc(hidden)]
    pub fn ui_test_fail_backend_version(&mut self) {
        self.set_backend_version(
            self.lifecycle_token().connection_id(),
            Err(io::Error::other("fixture backend version unavailable")),
        );
    }

    #[doc(hidden)]
    pub fn ui_test_host_update_catalog_failed(
        &mut self,
        action: &HostActionTarget,
    ) -> io::Result<()> {
        self.host_mut(action)
            .ok_or_else(|| invalid("host update fixture host changed"))?
            .catalog = CatalogState::Failed;
        Ok(())
    }

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
