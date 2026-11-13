//! `remote.autoForwardPorts` in the hub: a port that appears in a host's
//! `ports.changed` after the first list a relay carried is forwarded without
//! a click, once for all windows that show the host at the same time.
//!
//! Every relay keeps its own last list, so each connection's first list,
//! and the first after a reconnect, is a baseline. What was forwarded is
//! kept per host while at least one relay of it runs: a second window that
//! sees the same port appear forwards nothing, and a port the user stopped
//! is not forwarded again for the same process. Lists are followed while
//! the setting is off, so that turning it on forwards only what appears
//! afterwards. Whether a port already has a forward or a paused saved rule
//! is decided by `Forwards::open_automatically`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use splitty_transport::codec::Frame;
use splitty_transport::wire::Kind;

/// How a daemon's `ports.changed` begins: serde writes the tag of its
/// internally tagged events first. Other text frames are not parsed.
const PREFIX: &[u8] = br#"{"type":"ports.changed""#;
/// A larger list is not read; the relay's last list stays as it was.
const MAX_LIST_BYTES: usize = 256 * 1024;
/// At most this many ports of a list are followed, the first ones.
pub(super) const MAX_PORTS: usize = 256;
/// A process name is kept with at most this many characters.
const MAX_PROCESS: usize = 256;

#[derive(Default)]
struct Host {
    /// The relays of the host that run now.
    relays: usize,
    /// The ports forwarded automatically since the first of them started,
    /// with the process that held each.
    forwarded: HashMap<u16, String>,
}

type Hosts = Arc<Mutex<HashMap<String, Host>>>;

/// What the hub's relays have forwarded automatically, per host.
#[derive(Default)]
pub(super) struct AutoForwards {
    hosts: Hosts,
}

impl AutoForwards {
    /// Follow the lists of one relay to `target`, until the watch is dropped.
    pub(super) fn watch(&self, target: &str) -> Watch {
        lock(&self.hosts)
            .entry(target.to_owned())
            .or_default()
            .relays += 1;
        Watch {
            hosts: self.hosts.clone(),
            target: target.to_owned(),
            last: None,
        }
    }
}

/// One relay's view of its host's ports.
pub(super) struct Watch {
    hosts: Hosts,
    target: String,
    /// The ports of the relay's last list; none before its first.
    last: Option<HashSet<u16>>,
}

impl Watch {
    /// Follow `frame` when it is a `ports.changed` the host sent. With the
    /// setting `enabled`, `open` is asked to forward each port that appeared
    /// and was not forwarded for the same process yet; it answers whether
    /// it started one.
    pub(super) fn observe(
        &mut self,
        frame: &Frame,
        enabled: bool,
        open: impl FnMut(&str, u16) -> bool,
    ) {
        if frame.kind != Kind::Text {
            return;
        }
        let Ok((_, bytes)) = frame.application() else {
            return;
        };
        if let Some(ports) = parse(bytes) {
            self.list(ports, enabled, open);
        }
    }

