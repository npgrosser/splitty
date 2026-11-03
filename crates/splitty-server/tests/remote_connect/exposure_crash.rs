//! Real coordinator death and a deliberately hostile peer. Never substitute
//! Drop for SIGKILL, or remote EOF/heartbeat for observed local worker exit.
use super::*;

fn terminated(pid: i32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    stat.rsplit_once(") ").is_some_and(|(_, fields)| {
        matches!(fields.split_whitespace().next(), Some("Z" | "X" | "x"))
    })
}
fn birth(pid: i32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    stat.rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}
fn until(reason: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct HostileRelay {
    stop: PathBuf,
}
impl Drop for HostileRelay {
    fn drop(&mut self) {
        let _ = fs::write(&self.stop, b"stop");
    }
}
fn hostile_relay(lab: &Lab) -> HostileRelay {
    let stop = lab.dir.path().join("hostile-relay.stop");
    let program = lab.dir.path().join("hostile-relay.py");
    fs::write(
        &program,
        format!(
            r#"import os, time
print("SPLITTY_RELAY_READY_V1", flush=True)
# Ignore stdin and all relay heartbeat semantics; only the fixture's explicit
# teardown flag ends this program after the survival/exposure assertions.
while not os.path.exists({stop:?}): time.sleep(0.05)
"#,
            stop = stop.to_str().unwrap()
        ),
    )
    .unwrap();
    let path = lab.dir.path().join("host-env.sh");
    let original = fs::read_to_string(&path).unwrap();
    let marker = "if [ -n \"$SSH_ORIGINAL_COMMAND\" ]";
    assert!(original.contains(marker));
    let replacement = format!("case \"$SSH_ORIGINAL_COMMAND\" in\n *\" remote-relay serve \"*) exec python3 '{}' ;;\nesac\n{marker}", program.display());
    fs::write(path, original.replacen(marker, &replacement, 1)).unwrap();
    HostileRelay { stop }
}
fn from_host_unix(lab: &Lab, socket: &Path) -> String {
    let script = format!(
        r#"'import socket
s = socket.socket(socket.AF_UNIX)
s.settimeout(5)
s.connect("{}")
print(s.makefile().readline().strip())
'"#,
        socket.display()
    );
    let out = lab.on_host(&["python3", "-c", &script]);
    assert!(out.status.success(), "{}", describe(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
fn browser(lab: &Lab, address: &str) -> Connect {
    let directory = lab.dir.path().join("another-config");
    let mut child = lab
        .client(&["serve", "--oneshot-token", "--addr", address])
        .env("XDG_CONFIG_HOME", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let lines = read_lines(child.stdout.take().unwrap());
    Connect { child, lines }
}
fn opened_port(process: &Connect) -> u16 {
    process
        .until("SPLITTY_PORT=")
        .iter()
        .find_map(|line| line.strip_prefix("SPLITTY_PORT="))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn a_published_reverse_worker_blocks_gateway_reuse_after_its_coordinator_is_killed() {
    let Some(lab) = lab() else { return };
    let payload = lab.payload();
    let hub = HubServer::start(&lab, &payload);
    let (service, local) = local_marker_service();
    let body = serde_json::json!({"target":HOST,"port":18432,"local":local}).to_string();
    let before = lab.ssh_invocations();
    if splitty_remote::tunnel::exposure::check_sharing_supported()
        .is_err_and(|error| error.kind() == io::ErrorKind::Unsupported)
    {
        let (status, response) = reverses(&hub, "POST", Some(&body));
        assert_eq!(status, 501, "{response}");
        assert_eq!(response["code"], "unsupported_environment");
        assert_eq!(lab.ssh_invocations(), before);
        assert!(reverse_ssh(&hub).is_empty());
        assert_ne!(
            std::env::var("SPLITTY_TEST_REQUIRE_NATIVE_EXPOSURE").as_deref(),
            Ok("1"),
            "required native SSH crash scenario cannot use the unsupported path"
        );
        eprintln!("native Linux published-worker crash scenario UNRUN: explicit unsupported/no-SSH refusal verified in this environment");
        return;
    }
    let hostile = hostile_relay(&lab);
    let (status, response) = reverses(&hub, "POST", Some(&body));
    assert_eq!(status, 200, "{response}");
    let workers = reverse_ssh(&hub);
    assert_eq!(workers.len(), 1);
    let worker = workers[0];
    let worker_birth = birth(worker);
    let socket = fs::read_dir(lab.dir.path().join("home/.local/share/splitty/run/relay"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sock")
        })
        .unwrap();
    assert_eq!(from_host_unix(&lab, &socket), "local-service");
    let coordinator = i32::try_from(hub.child.id()).unwrap();
    kill(Pid::from_raw(coordinator), Signal::SIGKILL).unwrap();
    // Keep the direct child unreaped until final cleanup, pinning the owned
    // process-group number. HubServer::Drop must not kill the surviving SSH yet.
    until("coordinator did not terminate", || terminated(coordinator));
    assert!(!terminated(worker));
    assert_eq!(birth(worker), worker_birth);
    assert_eq!(
        from_host_unix(&lab, &socket),
        "local-service",
        "a hostile peer keeps the actual Unix forward usable after coordinator death"
    );
    drop(service);
    let address = format!("127.0.0.1:{local}");
    let mut denied_child = lab
        .client(&["serve", "--oneshot-token", "--addr", &address])
        .env("XDG_CONFIG_HOME", lab.dir.path().join("another-config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = read_lines(denied_child.stdout.take().unwrap());
    let mut denied = Connect {
        child: denied_child,
        lines,
    };
    let mut status = None;
    until(
        "conflicting gateway did not fail before its startup deadline",
        || {
            while let Ok(line) = denied.lines.try_recv() {
                assert!(
                    !line.starts_with("SPLITTY_PORT="),
                    "conflicting gateway unexpectedly opened: {line}"
                );
            }
            status = denied.child.try_wait().unwrap();
            status.is_some()
        },
    );
    assert!(!status.unwrap().success());
    let mut diagnostic = String::new();
    denied
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut diagnostic)
        .unwrap();
    assert!(diagnostic.contains("shared"), "{diagnostic}");
    assert!(
        !lab.dir.path().join("another-config").exists(),
        "refused bind must precede workspace creation"
    );
    let alternate = browser(&lab, "127.0.0.1:0");
    assert_ne!(opened_port(&alternate), local);
    assert!(alternate.terminate().0.success());
    // Ending the remote program is not our proof: independently observe the
    // recorded local worker's actual exit before expecting port release.
    drop(hostile);
    until("local reverse worker did not terminate", || {
        terminated(worker)
    });
    let replacement = browser(&lab, &address);
    assert_eq!(opened_port(&replacement), local);
    assert!(replacement.terminate().0.success());
    eprintln!("NATIVE_SSH_CRASH_POSITIVE_EXERCISED: hostile peer retained forwarding after coordinator death; gateway excluded until actual worker exit");
}
