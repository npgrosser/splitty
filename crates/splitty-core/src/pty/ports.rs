//! The TCP ports the processes of sessions listen on (`mux-protocol`,
//! Listening Ports), so a client can forward them.
//!
//! On Linux the scan reads `/proc`:
//! - `net/tcp` and `net/tcp6` for the sockets in state LISTEN, with their
//!   address, port and inode;
//! - every `<pid>/stat` for the parent pids, which give the process tree of
//!   each session's shell;
//! - the descriptors of those processes (`<pid>/fd/*` linking to
//!   `socket:[<inode>]`) to find the sockets they hold;
//! - `<pid>/comm` for the name of the process that holds one.
//!
//! Only listeners a forward to `localhost` on the host can reach count:
//! loopback and wildcard addresses. Elsewhere the list is always empty.

use serde::Serialize;

use super::Backend;

/// Whether this system has the scan; elsewhere [`listening`] is always
/// empty.
pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// A TCP port a process of a session listens on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    /// The session the process belongs to.
    pub session_id: String,
    /// The process's name, such as `node` or `python3`.
    pub process: String,
}

/// The ports the processes of `backend`'s sessions listen on, in port order,
/// each port once. Reads `/proc`, so it blocks.
pub fn listening(backend: &dyn Backend) -> Vec<ListeningPort> {
    let shells: Vec<(String, u32)> = backend
        .sessions()
        .iter()
        .filter_map(|session| Some((session.session_id().to_owned(), session.shell_pid()?)))
        .collect();
    listening_under(&shells)
}

