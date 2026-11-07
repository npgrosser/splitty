//! Passive local metadata, merged once for every kind of client.
use std::collections::HashSet;

use splitty_core::native_api::directory::{
    Directory, DirectoryHost, DirectoryScreen, DirectorySource,
};
use splitty_remote::hosts::Source;

use super::{catalog::CatalogScreen, Hub};

fn source_response(source: Source) -> DirectorySource {
    match source {
        Source::Recent => DirectorySource::Recent,
        Source::SshConfig => DirectorySource::SshConfig,
    }
}

fn screen_response(screen: &CatalogScreen) -> DirectoryScreen {
    DirectoryScreen::new(screen.id.clone(), screen.name.clone(), screen.panes)
}

impl Hub {
    /// Reads metadata only; neither discovery nor saved rules initiate SSH.
    pub fn directory(&self) -> Directory {
        let preferences = self.host_preferences();
        let hidden: HashSet<_> = preferences
            .hosts
            .iter()
            .filter(|p| p.hidden)
            .map(|p| p.target.as_str())
            .collect();
        let catalog = self.catalog();
        let mut hosts: Vec<_> = self
            .known_hosts()
            .into_iter()
            .map(|host| {
                let mut response = DirectoryHost::new(host.target);
                response.alias = host.alias;
                response.source = Some(source_response(host.source));
                response.last_connected = host.last_connected;
                response
            })
            .collect();
        let extra = catalog
            .hosts
            .iter()
            .map(|h| h.target.clone())
            .chain(self.forwards().into_iter().map(|f| f.target))
            .chain(
                self.forward_preferences()
                    .forwards
                    .into_iter()
                    .map(|f| f.target),
            )
            .chain(self.reverses().into_iter().map(|f| f.target))
            .chain(
                self.reverse_preferences()
                    .forwards
                    .into_iter()
                    .map(|f| f.target),
            );
        for target in extra {
            if hidden.contains(target.as_str()) || hosts.iter().any(|h| h.target == target) {
                continue;
            }
            let alias = preferences
                .hosts
                .iter()
                .find(|h| h.target == target)
                .and_then(|h| h.alias.clone());
            let mut response = DirectoryHost::new(target);
            response.alias = alias;
            hosts.push(response);
        }
        for host in &mut hosts {
            if let Some(saved) = catalog.hosts.iter().find(|h| h.target == host.target) {
                host.last_seen = Some(saved.last_seen);
                host.screens = saved.screens.iter().map(screen_response).collect();
            }
        }
        let order = self.host_order().order;
        hosts.sort_by_key(|h| {
            order
                .iter()
                .position(|target| target == &h.target)
                .unwrap_or(usize::MAX)
        });
        Directory::new(order, hosts)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hub::testing::{free_port, ScriptedHost};
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn directory_merges_metadata_without_connecting_and_filters_hidden_tunnel_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join("ssh");
        std::fs::create_dir(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host vm1 vm2\n").unwrap();
        let host = ScriptedHost::new(dir.path());
        let hub = Hub::new(Box::new(host.clone()), dir.path().join("config"), Some(ssh));
        let mut changes = hub.changes();
        hub.update_host_preference(json!({"target":"vm1","alias":"Build"}))
            .unwrap();
        hub.observe_handshake(
            "vm1",
            &[super::super::Screen {
                id: "screen".into(),
                name: Some("Work".into()),
                panes: 2,
            }],
        );
        hub.save_forward("vm1", 5173, None, None).await.unwrap();
        hub.save_reverse("reverse-only", 3000, Some(3000))
            .await
            .unwrap();
        hub.set_host_order(json!({"order":["reverse-only","vm1"]}))
            .unwrap();
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        let directory = hub.directory();
        assert_eq!(
            directory
                .hosts
                .iter()
                .map(|h| h.target.as_str())
                .collect::<Vec<_>>(),
            ["reverse-only", "vm1", "vm2"]
        );
        assert_eq!(directory.hosts[1].alias.as_deref(), Some("Build"));
        assert_eq!(directory.hosts[1].screens[0].panes, 2);
        let wire = serde_json::to_value(&directory).unwrap();
        assert_eq!(wire["version"], 1);
        assert_eq!(wire["order"], json!(["reverse-only", "vm1"]));
        assert_eq!(wire["hosts"][1]["source"], "ssh-config");
        assert_eq!(
            wire["hosts"][1]["screens"],
            json!([{"id":"screen","name":"Work","panes":2}])
        );
        assert_eq!(
            serde_json::from_value::<Directory>(wire).unwrap(),
            directory
        );
        assert!(host.asked().is_empty());
        assert!(
            !changes.has_changed().unwrap(),
            "reads must not publish changes"
        );
        hub.forget_host(json!({"target":"reverse-only"})).unwrap();
        assert!(changes.has_changed().unwrap());
        assert!(!hub
            .directory()
            .hosts
            .iter()
            .any(|h| h.target == "reverse-only"));
        assert_eq!(hub.reverse_preferences().forwards.len(), 1);
    }

    #[test]
    fn handshake_observations_replace_empty_lists_and_reject_invalid_or_hidden_updates() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(
            Box::new(ScriptedHost::new(dir.path())),
            dir.path().join("config"),
            None,
        );
        let screen = super::super::Screen {
            id: "s1".into(),
            name: Some("Before".into()),
            panes: 1,
        };
        hub.observe_handshake("vm1", std::slice::from_ref(&screen));
        assert_eq!(hub.catalog().hosts[0].screens.len(), 1);
        hub.observe_handshake("vm1", &[screen.clone(), screen]);
        assert_eq!(hub.catalog().hosts[0].screens.len(), 1);
        hub.update_catalog(
            json!({"target":"vm1","screens":[{"id":"s1","name":"After","panes":2}]}),
        )
        .unwrap();
        assert_eq!(
            hub.catalog().hosts[0].screens[0].name.as_deref(),
            Some("After")
        );
        hub.observe_handshake("vm1", &[]);
        assert!(hub.catalog().hosts[0].screens.is_empty());
        hub.forget_host(json!({"target":"vm1"})).unwrap();
        hub.observe_handshake("vm1", &[]);
        assert!(hub.catalog().hosts.is_empty());
    }

    #[tokio::test]
    async fn forwarding_changes_include_mutations_and_supervisor_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let host = ScriptedHost::new(dir.path());
        let hub = Hub::new(Box::new(host.clone()), dir.path().join("config"), None);
        let port = free_port();
        let mut changes = hub.changes();
        hub.save_forward("vm1", i64::from(port), None, None)
            .await
            .unwrap();
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        hub.open_forward("vm1", i64::from(port)).await.unwrap();
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        host.fail_next_forwards(1);
        host.end_forward(port);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                changes.changed().await.unwrap();
                if hub.forwards()[0].state == super::super::ForwardState::Down {
                    break;
                }
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                changes.changed().await.unwrap();
                if hub.forwards()[0].state == super::super::ForwardState::Up {
                    break;
                }
            }
        })
        .await
        .unwrap();
        hub.stop_forward("vm1", i64::from(port)).await.unwrap();
        assert!(changes.has_changed().unwrap());
        assert!(hub.forwards().is_empty());
        assert!(!hub.forward_preferences().forwards[0].enabled);
    }
}