    fn list(
        &mut self,
        ports: Vec<(u16, String)>,
        enabled: bool,
        mut open: impl FnMut(&str, u16) -> bool,
    ) {
        let current: HashSet<u16> = ports.iter().map(|(port, _)| *port).collect();
        let Some(previous) = self.last.replace(current) else {
            return;
        };
        if !enabled {
            return;
        }
        let mut hosts = lock(&self.hosts);
        let Some(host) = hosts.get_mut(&self.target) else {
            return;
        };
        for (port, process) in ports {
            if previous.contains(&port) || host.forwarded.get(&port) == Some(&process) {
                continue;
            }
            if open(&self.target, port) {
                host.forwarded.insert(port, process);
            }
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut hosts = lock(&self.hosts);
        if let Some(host) = hosts.get_mut(&self.target) {
            host.relays = host.relays.saturating_sub(1);
            if host.relays == 0 {
                hosts.remove(&self.target);
            }
        }
    }
}

/// The ports of a `ports.changed` text, each once, at most [`MAX_PORTS`].
fn parse(bytes: &[u8]) -> Option<Vec<(u16, String)>> {
    if !bytes.starts_with(PREFIX) || bytes.len() > MAX_LIST_BYTES {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct PortsChanged {
        ports: Vec<Listening>,
    }
    #[derive(serde::Deserialize)]
    struct Listening {
        port: u16,
        #[serde(default)]
        process: String,
    }
    let list: PortsChanged = serde_json::from_slice(bytes).ok()?;
    let mut seen = HashSet::new();
    Some(
        list.ports
            .into_iter()
            .filter(|entry| entry.port != 0 && seen.insert(entry.port))
            .take(MAX_PORTS)
            .map(|entry| {
                (
                    entry.port,
                    entry.process.chars().take(MAX_PROCESS).collect(),
                )
            })
            .collect(),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(watch: &mut Watch, enabled: bool, ports: &[(u16, &str)]) -> Vec<u16> {
        let mut opened = Vec::new();
        watch.list(
            ports.iter().map(|(p, n)| (*p, (*n).to_owned())).collect(),
            enabled,
            |target, port| {
                assert_eq!(target, "vm1");
                opened.push(port);
                true
            },
        );
        opened
    }

    #[test]
    fn only_ports_after_the_first_list_are_forwarded_once_per_process() {
        let forwards = AutoForwards::default();
        let mut watch = forwards.watch("vm1");
        assert!(
            list(&mut watch, true, &[(3000, "node")]).is_empty(),
            "the first list is a baseline"
        );
        assert_eq!(
            list(&mut watch, true, &[(3000, "node"), (5173, "vite")]),
            [5173]
        );
        assert!(list(&mut watch, true, &[(3000, "node"), (5173, "vite")]).is_empty());
        // The same program listening again is not forwarded a second time.
        assert!(list(&mut watch, true, &[(3000, "node")]).is_empty());
        assert!(list(&mut watch, true, &[(3000, "node"), (5173, "vite")]).is_empty());
        // Another program on the port is.
        list(&mut watch, true, &[(3000, "node")]);
        assert_eq!(
            list(&mut watch, true, &[(3000, "node"), (5173, "python3")]),
            [5173]
        );
    }

    #[test]
    fn lists_are_followed_while_the_setting_is_off() {
        let forwards = AutoForwards::default();
        let mut watch = forwards.watch("vm1");
        list(&mut watch, false, &[(3000, "node")]);
        assert!(list(&mut watch, false, &[(3000, "node"), (8080, "python")]).is_empty());
        assert!(
            list(&mut watch, true, &[(3000, "node"), (8080, "python")]).is_empty(),
            "what appeared while it was off is not forwarded later"
        );
        assert_eq!(
            list(
                &mut watch,
                true,
                &[(3000, "node"), (8080, "python"), (9000, "ruby")]
            ),
            [9000]
        );
    }

    #[test]
    fn relays_of_one_host_share_what_was_forwarded_until_the_last_ends() {
        let forwards = AutoForwards::default();
        let mut first = forwards.watch("vm1");
        let mut second = forwards.watch("vm1");
        list(&mut first, true, &[]);
        list(&mut second, true, &[]);
        assert_eq!(list(&mut first, true, &[(5173, "vite")]), [5173]);
        assert!(
            list(&mut second, true, &[(5173, "vite")]).is_empty(),
            "a second window does not forward it again"
        );
        // A refused port is not recorded and is asked for again.
        let mut refused = Vec::new();
        first.list(
            vec![(5173, "vite".into()), (8000, "api".into())],
            true,
            |_, port| {
                refused.push(port);
                false
            },
        );
        assert_eq!(refused, [8000]);
        drop(first);
        drop(second);
        // A new connection of the host starts with a baseline and no record.
        let mut third = forwards.watch("vm1");
        list(&mut third, true, &[]);
        assert_eq!(list(&mut third, true, &[(5173, "vite")]), [5173]);
    }

    #[test]
    fn only_a_daemon_ports_changed_text_is_read() {
        let frame = |kind, body: &str| {
            let delivery = splitty_transport::wire::Delivery {
                generation: 1,
                receipt: [0; 16],
            };
            Frame::data(kind, delivery, body.as_bytes()).unwrap()
        };
        let text = |body: &str| frame(Kind::Text, body);
        let forwards = AutoForwards::default();
        let mut watch = forwards.watch("vm1");
        let opened = std::cell::RefCell::new(Vec::new());
        let observe = |watch: &mut Watch, frame: &Frame| {
            watch.observe(frame, true, |_, port| {
                opened.borrow_mut().push(port);
                true
            })
        };
        let list = |ports: &str| {
            format!(r#"{{"type":"ports.changed","id":"e1","timestamp":1,"ports":[{ports}]}}"#)
        };
        observe(&mut watch, &text(&list("")));
        // Not a list: neither of these changes the baseline.
        observe(
            &mut watch,
            &text(r#"{"id":"e2","type":"ports.changed","ports":[{"port":1}]}"#),
        );
        observe(
            &mut watch,
            &text(r#"{"type":"ports.changed","ports":"none"}"#),
        );
        observe(&mut watch, &frame(Kind::Binary, &list(r#"{"port":2}"#)));
        observe(
            &mut watch,
            &text(&list(
                r#"{"port":3000,"sessionId":"s1","process":"node"},{"port":3000,"process":"node"},{"port":0}"#,
            )),
        );
        assert_eq!(*opened.borrow(), [3000]);
        let many: Vec<String> = (1..=300)
            .map(|port| format!(r#"{{"port":{port}}}"#))
            .collect();
        observe(&mut watch, &text(&list(&many.join(","))));
        let opened = opened.into_inner();
        assert_eq!(opened.len(), 1 + MAX_PORTS, "the first 256 ports");
        assert!(opened.iter().all(|port| *port == 3000 || *port <= 256));
    }
}