/// The ports that processes of the trees rooted at `shells`, a session id
/// and its shell's pid each, listen on. A port held in several sessions is
/// listed for the lowest session id.
pub fn listening_under(shells: &[(String, u32)]) -> Vec<ListeningPort> {
    #[cfg(target_os = "linux")]
    {
        linux::scan(std::path::Path::new("/proc"), shells)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = shells;
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::Path;

    use super::ListeningPort;

    /// The state of a listening socket in the tables of `/proc/net`.
    const LISTEN: &str = "0A";

    /// A socket in state LISTEN on an address that counts.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Listener {
        pub inode: u64,
        pub port: u16,
    }

    /// The scan over the `/proc` at `root`.
    pub(super) fn scan(root: &Path, shells: &[(String, u32)]) -> Vec<ListeningPort> {
        if shells.is_empty() {
            return Vec::new();
        }
        let mut ports: HashMap<u64, u16> = HashMap::new();
        for (table, v6) in [("net/tcp", false), ("net/tcp6", true)] {
            // A kernel without IPv6 has no `tcp6`.
            if let Ok(text) = fs::read_to_string(root.join(table)) {
                ports.extend(
                    listeners(&text, v6)
                        .into_iter()
                        .map(|listener| (listener.inode, listener.port)),
                );
            }
        }
        // Most scans end here: nothing listens on loopback at all.
        if ports.is_empty() {
            return Vec::new();
        }

        let children = children(root);
        // (port, session, pid) of every listener a session's process holds.
        let mut held: Vec<(u16, &str, u32)> = Vec::new();
        for (session, shell) in shells {
            for pid in tree(&children, *shell) {
                for inode in socket_inodes(root, pid) {
                    if let Some(&port) = ports.get(&inode) {
                        held.push((port, session, pid));
                    }
                }
            }
        }
        // The lowest session id, and within it the lowest pid (a server's
        // parent before the workers it forked), names a port.
        held.sort_unstable();
        held.dedup_by_key(|(port, _, _)| *port);
        held.into_iter()
            .filter_map(|(port, session, pid)| {
                // A process that ended meanwhile has closed its sockets.
                let process = fs::read_to_string(root.join(pid.to_string()).join("comm")).ok()?;
                Some(ListeningPort {
                    port,
                    session_id: session.to_owned(),
                    process: process.trim_end_matches('\n').to_owned(),
                })
            })
            .collect()
    }

    /// The listeners of a `/proc/net/tcp` (`v6` false) or `/proc/net/tcp6`
    /// table whose address is loopback or a wildcard.
    pub(super) fn listeners(table: &str, v6: bool) -> Vec<Listener> {
        // After the header: `sl local_address rem_address st … uid timeout
        // inode …`, with the local address as `<hex address>:<hex port>`.
        table
            .lines()
            .skip(1)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let (local, state, inode) = (fields.get(1)?, fields.get(3)?, fields.get(9)?);
                if *state != LISTEN {
                    return None;
                }
                let (address, port) = local.split_once(':')?;
                let address = if v6 {
                    IpAddr::V6(ipv6(address)?)
                } else {
                    IpAddr::V4(ipv4(address)?)
                };
                if !reachable(address) {
                    return None;
                }
                Some(Listener {
                    inode: inode.parse().ok()?,
                    port: u16::from_str_radix(port, 16).ok()?,
                })
            })
            .collect()
    }

    /// An IPv4 address as the kernel prints it: the 32-bit word of the
    /// address in network order, read as a number of this machine and
    /// printed in hex, so `127.0.0.1` is `0100007F` on a little-endian one.
    fn ipv4(hex: &str) -> Option<Ipv4Addr> {
        word(hex).map(|word| Ipv4Addr::from(word.to_ne_bytes()))
    }

    /// An IPv6 address as the kernel prints it: four such words, so `::1`
    /// is `00000000000000000000000001000000` on a little-endian machine.
    fn ipv6(hex: &str) -> Option<Ipv6Addr> {
        if hex.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let start = index * 8;
            *chunk = word(hex.get(start..start + 8)?)?.to_ne_bytes();
        }
        Some(Ipv6Addr::from(bytes))
    }

    fn word(hex: &str) -> Option<u32> {
        if hex.len() != 8 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    }

    /// Loopback (`127.0.0.0/8`, `::1`, v4-mapped loopback) or a wildcard
    /// (`0.0.0.0`, `::`).
    pub(super) fn reachable(address: IpAddr) -> bool {
        match address {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
        }
    }

    /// The parent pid in a `/proc/<pid>/stat`: the field after the state,
    /// which follows the process's name in parentheses. The name may hold
    /// parentheses and spaces itself, so the last `)` ends it.
    pub(super) fn parent(stat: &str) -> Option<u32> {
        let rest = stat.get(stat.rfind(')')? + 1..)?;
        let mut fields = rest.split_whitespace();
        let _state = fields.next()?;
        fields.next()?.parse().ok()
    }

    /// The children of every process, from the parent pids in `/proc`.
    fn children(root: &Path) -> HashMap<u32, Vec<u32>> {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        let Ok(entries) = fs::read_dir(root) else {
            return children;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            // A process that ended meanwhile has no stat any more.
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            if let Some(parent) = parent(&stat) {
                children.entry(parent).or_default().push(pid);
            }
        }
        children
    }

    /// `root` and all its descendants.
    pub(super) fn tree(children: &HashMap<u32, Vec<u32>>, root: u32) -> Vec<u32> {
        let mut seen = HashSet::from([root]);
        let mut queue = VecDeque::from([root]);
        let mut tree = Vec::new();
        while let Some(pid) = queue.pop_front() {
            tree.push(pid);
            for &child in children.get(&pid).into_iter().flatten() {
                if seen.insert(child) {
                    queue.push_back(child);
                }
            }
        }
        tree
    }

    /// The inodes of the sockets `pid` holds. The descriptors of another
    /// user's process cannot be read; it holds none of these sessions'.
    fn socket_inodes(root: &Path, pid: u32) -> Vec<u64> {
        let Ok(entries) = fs::read_dir(root.join(pid.to_string()).join("fd")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let link = fs::read_link(entry.path()).ok()?;
                link.to_str()?
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')?
                    .parse()
                    .ok()
            })
            .collect()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener};
    use std::path::Path;

    use super::linux::{listeners, parent, reachable, scan, tree, Listener};
    use super::*;

    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";

    /// A table of `/proc/net/tcp` or `tcp6` with one socket per (address,
    /// port, state, inode).
    fn table(sockets: &[(&str, &str, &str, u64)]) -> String {
        let mut text = format!("{HEADER}\n");
        for (index, (address, port, state, inode)) in sockets.iter().enumerate() {
            text.push_str(&format!(
                "   {index}: {address}:{port} 00000000:0000 {state} 00000000:00000000 00:00000000 \
                 00000000  1000        0 {inode} 1 0000000000000000 100 0 0 10 0\n"
            ));
        }
        text
    }

    fn listener(inode: u64, port: u16) -> Listener {
        Listener { inode, port }
    }

    // The fixtures are tables of a little-endian machine, where every
    // Splitty host runs.

    /// mux-protocol, Listening Ports: loopback and the wildcard count, a
    /// specific address (Listener on a specific address) and other states do
    /// not.
    #[test]
    fn ipv4_listeners_on_loopback_and_the_wildcard_count() {
        let text = table(&[
            ("0100007F", "1435", "0A", 101), // 127.0.0.1:5173
            ("00000000", "0BB8", "0A", 102), // 0.0.0.0:3000
            ("3600007F", "0035", "0A", 103), // 127.0.0.54:53
            ("0500000A", "1F90", "0A", 104), // 10.0.0.5:8080
            ("0100007F", "1435", "01", 105), // an established connection
            ("0100007F", "C350", "06", 0),   // TIME_WAIT
        ]);
        assert_eq!(
            listeners(&text, false),
            [listener(101, 5173), listener(102, 3000), listener(103, 53)]
        );
    }

    #[test]
    fn ipv6_listeners_on_loopback_the_wildcard_and_mapped_loopback_count() {
        let text = table(&[
            ("00000000000000000000000001000000", "1435", "0A", 201), // ::1
            ("00000000000000000000000000000000", "0BB8", "0A", 202), // ::
            ("0000000000000000FFFF00000100007F", "2328", "0A", 203), // ::ffff:127.0.0.1
            ("0000000000000000FFFF00000500000A", "1F90", "0A", 204), // ::ffff:10.0.0.5
            ("D400032000F92F371F4BAAF84EA69107", "0016", "0A", 205), // a global address
            ("000080FE00000000FF005450B6AD1DFE", "0050", "0A", 206), // fe80::…
        ]);
        assert_eq!(
            listeners(&text, true),
            [
                listener(201, 5173),
                listener(202, 3000),
                listener(203, 9000)
            ]
        );
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let text = format!(
            "{HEADER}\n   0: 0100007F 00000000:0000 0A\n   1: 0100007F:XYZ0 00000000:0000 0A 0 0 0 0 0 7\n   \
             2: 01007F:1435 00000000:0000 0A 0 0 0 0 0 8\n   3: 0100007F:1435 00000000:0000 0A 0 0 0 0 0 x\n\n"
        );
        assert_eq!(listeners(&text, false), []);
        assert_eq!(listeners("", false), []);
    }

    #[test]
    fn reachable_addresses_are_loopback_and_wildcards() {
        for (address, expected) in [
            ("127.0.0.1", true),
            ("127.1.2.3", true),
            ("0.0.0.0", true),
            ("10.0.0.5", false),
            ("::1", true),
            ("::", true),
            ("::ffff:127.0.0.1", true),
            ("::ffff:10.0.0.5", false),
            ("fe80::1", false),
        ] {
            let parsed: IpAddr = address.parse().expect("address");
            assert_eq!(reachable(parsed), expected, "{address}");
        }
        assert!(reachable(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!reachable(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn the_parent_follows_a_name_with_parentheses_and_spaces() {
        assert_eq!(parent("4711 (node) S 4700 4711 4700 0 -1"), Some(4700));
        assert_eq!(
            parent("4712 (my (odd) name) R 1 4712 4712 0 -1 4194304"),
            Some(1)
        );
        assert_eq!(parent("4713 (cut"), None);
        assert_eq!(parent(""), None);
    }

    #[test]
    fn a_tree_is_its_root_and_every_descendant() {
        let children = HashMap::from([(10, vec![11, 12]), (12, vec![13]), (20, vec![21])]);
        let mut found = tree(&children, 10);
        found.sort_unstable();
        assert_eq!(found, [10, 11, 12, 13]);
        assert_eq!(tree(&children, 13), [13]);
    }

    /// A `/proc` under `root` with the socket tables `tcp` and `tcp6` and
    /// the processes `(pid, parent, name, socket inodes)`.
    fn fake_proc(root: &Path, tcp: &str, tcp6: &str, processes: &[(u32, u32, &str, &[u64])]) {
        std::fs::create_dir_all(root.join("net")).expect("net");
        std::fs::write(root.join("net/tcp"), tcp).expect("tcp");
        std::fs::write(root.join("net/tcp6"), tcp6).expect("tcp6");
        for (pid, parent, name, inodes) in processes {
            let dir = root.join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).expect("fd");
            std::fs::write(
                dir.join("stat"),
                format!("{pid} ({name}) S {parent} {pid} {pid} 0 -1"),
            )
            .expect("stat");
            std::fs::write(dir.join("comm"), format!("{name}\n")).expect("comm");
            std::os::unix::fs::symlink("/dev/null", dir.join("fd/0")).expect("fd 0");
            for (index, inode) in inodes.iter().enumerate() {
                std::os::unix::fs::symlink(
                    format!("socket:[{inode}]"),
                    dir.join(format!("fd/{}", index + 3)),
                )
                .expect("socket fd");
            }
        }
    }

    /// The listeners of the shells' descendants, ordered by port, each port
    /// once for the lowest session id; other processes' listeners and
    /// specific addresses are left out.
    #[test]
    fn the_scan_maps_the_sessions_listeners_to_their_ports() {
        let dir = tempfile::tempdir().expect("dir");
        let tcp = table(&[
            ("0100007F", "1435", "0A", 101), // 127.0.0.1:5173, vite in s2
            ("0500000A", "1F90", "0A", 102), // 10.0.0.5:8080, in s1
            ("00000000", "0BB8", "0A", 103), // 0.0.0.0:3000, node in s1 and s2
            ("0100007F", "1538", "0A", 104), // 127.0.0.1:5432, outside the sessions
        ]);
        let tcp6 = table(&[
            ("00000000000000000000000001000000", "0BB8", "0A", 201), // [::1]:3000, in s2
            ("00000000000000000000000000000000", "2328", "0A", 202), // [::]:9000, the worker
        ]);
        fake_proc(
            dir.path(),
            &tcp,
            &tcp6,
            &[
                (100, 1, "bash", &[]),
                (110, 100, "node", &[103, 102]),
                (111, 110, "node-worker", &[202]),
                (200, 1, "zsh", &[]),
                (210, 200, "vite", &[101, 201]),
                (300, 1, "postgres", &[104]),
            ],
        );
        let shells = [("s2".to_owned(), 200), ("s1".to_owned(), 100)];
        let found = scan(dir.path(), &shells);
        let port = |port: u16, session: &str, process: &str| ListeningPort {
            port,
            session_id: session.to_owned(),
            process: process.to_owned(),
        };
        assert_eq!(
            found,
            [
                port(3000, "s1", "node"),
                port(5173, "s2", "vite"),
                port(9000, "s1", "node-worker"),
            ]
        );
        assert_eq!(
            serde_json::to_value(&found[0]).expect("JSON"),
            serde_json::json!({"port": 3000, "sessionId": "s1", "process": "node"})
        );

        assert_eq!(scan(dir.path(), &[]), []);
        assert_eq!(scan(dir.path(), &[("s3".to_owned(), 999)]), []);
    }

    /// The real `/proc`: a listener of this process, on loopback, is found
    /// when this process is a session's shell, and is gone once it closed.
    #[test]
    fn a_listener_of_this_process_is_found_in_the_real_proc() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("address").port();
        let name = std::fs::read_to_string("/proc/self/comm").expect("comm");
        let shells = [("me".to_owned(), std::process::id())];

        let found = listening_under(&shells);
        assert!(
            found.contains(&ListeningPort {
                port,
                session_id: "me".to_owned(),
                process: name.trim_end().to_owned(),
            }),
            "{port} in {found:?}"
        );

        drop(listener);
        assert!(
            !listening_under(&shells)
                .iter()
                .any(|found| found.port == port),
            "{port} is closed"
        );
    }
}
