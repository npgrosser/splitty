//! `remote.autoForwardPorts`: a port that appears on a host after its first
//! discovery list of a connection is forwarded once, without a notice.
use crate::{
    ports::{ForwardRequest, ForwardSnapshot, ForwardState, MappingKey},
    workspace::{SourceToken, WindowWorkspace},
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Seen {
    /// The ports of the last discovery list.
    ports: HashSet<u16>,
    /// Ports forwarded for this connection, with the process that held them.
    forwarded: HashMap<u16, String>,
}

/// What each host connection has shown so far. A new connection of a host
/// has another token, so its first list is a baseline again.
#[derive(Default)]
pub(super) struct AutoForward {
    hosts: HashMap<SourceToken, Seen>,
}

impl AutoForward {
    /// The forwards to start for the ports that appeared since the last call.
    /// Lists are followed while the setting is off, so that turning it on
    /// forwards only what appears afterwards.
    pub(super) fn appeared(
        &mut self,
        enabled: bool,
        window: &WindowWorkspace,
        forwards: Option<&ForwardSnapshot>,
    ) -> Vec<ForwardRequest> {
        let mut requests = Vec::new();
        let mut live = HashSet::new();
        let rows = forwards.map(|forwards| forwards.rows(window));
        for host in window.hosts() {
            let Some(source) = window.source(host.action_target().source_id()) else {
                continue;
            };
            let (Some(observation), Some(target)) = (source.port_observation(), source.target())
            else {
                continue;
            };
            let token = observation.source();
            live.insert(token);
            let current: HashSet<u16> = observation.ports().iter().map(|p| p.port()).collect();
            let Some(seen) = self.hosts.get_mut(&token) else {
                self.hosts.insert(
                    token,
                    Seen {
                        ports: current,
                        forwarded: HashMap::new(),
                    },
                );
                continue;
            };
            let previous = std::mem::replace(&mut seen.ports, current);
            let Some(rows) = rows.as_ref().filter(|_| enabled) else {
                continue;
            };
            for port in observation.ports() {
                if previous.contains(&port.port())
                    || seen.forwarded.get(&port.port()).map(String::as_str) == Some(port.process())
                {
                    continue;
                }
                let Ok(key) = MappingKey::new(target, port.port()) else {
                    continue;
                };
                // A forward that is up needs nothing; a paused rule stays paused.
                let state = rows
                    .iter()
                    .find(|row| *row.key() == key)
                    .map(|row| row.state());
                if matches!(
                    state,
                    Some(ForwardState::Up | ForwardState::Rebuilding | ForwardState::Paused)
                ) {
                    continue;
                }
                if let Ok(request) = ForwardRequest::start(key, None, false, None) {
                    seen.forwarded
                        .insert(port.port(), port.process().to_owned());
                    requests.push(request.automatic());
                }
            }
        }
        self.hosts.retain(|token, _| live.contains(token));
        requests
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn only_ports_after_the_first_list_are_forwarded_once_per_connection() {
        let mut window = WindowWorkspace::new(Uuid::new_v4());
        let owner = window.lifecycle_token();
        let (screen, _) = crate::workspace::remote_terminal_fixture(&mut window, "vm1");
        let id = screen.source_id();
        let forwards = ForwardSnapshot::new(owner, true);
        let mut auto = AutoForward::default();
        let mut step = |window: &WindowWorkspace, enabled| -> Vec<u16> {
            auto.appeared(enabled, window, Some(&forwards))
                .iter()
                .map(|request| request.key().remote())
                .collect()
        };
        assert!(step(&window, true).is_empty(), "no discovery list yet");
        window
            .ui_test_port_observation(id, &[(3000, "node")])
            .unwrap();
        assert!(
            step(&window, true).is_empty(),
            "the first list is a baseline"
        );
        window
            .ui_test_port_observation(id, &[(3000, "node"), (5173, "vite")])
            .unwrap();
        assert_eq!(step(&window, true), [5173]);
        assert!(step(&window, true).is_empty());
        // The same program listening again is not forwarded a second time.
        window
            .ui_test_port_observation(id, &[(3000, "node")])
            .unwrap();
        assert!(step(&window, true).is_empty());
        window
            .ui_test_port_observation(id, &[(3000, "node"), (5173, "vite")])
            .unwrap();
        assert!(step(&window, true).is_empty());
        // Off: the lists are followed, nothing is forwarded, also not later.
        window
            .ui_test_port_observation(id, &[(3000, "node"), (5173, "vite"), (8080, "python")])
            .unwrap();
        assert!(step(&window, false).is_empty());
        assert!(step(&window, true).is_empty());
    }
}
