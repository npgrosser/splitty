//! OS smoke: a genuinely different user, rather than mocked credentials, must
//! be rejected by both filesystem permissions and the server's credential gate.
#![cfg(unix)]
use splitty_server::native::endpoint;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

fn other_user(script: &str, path: &Path) -> Command {
    let mut command = if let Ok(distribution) = std::env::var("WSL_DISTRO_NAME") {
        let mut command = Command::new("wsl.exe");
        command.args([
            "--distribution",
            &distribution,
            "--user",
            "nobody",
            "--cd",
            "/tmp",
            "--exec",
        ]);
        command
    } else {
        let mut command = Command::new("sudo");
        command.args(["-n", "-u", "nobody", "--"]);
        command
    };
    command
        .args(["/usr/bin/python3", "-c", script])
        .arg(path)
        .current_dir("/tmp")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn another_os_user_cannot_reach_application_data_even_after_modes_are_loosened() {
    // macOS $TMPDIR has private ancestors that would defeat the second,
    // deliberately loosened-mode probe before peer credentials are tested.
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
    let private = root.join("private");
    let path = private.join(endpoint::SOCKET_NAME);
    let cwd = std::env::current_dir().unwrap();
    let bound = endpoint::bind(&path).await.unwrap();
    let client = endpoint::connect(&path).await.unwrap();
    let (peer, _) = bound.listener.accept().await.unwrap();
    endpoint::verify_peer(&client).unwrap();
    endpoint::verify_peer(&peer).unwrap();
    drop((client, peer));
    assert_eq!(std::env::current_dir().unwrap(), cwd);
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o775
    );
    let probe = tokio::time::timeout(
        Duration::from_secs(20),
        other_user("import os; print(os.geteuid())", &path).output(),
    )
    .await;
    let Ok(Ok(probe)) = probe else {
        eprintln!("native endpoint alternate-user smoke unavailable: configure passwordless sudo or WSL interop");
        return;
    };
    if !probe.status.success() {
        eprintln!(
            "native endpoint alternate-user smoke unavailable: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        return;
    }
    let uid = String::from_utf8(probe.stdout)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    assert_ne!(
        uid,
        nix::unistd::geteuid().as_raw(),
        "the smoke must actually use another OS user"
    );
    let denied = tokio::time::timeout(
        Duration::from_secs(20),
        other_user(
            r#"
import socket, sys
s = socket.socket(socket.AF_UNIX)
try: s.connect(sys.argv[1])
except PermissionError: sys.exit(0)
sys.exit(17)
"#,
            &path,
        )
        .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        denied.status.success(),
        "private directory/socket allowed another user: {:?}",
        denied.status
    );

    // Deliberately damage only this temporary fixture's modes to test the
    // independent credential check. No application frame may be returned.
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    let mut command = other_user(
        r#"
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(5)
s.connect(sys.argv[1])
try:
 s.sendall(b'SPLITTY\x01')
 data = s.recv(1)
except (ConnectionResetError, BrokenPipeError): data = b''
sys.exit(0 if data == b'' else 19)
"#,
        &path,
    );
    let (child, rejected) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(command.output(), async {
            let (stream, _) = bound.listener.accept().await.unwrap();
            let rejected = endpoint::verify_peer(&stream).is_err();
            drop(stream);
            rejected
        })
    })
    .await
    .unwrap();
    assert!(rejected, "peer credentials accepted another OS user");
    assert!(
        child.unwrap().status.success(),
        "other user received application bytes"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
