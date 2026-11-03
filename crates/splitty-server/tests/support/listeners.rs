//! Actual TCP listeners owned by a process, including IPv6. Inspection failure
//! is a failed smoke check, never evidence that the process has no listener.
use std::net::SocketAddr;

pub fn tcp_listeners(pid: u32) -> Vec<SocketAddr> {
    #[cfg(target_os = "linux")]
    let mut listeners = {
        use std::{
            collections::HashSet,
            fs,
            net::{IpAddr, Ipv4Addr, Ipv6Addr},
        };
        let sockets: HashSet<String> = fs::read_dir(format!("/proc/{pid}/fd"))
            .expect("process fd inventory")
            .map(|entry| entry.expect("process fd entry"))
            .filter_map(|entry| match fs::read_link(entry.path()) {
                Ok(path) => Some(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None, // fd closed during inventory
                Err(error) => panic!("cannot inspect process fd: {error}"),
            })
            .filter_map(|path| {
                path.to_str()
                    .and_then(|path| path.strip_prefix("socket:[")?.strip_suffix(']'))
                    .map(str::to_owned)
            })
            .collect();
        let mut listeners = Vec::new();
        for table in ["tcp", "tcp6"] {
            let text = fs::read_to_string(format!("/proc/{pid}/net/{table}"))
                .expect("process TCP inventory");
            for line in text.lines().skip(1) {
                let fields: Vec<_> = line.split_whitespace().collect();
                assert!(fields.len() >= 10, "invalid TCP inventory row: {line}");
                if fields[3] != "0A" || !sockets.contains(fields[9]) {
                    continue;
                }
                let (address, port) = fields[1].split_once(':').expect("listener address");
                let port = u16::from_str_radix(port, 16).expect("listener port");
                let ip = if table == "tcp" {
                    IpAddr::V4(Ipv4Addr::from(
                        u32::from_str_radix(address, 16)
                            .expect("IPv4 address")
                            .to_ne_bytes(),
                    ))
                } else {
                    assert_eq!(address.len(), 32);
                    let mut bytes = [0; 16];
                    for (index, chunk) in bytes.chunks_exact_mut(4).enumerate() {
                        chunk.copy_from_slice(
                            &u32::from_str_radix(&address[index * 8..index * 8 + 8], 16)
                                .expect("IPv6 address")
                                .to_ne_bytes(),
                        );
                    }
                    IpAddr::V6(Ipv6Addr::from(bytes))
                };
                listeners.push(SocketAddr::new(ip, port));
            }
        }
        listeners
    };
    #[cfg(not(target_os = "linux"))]
    let mut listeners = {
        let process = nix::unistd::Pid::from_raw(pid.try_into().expect("process ID"));
        nix::sys::signal::kill(process, None).expect("live process before listener inventory");
        let output = std::process::Command::new("lsof")
            .args([
                "-nP",
                "-a",
                "-p",
                &pid.to_string(),
                "-iTCP",
                "-sTCP:LISTEN",
                "-Fn",
            ])
            .output()
            .expect("process listener inventory");
        assert!(
            output.status.success()
                || (output.status.code() == Some(1)
                    && output.stdout.is_empty()
                    && output.stderr.is_empty()),
            "lsof failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        nix::sys::signal::kill(process, None).expect("live process after listener inventory");
        String::from_utf8(output.stdout)
            .expect("listener inventory encoding")
            .lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(|address| {
                address
                    .parse::<SocketAddr>()
                    .unwrap_or_else(|_| panic!("unexpected listener address: {address}"))
            })
            .collect::<Vec<_>>()
    };
    listeners.sort();
    listeners
}

#[test]
fn inventory_observes_owned_ipv4_and_ipv6_listeners() {
    let ipv4 = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let ipv6 = std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)).unwrap();
    let listeners = tcp_listeners(std::process::id());
    assert!(
        listeners.contains(&ipv4.local_addr().unwrap()),
        "IPv4 listener missing from OS inventory"
    );
    assert!(
        listeners.contains(&ipv6.local_addr().unwrap()),
        "IPv6 listener missing from OS inventory"
    );
}
