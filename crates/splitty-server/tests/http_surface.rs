//! End-to-end assertions about the server's externally reachable HTTP surface.
//!
//! These tests boot the real `splitty serve` binary on a random loopback port,
//! as an explicitly requested browser gateway, and then make real requests
//! against it. They therefore cover the full middleware stack (localhost guard,
//! CORS, auth) rather than a hand-assembled router.
//!
//! The child process gets a throwaway `HOME`/`XDG_*` so it never touches the
//! developer's real `~/.config/splitty`.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[path = "support/command.rs"]
mod isolated_command;
#[cfg(target_os = "linux")]
#[path = "support/processes.rs"]
mod process_cleanup;
#[cfg(unix)]
#[path = "support/listeners.rs"]
mod process_inventory;

const START_TIMEOUT: Duration = Duration::from_secs(20);

struct TestServer {
    child: Child,
    port: u16,
    /// The single-use code the launcher would embed in its URL.
    bootstrap: String,
    /// The process token, as a CLI or native launcher would use it.
    token: String,
    _home: TempDir,
}

impl TestServer {
    fn start() -> Self {
        Self::start_options(&[])
    }
    fn start_options(options: &[&str]) -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let bin = home.path().join("opener-bin");
        std::fs::create_dir(&bin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in [
                "xdg-open",
                "gio",
                "gnome-open",
                "kde-open",
                "wslview",
                "open",
                "powershell.exe",
            ] {
                let path = bin.join(name);
                let body = if name == "powershell.exe" {
                    "printf '%s\\n' \"$OPEN_RS_TARGET\""
                } else {
                    "printf '%s\\n' \"$@\""
                };
                std::fs::write(
                    &path,
                    format!(
                        "#!/bin/sh\n{body} >> '{}'\n",
                        home.path().join("opened").display()
                    ),
                )
                .unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        // Deliberately without `--oneshot-token`: the desktop launchers use that
        // flag, but then the process token is never written anywhere and these
        // tests could not exercise the `Authorization: Bearer` path that the CLI
        // relies on. The bootstrap flow under test is identical either way.
        let mut child = Command::new(env!("CARGO_BIN_EXE_splitty"))
            .args(["serve", "--addr", "127.0.0.1:0"])
            .args(options)
            .env("PATH", path)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_RUNTIME_DIR", home.path().join("run"))
            // Inherited values would otherwise make the child adopt the parent's
            // credential instead of minting its own.
            .env_remove("SPLITTY_TOKEN")
            .env_remove("SPLITTY_AGENT_SOCK")
            .env_remove("SPLITTY_SOCK")
            .env_remove("SPLITTY_SERVER")
            .env_remove("SPLITTY_PANE_ID")
            .env_remove("SPLITTY_SCREEN_ID")
            .env_remove("SPLITTY_SESSION_ID")
            .env_remove("SPLITTY_ADDR")
            .stdout(Stdio::piped())
            // Kept rather than discarded. When the child dies mid-test the only
            // symptom a request sees is a reset connection, which says nothing
            // about the cause; `Drop` replays this file whenever a test panics.
            .stderr(Stdio::from(
                File::create(home.path().join("server.stderr")).expect("stderr log"),
            ))
            .spawn()
            .expect("spawn splitty serve");

        let stdout = child.stdout.take().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut port = None;
        let mut bootstrap = None;

        let deadline = Instant::now() + START_TIMEOUT;
        while (port.is_none() || bootstrap.is_none()) && Instant::now() < deadline {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if let Some(v) = line.strip_prefix("SPLITTY_PORT=") {
                port = v.trim().parse::<u16>().ok();
            } else if let Some(v) = line.strip_prefix("SPLITTY_BOOTSTRAP=") {
                bootstrap = Some(v.trim().to_string());
            }
        }

        let (port, bootstrap) = match (port, bootstrap) {
            (Some(p), Some(b)) => (p, b),
            _ => {
                let _ = child.kill();
                let stderr = read_log(home.path().join("server.stderr"));
                panic!(
                    "server did not report a port and bootstrap code within {START_TIMEOUT:?}\n\
                     server stderr:\n{stderr}"
                );
            }
        };

        // The real launchers keep reading stdout for the backend's lifetime.
        // Dropping the read end here would instead hand the server EPIPE on its
        // next line, so drain it into the throwaway home and keep the pipe open.
        let stdout_log = home.path().join("server.stdout");
        std::thread::spawn(move || {
            let mut sink = match File::create(stdout_log) {
                Ok(file) => file,
                Err(_) => return,
            };
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let _ = sink.write_all(&buf[..n]);
                    }
                }
            }
        });

        // `serve` no longer prints the process token, so read it from the file
        // the server persisted — the same path the CLI discovers it through.
        // That this works at all is the point: the credential lives in a
        // 0600 file, not on a stream that ends up in logs.
        let token = if let Some(pair) = options.windows(2).find(|pair| pair[0] == "--token") {
            pair[1].to_owned()
        } else if options.contains(&"--oneshot-token") {
            String::new()
        } else {
            std::fs::read_to_string(home.path().join("config/splitty/auth-token"))
                .map(|s| s.trim().to_string())
                .expect("server should have persisted an auth token")
        };

        #[cfg(unix)]
        assert_eq!(
            process_inventory::tcp_listeners(child.id()),
            [std::net::SocketAddr::from(([127, 0, 0, 1], port))],
            "an explicit browser gateway owns exactly its advertised loopback listener"
        );

        Self {
            child,
            port,
            bootstrap,
            token,
            _home: home,
        }
    }

    /// Perform the bootstrap exchange and return a client holding the session
    /// cookie, exactly as the UI does on startup.
    async fn authenticated_client(&self) -> reqwest::Client {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .cookie_store(true)
            .build()
            .expect("client");

        let response = client
            .post(self.url("/api/session"))
            .header("Origin", self.own_origin())
            .json(&serde_json::json!({ "bootstrap": self.bootstrap }))
            .send()
            .await
            .expect("bootstrap exchange");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::OK,
            "bootstrap exchange failed"
        );

        client
    }

    /// Fetch a single-use WebSocket ticket through the authenticated path.
    async fn ws_ticket(&self) -> String {
        let client = self.authenticated_client().await;
        let response = client
            .post(self.url("/api/ws-ticket"))
            .header("Origin", self.own_origin())
            .send()
            .await
            .expect("ticket request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);

        let body: serde_json::Value = response.json().await.expect("ticket json");
        body["ticket"].as_str().expect("ticket field").to_string()
    }

    /// The throwaway home the child process was given.
    fn home_path(&self) -> &Path {
        self._home.path()
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn own_origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Perform a raw WebSocket handshake and return the response status line.
    ///
    /// `reqwest` cannot express this: the point is to control `Origin` exactly,
    /// including omitting it, which no browser-shaped client allows.
    fn ws_handshake(&self, query: &str, origin: Option<&str>) -> String {
        self.ws_handshake_with(query, origin, "")
    }

    /// [`Self::ws_handshake`] with additional raw header lines, each ending in
    /// `\r\n`.
    fn ws_handshake_with(&self, query: &str, origin: Option<&str>, extra_headers: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

        let origin_header = match origin {
            Some(value) => format!("Origin: {value}\r\n"),
            None => String::new(),
        };
        let request = format!(
            "GET /ws{query} HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             {origin_header}{extra_headers}\r\n",
            port = self.port,
        );
        stream.write_all(request.as_bytes()).expect("write");

        let mut buf = [0u8; 256];
        let read = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..read])
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// A client that does not follow redirects, so status codes are observed
    /// exactly as the server produced them.
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // A failing test here usually fails *because* the child went away, and a
        // reset connection carries none of that context. Report whether it is
        // still alive and what it said before reaping it.
        if std::thread::panicking() {
            let liveness = match self.child.try_wait() {
                Ok(None) => "still running".to_string(),
                Ok(Some(status)) => format!("already exited: {status}"),
                Err(error) => format!("unknown: {error}"),
            };
            eprintln!(
                "── splitty serve diagnostics (port {}) ──\n\
                 process: {liveness}\n\
                 stderr:\n{}\n\
                 stdout after startup:\n{}\n\
                 ── end diagnostics ──",
                self.port,
                read_log(self._home.path().join("server.stderr")),
                read_log(self._home.path().join("server.stdout")),
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one of the child's captured streams, tolerating its absence.
///
/// Only ever called on a path that is already failing, so a missing or
/// unreadable file must not mask the original panic with a second one.
fn read_log(path: PathBuf) -> String {
    match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => "<empty>".to_string(),
        Ok(text) => text.trim_end().to_string(),
        Err(error) => format!("<unreadable: {error}>"),
    }
}

/// SR-01 — the routes that served foreign web content must not exist.
///
/// 404 rather than 401/403 is the point: the routes and their handlers were
/// removed, so there is nothing to authenticate against. A valid credential is
/// supplied precisely to show that authentication is not what is rejecting the
/// request.
#[tokio::test]
async fn webview_routes_are_absent_even_with_a_valid_token() {
    let server = TestServer::start();
    let client = TestServer::client();

    let paths = [
        "/proxy/8080/",
        "/api/favicon?url=http://example.com",
        "/api/frame-check?url=http://example.com",
        "/api/webapps",
    ];

    for path in paths {
        let response = client
            .get(server.url(path))
            .bearer_auth(&server.token)
            .send()
            .await
            .unwrap_or_else(|e| panic!("request to {path} failed: {e}"));

        assert_eq!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "{path} must not be routable (security review SR-01/SR-08)",
        );
    }
}

/// The SPA fallback must still serve the app shell for real app routes.
///
/// Regression guard for the `SERVER_PATH_PREFIXES` check that makes the removed
/// routes answer 404: it must not swallow ordinary client-side routes.
#[tokio::test]
async fn spa_fallback_still_serves_the_app() {
    let server = TestServer::start();
    let client = TestServer::client();

    for path in ["/", "/some/client/side/route"] {
        let response = client.get(server.url(path)).send().await.expect("request");
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
        assert!(
            response
                .text()
                .await
                .unwrap_or_default()
                .contains("<!doctype html"),
            "{path} should return the app shell",
        );
    }
}

/// The endpoints that remain must still authenticate — a guard against a fix
/// that removes routes by accidentally loosening the middleware instead.
#[tokio::test]
async fn surviving_api_still_requires_authentication() {
    let server = TestServer::start();
    let client = TestServer::client();

    let anonymous = client
        .get(server.url("/api/stats"))
        .send()
        .await
        .expect("request");
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);

    let authenticated = client
        .get(server.url("/api/stats"))
        .bearer_auth(&server.token)
        .send()
        .await
        .expect("request");
    assert_eq!(authenticated.status(), reqwest::StatusCode::OK);
}

// ── SR-07: origin, CORS and binding invariants ──────────────────────────────

/// The finding this closes: section 4.2 of the review recorded a successful
/// WebSocket upgrade (HTTP 101) with a valid token and `Origin:
/// https://evil.example`. That is what let a *leaked* token stay useful — any
/// page the user later visits could open the socket and drive every terminal.
#[tokio::test]
async fn websocket_rejects_foreign_origin() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;
    let query = format!("?ticket={ticket}");

    let status = server.ws_handshake(&query, Some("https://evil.example"));
    assert!(
        status.contains("403"),
        "foreign origin must be rejected, got: {status:?}",
    );
}

/// Browsers always send `Origin` on a WebSocket handshake, so its absence means
/// a non-browser caller. Nothing in Splitty needs one — the CLI speaks HTTP.
#[tokio::test]
async fn websocket_rejects_missing_origin() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;
    let query = format!("?ticket={ticket}");

    let status = server.ws_handshake(&query, None);
    assert!(
        status.contains("403"),
        "missing origin must be rejected, got: {status:?}",
    );
}

/// The legitimate path must keep working: same-origin handshake with a valid
/// token upgrades. Without this, the two rejection tests above could be passing
/// for the wrong reason.
#[tokio::test]
async fn websocket_accepts_own_origin() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;
    let query = format!("?ticket={ticket}");

    let status = server.ws_handshake(&query, Some(&server.own_origin()));
    assert!(
        status.contains("101"),
        "same-origin handshake must upgrade, got: {status:?}",
    );
}

/// Origin is checked before the token, so a foreign page cannot even probe
/// whether a guessed token is valid.
#[tokio::test]
async fn websocket_rejects_foreign_origin_before_checking_token() {
    let server = TestServer::start();

    let status = server.ws_handshake("?ticket=not-a-real-ticket", Some("https://evil.example"));
    assert!(
        status.contains("403"),
        "origin must be rejected regardless of token validity, got: {status:?}",
    );
}

/// A cross-origin fetch carrying a valid credential must not reach the API.
#[tokio::test]
async fn api_rejects_foreign_origin() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .get(server.url("/api/stats"))
        .bearer_auth(&server.token)
        .header("Origin", "https://evil.example")
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

/// A request with no `Origin` at all is still served: browsers omit it on
/// same-origin GETs and the CLI never sends one. Rejecting it here would break
/// both without closing anything — a cross-origin browser request always
/// carries an origin.
#[tokio::test]
async fn api_allows_absent_origin_for_non_browser_clients() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .get(server.url("/api/stats"))
        .bearer_auth(&server.token)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

/// Send an API request with the session cookie and the given `Sec-Fetch-Site`,
/// the way a browser would on behalf of a page.
async fn api_status_with_fetch_site(server: &TestServer, fetch_site: &str) -> reqwest::StatusCode {
    let client = server.authenticated_client().await;
    client
        .get(server.url("/api/home/settings"))
        .header("Sec-Fetch-Site", fetch_site)
        .send()
        .await
        .expect("request")
        .status()
}

/// A page on another loopback port is *same-site* with Splitty: the browser
/// attaches the session cookie, and a no-cors GET carries no `Origin`. Fetch
/// Metadata is what tells such a page apart from the app itself.
#[tokio::test]
async fn api_rejects_same_site_fetch_metadata() {
    let server = TestServer::start();
    assert_eq!(
        api_status_with_fetch_site(&server, "same-site").await,
        reqwest::StatusCode::FORBIDDEN,
    );
}

#[tokio::test]
async fn api_rejects_cross_site_fetch_metadata() {
    let server = TestServer::start();
    assert_eq!(
        api_status_with_fetch_site(&server, "cross-site").await,
        reqwest::StatusCode::FORBIDDEN,
    );
}

/// The UI never reaches the API through a navigation, so a URL typed into the
/// address bar (`none`) is not a legitimate caller either.
#[tokio::test]
async fn api_rejects_navigation_fetch_metadata() {
    let server = TestServer::start();
    assert_eq!(
        api_status_with_fetch_site(&server, "none").await,
        reqwest::StatusCode::FORBIDDEN,
    );
}

#[tokio::test]
async fn api_accepts_same_origin_fetch_metadata() {
    let server = TestServer::start();
    assert_eq!(
        api_status_with_fetch_site(&server, "same-origin").await,
        reqwest::StatusCode::OK,
    );
}

/// The WebSocket gets the same treatment, even with a valid ticket and the
/// app's own `Origin`.
#[tokio::test]
async fn websocket_rejects_same_site_fetch_metadata() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;

    let status = server.ws_handshake_with(
        &format!("?ticket={ticket}"),
        Some(&server.own_origin()),
        "Sec-Fetch-Site: same-site\r\n",
    );
    assert!(
        status.contains("403"),
        "same-site handshake must be refused, got: {status:?}"
    );
}

#[tokio::test]
async fn websocket_accepts_same_origin_fetch_metadata() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;

    let status = server.ws_handshake_with(
        &format!("?ticket={ticket}"),
        Some(&server.own_origin()),
        "Sec-Fetch-Site: same-origin\r\n",
    );
    assert!(
        status.contains("101"),
        "same-origin handshake must upgrade, got: {status:?}"
    );
}

/// No response may advertise a wildcard CORS allowance in any mode.
#[tokio::test]
async fn no_response_carries_wildcard_cors() {
    let server = TestServer::start();
    let client = TestServer::client();

    for path in ["/health", "/api/stats"] {
        let response = client
            .get(server.url(path))
            .bearer_auth(&server.token)
            .header("Origin", &server.own_origin())
            .send()
            .await
            .expect("request");

        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "{path} must not emit CORS headers",
        );
    }
}

/// The invariant that actually keeps the service off the network: a routable
/// bind address is refused at startup rather than accepted with a header check
/// as the only guard.
#[test]
fn refuses_to_bind_to_a_routable_address() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_splitty"))
        .args(["serve", "--addr", "0.0.0.0:0"])
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .output()
        .expect("run splitty");

    assert!(!output.status.success(), "startup must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("loopback"),
        "error must explain the loopback invariant, got: {stderr}",
    );
}

/// Direct remote mode is refused with a pointer to the supported alternative,
/// rather than silently serving plaintext credentials over the network.
#[test]
fn refuses_direct_remote_mode() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_splitty"))
        .args(["serve", "--addr", "127.0.0.1:0", "--remote"])
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .output()
        .expect("run splitty");

    assert!(!output.status.success(), "startup must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("splitty connect"),
        "error must name the supported alternative, got: {stderr}",
    );
}

/// The `Host` guard is a DNS-rebinding control and must stay effective.
#[tokio::test]
async fn rejects_foreign_host_header() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .get(server.url("/health"))
        .header("Host", "evil.example")
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

// ── SR-04: credential architecture ──────────────────────────────────────────

/// A bootstrap code is spent on first use.
///
/// This is what makes capturing the launch URL — from a log, a screenshot, a
/// crash report — worthless after startup (security review SR-04).
#[tokio::test]
async fn bootstrap_code_cannot_be_replayed() {
    let server = TestServer::start();
    let client = TestServer::client();

    let exchange = || {
        client
            .post(server.url("/api/session"))
            .header("Origin", server.own_origin())
            .json(&serde_json::json!({ "bootstrap": server.bootstrap }))
            .send()
    };

    assert_eq!(
        exchange().await.expect("first").status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        exchange().await.expect("replay").status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a bootstrap code must work exactly once",
    );
}

/// The session cookie must be `HttpOnly`.
///
/// Cookies are not port-scoped, so a page served from any other `127.0.0.1:*`
/// port is same-site with Splitty. Without `HttpOnly` its script could read
/// `document.cookie` and take full terminal control — which is exactly what the
/// removed `splitty-token` cookie allowed.
#[tokio::test]
async fn session_cookie_is_httponly() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .post(server.url("/api/session"))
        .header("Origin", server.own_origin())
        .json(&serde_json::json!({ "bootstrap": server.bootstrap }))
        .send()
        .await
        .expect("exchange");

    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie header");

    assert!(
        cookie.contains("HttpOnly"),
        "cookie must be HttpOnly, got: {cookie}"
    );
    assert!(
        cookie.contains("SameSite=Strict"),
        "cookie must be SameSite=Strict, got: {cookie}"
    );
    assert!(
        cookie.starts_with("splitty-session-"),
        "unexpected cookie name: {cookie}"
    );
}

/// The query parameter is no longer an authentication source.
///
/// It used to be, which put the credential into the address bar, browser
/// history, `Referer` headers and every access log.
#[tokio::test]
async fn query_parameter_token_is_not_accepted() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .get(server.url(&format!("/api/stats?token={}", server.token)))
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// The old script-readable cookie is no longer an authentication source either.
#[tokio::test]
async fn legacy_token_cookie_is_not_accepted() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .get(server.url("/api/stats"))
        .header("Cookie", format!("splitty-token={}", server.token))
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// A session established through the bootstrap exchange authenticates the API.
#[tokio::test]
async fn session_cookie_authenticates_the_api() {
    let server = TestServer::start();
    let client = server.authenticated_client().await;

    let response = client
        .get(server.url("/api/stats"))
        .header("Origin", server.own_origin())
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

/// Tickets are single-use: a replayed handshake must fail even from the correct
/// origin. Otherwise a ticket observed once would be as good as a token.
#[tokio::test]
async fn websocket_ticket_cannot_be_replayed() {
    let server = TestServer::start();
    let ticket = server.ws_ticket().await;
    let query = format!("?ticket={ticket}");

    let first = server.ws_handshake(&query, Some(&server.own_origin()));
    assert!(
        first.contains("101"),
        "first use must upgrade, got: {first:?}"
    );

    let replay = server.ws_handshake(&query, Some(&server.own_origin()));
    assert!(
        replay.contains("401"),
        "replay must be rejected, got: {replay:?}"
    );
}

/// Minting a ticket requires an established session — a caller holding nothing
/// cannot obtain one.
#[tokio::test]
async fn ws_ticket_requires_a_session() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client
        .post(server.url("/api/ws-ticket"))
        .header("Origin", server.own_origin())
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// A foreign origin cannot mint a ticket, which is what keeps the WebSocket
/// closed to it even though WebSockets bypass CORS.
#[tokio::test]
async fn foreign_origin_cannot_mint_a_ticket() {
    let server = TestServer::start();
    let client = server.authenticated_client().await;

    let response = client
        .post(server.url("/api/ws-ticket"))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

/// Startup output must not contain a reusable credential.
///
/// The Windows launcher mirrors this stream into its debug log, so anything
/// printed here can end up in a support bundle.
#[test]
fn startup_output_contains_no_reusable_credential() {
    let server = TestServer::start();

    // The process token is never printed; only a single-use bootstrap code is.
    assert!(
        !server.token.is_empty(),
        "test setup should have found the token file"
    );
    assert_ne!(
        server.bootstrap, server.token,
        "the bootstrap code must not be the process token",
    );
}

// ── SR-11: content security policy and security headers ─────────────────────

/// The application document must carry a deny-by-default CSP.
///
/// `frame-src`/`frame-ancestors 'none'` are the machine-checked half of SR-01: if
/// an iframe path ever comes back by accident, the browser refuses it even
/// though the code would allow it.
#[tokio::test]
async fn app_document_carries_a_strict_csp() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client.get(server.url("/")).send().await.expect("request");
    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .expect("CSP header")
        .to_string();

    // Scripts: own origin only. WebAssembly compilation is allowed (the ghostty
    // renderer is a wasm module), but never string evaluation or inline script.
    let script_src = csp
        .split(';')
        .map(str::trim)
        .find(|directive| directive.starts_with("script-src "))
        .expect("script-src directive");
    assert_eq!(script_src, "script-src 'self' 'wasm-unsafe-eval'");

    for directive in [
        "default-src 'none'",
        "frame-src 'none'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'none'",
    ] {
        assert!(
            csp.contains(directive),
            "CSP must contain {directive:?}, got: {csp}"
        );
    }

    // connect-src must name this server's own port and nothing else, so the page
    // cannot open a socket or fetch to a foreign host.
    assert!(
        csp.contains(&format!("ws://127.0.0.1:{}", server.port)),
        "connect-src must allow the server's own WebSocket, got: {csp}",
    );
    assert!(
        !csp.contains('*'),
        "no directive may use a wildcard, got: {csp}"
    );
}

/// The remaining baseline headers must be present on the app document.
#[tokio::test]
async fn app_document_carries_baseline_security_headers() {
    let server = TestServer::start();
    let client = TestServer::client();

    let response = client.get(server.url("/")).send().await.expect("request");
    let headers = response.headers();

    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
    );
    // The app URL briefly carries a bootstrap code; it must never travel in a
    // Referer to anything the user navigates to.
    assert_eq!(
        headers.get("referrer-policy").and_then(|v| v.to_str().ok()),
        Some("no-referrer"),
    );
    let permissions = headers
        .get("permissions-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        permissions.contains("clipboard-read=()"),
        "clipboard read must be denied at the platform level too, got: {permissions}",
    );
    // A foreign page that opens the app gets no handle on it, and no other
    // origin may embed the responses through a no-cors load.
    assert_eq!(
        headers
            .get("cross-origin-opener-policy")
            .and_then(|v| v.to_str().ok()),
        Some("same-origin"),
    );
    assert_eq!(
        headers
            .get("cross-origin-resource-policy")
            .and_then(|v| v.to_str().ok()),
        Some("same-origin"),
    );
}

/// Static assets get the same treatment — a policy that only covers the HTML
/// entry point is easy to sidestep.
#[tokio::test]
async fn static_assets_carry_the_security_headers() {
    let server = TestServer::start();
    let client = TestServer::client();

    let index = client.get(server.url("/")).send().await.expect("request");
    let body = index.text().await.unwrap_or_default();

    // Pick any asset the built index references.
    let asset = body
        .split('"')
        .find(|part| part.starts_with("/assets/") && part.ends_with(".js"))
        .expect("index should reference a built asset");

    let response = client.get(server.url(asset)).send().await.expect("request");
    assert_eq!(response.status(), reqwest::StatusCode::OK, "{asset}");
    assert!(
        response.headers().get("content-security-policy").is_some(),
        "{asset} must carry a CSP",
    );
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
        "{asset} must be nosniff",
    );
}

// ── SR-05: local control channel ────────────────────────────────────────────

/// Minimal HTTP-over-Unix-socket request, mirroring what the CLI does.
fn control_request(socket: &Path, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).expect("connect to control socket");
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    let payload = body.unwrap_or("");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: splitty\r\nConnection: close\r\n");
    if body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    request.push_str("\r\n");
    request.push_str(payload);

    stream.write_all(request.as_bytes()).expect("write");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).ok();

    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    (status, raw)
}

/// Find the control socket a test server created.
fn control_socket_of(server: &TestServer) -> PathBuf {
    let dir = server.home_path().join("run/splitty");
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("control socket dir {} missing: {e}", dir.display()));

    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "sock"))
        .unwrap_or_else(|| panic!("no socket in {}", dir.display()))
}

/// The multiplexer operations the in-shell CLI needs are reachable, with no
/// credential of any kind — filesystem permissions are the authorisation.
#[tokio::test]
async fn control_socket_serves_mux_operations() {
    let server = TestServer::start();
    let socket = control_socket_of(&server);

    let (status, body) = control_request(&socket, "GET", "/api/mux/status", None);
    assert_eq!(
        status, 200,
        "control socket should answer mux status: {body}"
    );
}

/// The heart of SR-05: this channel must not reach anything beyond the
/// multiplexer. Configuration and shell profiles are executable configuration —
/// reaching them would let a build script in a pane persist a change that runs
/// in every future pane, which is the escalation the channel exists to remove.
#[tokio::test]
async fn control_socket_cannot_reach_privileged_endpoints() {
    let server = TestServer::start();
    let socket = control_socket_of(&server);

    let forbidden = [
        ("GET", "/api/home/settings"),
        ("GET", "/api/shell-profiles"),
        ("GET", "/api/home/theme"),
        ("GET", "/api/templates"),
        ("GET", "/api/stats"),
        // Minting browser credentials must stay on the token-authenticated path.
        ("POST", "/api/bootstrap"),
        ("POST", "/api/ws-ticket"),
        ("POST", "/api/session"),
        // A pane process must not open SSH connections or read the user's hosts.
        ("GET", "/ws/remote?target=vm1"),
        ("GET", "/api/remote/hosts"),
    ];

    for (method, path) in forbidden {
        let (status, _) = control_request(&socket, method, path, Some("{}"));
        assert_eq!(
            status, 404,
            "{method} {path} must not be routable over the control socket",
        );
    }
}

/// The socket and its directory must be private to the owner. This is the
/// primary authorisation for the channel, so it is asserted rather than assumed.
#[tokio::test]
async fn control_socket_is_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;

    let server = TestServer::start();
    let socket = control_socket_of(&server);

    let dir_mode = std::fs::metadata(socket.parent().unwrap())
        .expect("socket dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "the socket directory must not be traversable by others"
    );

    let socket_mode = std::fs::metadata(&socket)
        .expect("socket")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        socket_mode, 0o600,
        "the socket itself must not be connectable by others"
    );
}

/// A real grant listener has its own ACL and routes, not a wrapper-side filter.
#[tokio::test]
async fn agent_control_socket_is_private_and_only_routes_owned_jobs() {
    use std::os::unix::fs::PermissionsExt;
    let server = TestServer::start();
    let owner = control_socket_of(&server);
    let (status, response) = control_request(
        &owner,
        "POST",
        "/api/agents",
        Some(r#"{"name":"test-agent"}"#),
    );
    assert_eq!(status, 200);
    let grant: serde_json::Value =
        serde_json::from_str(response.split_once("\r\n\r\n").expect("body").1).expect("grant");
    let socket = PathBuf::from(grant["socket"].as_str().expect("socket"));
    assert_ne!(socket, owner);
    assert_eq!(
        std::fs::metadata(&socket)
            .expect("mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(control_request(&socket, "GET", "/api/jobs", None).0, 200);
    for path in ["/api/mux/state", "/api/agents", "/api/bootstrap", "/ws"] {
        assert_eq!(control_request(&socket, "GET", path, None).0, 404);
    }
    let (status, _) = control_request(
        &owner,
        "POST",
        &format!(
            "/api/agents/{}/revoke",
            grant["agentId"].as_str().expect("id")
        ),
        Some("{}"),
    );
    assert_eq!(status, 200);
    assert!(!socket.exists());
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn agent_socket_rejects_another_uid_even_if_file_modes_are_loosened() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    if !nix::unistd::geteuid().is_root() {
        eprintln!("skipping cross-UID subprocess: requires root; owner modes and route gates are tested separately");
        return;
    }
    let server = TestServer::start();
    let owner = control_socket_of(&server);
    let (_, response) = control_request(
        &owner,
        "POST",
        "/api/agents",
        Some(r#"{"name":"peer-check"}"#),
    );
    let grant: serde_json::Value =
        serde_json::from_str(response.split_once("\r\n\r\n").expect("body").1).expect("grant");
    let socket = PathBuf::from(grant["socket"].as_str().expect("socket"));
    let mut parent = socket.parent();
    while let Some(path) = parent {
        if !path.starts_with(server.home_path()) {
            break;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o711))
            .expect("test traversal");
        parent = path.parent();
    }
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666))
        .expect("test socket mode");
    let script="import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(5)\ntry:s.connect(sys.argv[1])\nexcept PermissionError:sys.exit(4)\ntry:\n s.sendall(b'GET /api/jobs HTTP/1.1\\r\\nHost: localhost\\r\\n\\r\\n');data=s.recv(1024)\nexcept (ConnectionResetError,BrokenPipeError):data=b''\nsys.exit(1 if data else 0)";
    let output = Command::new("python3")
        .args(["-c", script])
        .arg(&socket)
        .uid(65534)
        .gid(65534)
        .output()
        .expect("different uid");
    assert_eq!(
        output.status.code(),
        Some(0),
        "peer gate did not reject: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// control-api: `create_screen` for a screen that still has panes is refused
/// like any invalid op, and leaves the screen as it was (workspace-model,
/// Screen Creation).
#[tokio::test]
async fn create_screen_over_a_screen_with_panes_is_rejected() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let create = |pane: &str| {
        client
            .post(server.url("/api/mux/op"))
            .bearer_auth(&server.token)
            .json(&serde_json::json!({
                "screenId": "s-api",
                "op": { "type": "create_screen", "screenId": "s-api", "newPaneId": pane },
            }))
            .send()
    };

    let first = create("p-first").await.expect("first create_screen");
    assert_eq!(first.status(), reqwest::StatusCode::OK);

    let second = create("p-second").await.expect("second create_screen");
    assert_eq!(second.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = second.json().await.expect("error body");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "body: {body}",
    );

    let screen: serde_json::Value = client
        .get(server.url("/api/mux/screen/s-api"))
        .bearer_auth(&server.token)
        .send()
        .await
        .expect("screen")
        .json()
        .await
        .expect("screen json");
    let panes = screen["panes"].as_object().expect("panes");
    assert!(
        panes.contains_key("p-first") && !panes.contains_key("p-second"),
        "panes: {panes:?}"
    );
}

// Smoke: a private child process owns the entire admission pool, so this can
// exhaust real idle WebSockets without borrowing capacity from parallel tests.
#[tokio::test]
async fn idle_browser_streams_are_bounded_and_closing_one_restores_capacity() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error, Message};
    let server = TestServer::start();
    let browser = server.authenticated_client().await;
    let open = || async {
        let ticket = browser
            .post(server.url("/api/ws-ticket"))
            .header("Origin", server.own_origin())
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        let mut request = format!(
            "ws://127.0.0.1:{}/ws?ticket={}",
            server.port,
            ticket["ticket"].as_str().unwrap()
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("Origin", server.own_origin().parse().unwrap());
        tokio_tungstenite::connect_async(request).await
    };
    let mut streams = Vec::new();
    for _ in 0..32 {
        let (mut stream, _) = open().await.unwrap();
        let welcome = stream.next().await.unwrap().unwrap();
        let Message::Text(text) = welcome else {
            panic!("missing welcome");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["type"],
            "mux.welcome"
        );
        streams.push(stream);
    }
    match open().await {
        Err(Error::Http(response)) => assert_eq!(response.status(), 503),
        _ => panic!("the 33rd stream must be refused before service creation"),
    }
    let mut closed = streams.pop().unwrap();
    closed.close(None).await.unwrap();
    drop(closed);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut replacement = loop {
        match open().await {
            Ok((stream, _)) => break stream,
            Err(Error::Http(response)) if response.status() == 503 => {
                assert!(
                    Instant::now() < deadline,
                    "closing a stream did not release its capacity"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("replacement failed: {error}"),
        }
    };
    let Message::Text(welcome) = replacement.next().await.unwrap().unwrap() else {
        panic!("replacement welcome");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&welcome).unwrap()["type"],
        "mux.welcome"
    );
    replacement
        .send(Message::Text(r#"{"type":"mux.resync"}"#.into()))
        .await
        .unwrap();
    replacement.close(None).await.unwrap();
    for mut stream in streams {
        stream.close(None).await.unwrap();
    }
}

#[test]
fn removed_desktop_stdio_and_busy_browser_binds_fail_before_creating_a_workspace() {
    let home = tempfile::tempdir().unwrap();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = held.local_addr().unwrap().to_string();
    for (args, expected, code) in [
        (
            vec!["serve", "--addr", "127.0.0.1:0", "--desktop-stdio"],
            "desktop-backend --transport native-v1",
            2,
        ),
        (vec!["serve", "--addr", address.as_str()], "cannot bind", 1),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let stdout = home.path().join("command.stdout");
        let stderr = home.path().join("command.stderr");
        let child = command
            .args(args)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_RUNTIME_DIR", home.path().join("run"))
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        #[cfg(target_os = "linux")]
        let group = child.id();
        let status = isolated_command::wait(child);
        let output = std::process::Output {
            status,
            stdout: std::fs::read(stdout).unwrap(),
            stderr: std::fs::read(stderr).unwrap(),
        };
        #[cfg(target_os = "linux")]
        process_cleanup::assert_group_exited(group);
        assert_eq!(output.status.code(), Some(code));
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("SPLITTY_PORT="));
        assert!(
            !home.path().join("config").exists(),
            "refused startup must not create workspace/credentials"
        );
        assert!(!home.path().join("run").exists());
    }
}

/// A leaking descendant can retain diagnostic descriptors after its command
/// exits. A real child is needed to prove the inventory remains reachable.
#[cfg(target_os = "linux")]
#[test]
fn inherited_diagnostics_do_not_block_failed_command_inventory() {
    use std::os::unix::process::CommandExt;
    let fixture = tempfile::tempdir().unwrap();
    let marker = fixture.path().join("descendant.pid");
    let child = Command::new("sh")
        .args(["-c", "sleep 30 & echo $! > \"$1\"; exit 1", "sh"])
        .arg(&marker)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(File::create(fixture.path().join("stdout")).unwrap())
        .stderr(File::create(fixture.path().join("stderr")).unwrap())
        .spawn()
        .unwrap();
    let group = child.id();
    let started = Instant::now();
    let status = isolated_command::wait(child);
    let elapsed = started.elapsed();
    let members = process_cleanup::live_group_members(group);
    let descendant: u32 = std::fs::read_to_string(marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // End the deliberately leaked fixture before reporting its assertions.
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(group.try_into().unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    process_cleanup::assert_group_exited(group);
    assert_eq!(status.code(), Some(1));
    assert!(
        elapsed < Duration::from_secs(5),
        "leader wait inherited a descendant's output lifetime"
    );
    assert!(
        members.contains(&descendant),
        "the orphan remains visible to the cleanup inventory"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_serve_open_and_token_options_stay_gateway_local() {
    let server = TestServer::start_options(&["--open", "--oneshot-token"]);
    let opened = server._home.path().join("opened");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !opened.exists() {
        assert!(
            Instant::now() < deadline,
            "--open did not invoke the browser launcher"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        std::fs::read_to_string(&opened).unwrap().trim(),
        format!(
            "http://127.0.0.1:{}/?bootstrap={}&desktop=1",
            server.port, server.bootstrap
        )
    );
    let browser = server.authenticated_client().await;
    assert_eq!(
        browser
            .get(server.url("/api/mux/status"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(
        !server
            ._home
            .path()
            .join("config/splitty/auth-token")
            .exists(),
        "oneshot must not resolve stored credentials"
    );

    let explicit = TestServer::start_options(&["--token", "explicit-owner-token"]);
    assert_eq!(
        TestServer::client()
            .get(explicit.url("/api/mux/status"))
            .bearer_auth("explicit-owner-token")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        TestServer::client()
            .get(explicit.url("/api/mux/status"))
            .bearer_auth("other-token")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(
        !explicit._home.path().join("opened").exists(),
        "serve without --open must not open a browser"
    );
    assert!(!explicit
        ._home
        .path()
        .join("config/splitty/auth-token")
        .exists());
}

// ── cli: comments and reviews ───────────────────────────────────────────────

type Window =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A window of `server`: a WebSocket with a ticket and the page's origin,
/// past its welcome.
async fn window(server: &TestServer) -> Window {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let ticket = server.ws_ticket().await;
    let mut request = format!("ws://127.0.0.1:{}/ws?ticket={ticket}", server.port)
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert("Origin", server.own_origin().parse().expect("origin"));
    let (mut window, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("WebSocket");
    message_of(&mut window, "mux.welcome").await;
    window
}

/// The next message of `window` of type `kind`, past others.
async fn message_of(window: &mut Window, kind: &str) -> serde_json::Value {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Message::Text(text) = window.next().await.expect("open").expect("message") {
                let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
                if value["type"] == kind {
                    return value;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} in time"))
}

async fn send(window: &mut Window, value: serde_json::Value) {
    use futures::SinkExt;
    window
        .send(tokio_tungstenite::tungstenite::Message::Text(
            value.to_string().into(),
        ))
        .await
        .expect("send");
}

/// Screen `s-cli` as `window` sees it after everything it sent before.
async fn screen(window: &mut Window) -> serde_json::Value {
    send(
        window,
        serde_json::json!({"type": "mux.resync", "screenId": "s-cli"}),
    )
    .await;
    message_of(window, "mux.screen_snapshot").await["screen"].clone()
}

/// The text viewer titled `Plan`, the wait for its review and its record,
/// once a `splitty review` waits for it.
async fn waiting_review(window: &mut Window) -> (String, String, serde_json::Value) {
    loop {
        let screen = screen(window).await;
        let waiting = screen["panes"]
            .as_object()
            .into_iter()
            .flatten()
            .find_map(|(id, pane)| {
                let viewer = &pane["viewer"];
                let wait = viewer["reviewWait"]["id"].as_str()?;
                (viewer["source"]["title"] == "Plan")
                    .then(|| (id.clone(), wait.to_owned(), viewer.clone()))
            });
        if let Some(waiting) = waiting {
            return waiting;
        }
        message_of(window, "mux.nudge").await;
    }
}

/// `splitty` run in `dir` from the pane `p-agent` of screen `s-cli`, which
/// finds the server through `SPLITTY_SOCK` only.
fn cli(server: &TestServer, dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_splitty"));
    command
        .current_dir(dir)
        .env_clear()
        .env("HOME", server.home_path())
        .env("SPLITTY_SOCK", control_socket_of(server))
        .env("SPLITTY_SCREEN_ID", "s-cli")
        .env("SPLITTY_PANE_ID", "p-agent")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// `command` with `args`, run to its end off the runtime's thread.
async fn run(mut command: Command, args: &[&str]) -> std::process::Output {
    command.args(args);
    tokio::task::spawn_blocking(move || command.output().expect("splitty"))
        .await
        .expect("run")
}

/// `splitty review <args>` with `text` on standard input, left waiting.
fn review(server: &TestServer, dir: &Path, args: &[&str], text: &str) -> Child {
    let mut child = cli(server, dir)
        .arg("review")
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .expect("splitty review");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("text");
    child
}

async fn finished(child: Child) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || child.wait_with_output().expect("splitty review")),
    )
    .await
    .expect("the command ends in time")
    .expect("wait")
}

/// cli, Comments Command (Nothing sent yet, Read a sent review, Reply, Two
/// viewers of one file), Review Command (A plan reviewed, The next revision
/// of the plan, Approved, Outcome as JSON, The pane closed) and Review Hook
/// For Claude Code (Changes requested, Outside Splitty): the
/// real `splitty` in a pane's environment against a real server, answered by
/// a window over the WebSocket. Smoke: only here are the CLI's discovery of
/// the server, its output and its exit status visible; the endpoints are
/// tested in process (`http::tests::annotations`, `http::tests::reviews`).
#[cfg(unix)]
#[tokio::test]
async fn an_agent_reads_and_answers_a_review_through_the_cli() {
    use serde_json::json;
    let server = TestServer::start();
    let created = TestServer::client()
        .post(server.url("/api/mux/op"))
        .bearer_auth(&server.token)
        .json(&json!({
            "screenId": "s-cli",
            "op": {"type": "create_screen", "screenId": "s-cli", "newPaneId": "p-agent"},
        }))
        .send()
        .await
        .expect("create_screen");
    assert_eq!(created.status(), reqwest::StatusCode::OK);
    let dir = server.home_path().join("app");
    std::fs::create_dir(&dir).expect("app");
    let dir = dir.canonicalize().expect("canonical");
    std::fs::write(
        dir.join("PLAN.md"),
        "# Plan\n\nOn failure, retry three times.\n\nThen cache the list.\n",
    )
    .expect("plan");
    let plan = dir.join("PLAN.md").to_str().expect("UTF-8").to_owned();
    let mut window = window(&server).await;

    let opened = run(cli(&server, &dir), &["open", "PLAN.md"]).await;
    assert!(opened.status.success(), "{opened:?}");
    let stdout = String::from_utf8(opened.stdout).expect("UTF-8");
    let viewer = stdout.trim().rsplit(' ').next().expect("pane").to_owned();
    assert!(viewer.starts_with("viewer-"), "{stdout}");

    let listed = run(cli(&server, &dir), &["comments", "PLAN.md"]).await;
    assert!(listed.status.success(), "{listed:?}");
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        format!("No comments on {plan}.\n"),
        "nothing sent yet"
    );

    let handed_over = json!([
        {"id": "ann-0000000000000001", "kind": "comment", "body": "Why not five times?",
         "target": {"view": "source", "quote": "retry three times", "prefix": "On failure, ",
                    "suffix": ".", "startLine": 3, "endLine": 3}},
        {"id": "ann-0000000000000002", "kind": "change", "replacement": "compute it on demand",
         "target": {"view": "source", "quote": "cache the list", "prefix": "Then ",
                    "suffix": ".", "startLine": 5, "endLine": 5}},
    ]);
    send(
        &mut window,
        json!({"type": "mux.op", "screenId": "s-cli", "op": {
            "type": "put_annotations", "paneId": viewer, "annotations": handed_over,
        }}),
    )
    .await;
    // The review held both as they were put.
    send(
        &mut window,
        json!({"type": "mux.op", "screenId": "s-cli", "op": {
            "type": "mark_sent", "paneId": viewer, "entries": handed_over,
        }}),
    )
    .await;
    let annotations = screen(&mut window).await["panes"][&viewer]["viewer"]["annotations"].clone();
    assert_eq!(annotations[1]["sent"], true, "{annotations}");

    let listed = run(cli(&server, &dir), &["comments", "PLAN.md"]).await;
    assert!(listed.status.success(), "{listed:?}");
    let listing = String::from_utf8(listed.stdout).expect("UTF-8");
    for part in [
        "# Review of PLAN.md".to_owned(),
        format!("Document: {plan}\nViewer pane: {viewer}"),
        "## #1 Line 3, comment\n> retry three times\n\nWhy not five times?".to_owned(),
        "## #2 Line 5, suggested change\n> cache the list\n\nReplace with:\n```\ncompute it on demand\n```"
            .to_owned(),
    ] {
        assert!(listing.contains(&part), "{part}\n---\n{listing}");
    }

    let replied = run(
        cli(&server, &dir),
        &[
            "comments",
            "reply",
            "PLAN.md",
            "2",
            "Switched to on-demand computation.",
        ],
    )
    .await;
    assert!(replied.status.success(), "{replied:?}");
    assert_eq!(
        String::from_utf8_lossy(&replied.stdout),
        format!("Replied to #2 in {plan}\n")
    );
    let thread =
        screen(&mut window).await["panes"][&viewer]["viewer"]["annotations"][1]["replies"].clone();
    assert_eq!(thread[0]["author"], "agent", "{thread}");
    assert_eq!(thread[0]["body"], "Switched to on-demand computation.");
    assert_eq!(thread[0]["from"]["paneId"], "p-agent", "{thread}");
    let listed = run(cli(&server, &dir), &["comments", &viewer, "--json"]).await;
    assert!(listed.status.success(), "{listed:?}");
    let answer: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("JSON");
    assert_eq!(
        answer["documents"][0]["annotations"][1]["replies"][0]["body"],
        "Switched to on-demand computation.",
        "{answer}"
    );

    send(
        &mut window,
        json!({"type": "mux.op", "screenId": "s-cli", "op": {
            "type": "open_viewer", "paneId": "p-agent", "newPaneId": "viewer-00000000000000aa",
            "placement": "tab", "source": {"kind": "file", "path": plan},
        }}),
    )
    .await;
    screen(&mut window).await;
    let refused = run(
        cli(&server, &dir),
        &["comments", "reply", "PLAN.md", "1", "Which one?"],
    )
    .await;
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(
        error.contains(&viewer) && error.contains("viewer-00000000000000aa"),
        "two viewers of one file: {error}"
    );

    let first = "1. Retry three times.\n2. Cache the list.\n";
    let changes = "# Review of Plan\n\nVerdict: changes requested\n\n## #1 Line 1, comment\n> Retry three times.\n\nWhy not five times?\n";
    let child = review(&server, &dir, &["--text", "--title", "Plan"], first);
    let (plan_pane, wait, record) = waiting_review(&mut window).await;
    assert_eq!(record["source"]["kind"], "text", "{record}");
    let mut revision = record["revision"].as_u64().expect("revision");
    send(
        &mut window,
        json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": plan_pane,
               "waitId": wait, "outcome": "changes_requested", "review": changes}),
    )
    .await;
    assert_eq!(
        message_of(&mut window, "viewer.review_answered").await["accepted"],
        true
    );
    let output = finished(child).await;
    // Only the binary's exit path shows the status of a request for changes.
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), changes);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&plan_pane),
        "{output:?}"
    );

    let revised = "1. Retry five times with backoff.\n2. Cache the list.\n";
    for (json, printed) in [(false, ""), (true, "{\"outcome\":\"approved\"}\n")] {
        let args: &[&str] = if json {
            &["--text", "--title", "Plan", "--json"]
        } else {
            &["--text", "--title", "Plan"]
        };
        let child = review(&server, &dir, args, revised);
        let (again, wait, record) = waiting_review(&mut window).await;
        assert_eq!(again, plan_pane, "the next revision goes to the same pane");
        let shown = record["revision"].as_u64().expect("revision");
        assert!(shown > revision, "the pane shows the new text: {record}");
        revision = shown;
        send(
            &mut window,
            json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": again,
                   "waitId": wait, "outcome": "approved"}),
        )
        .await;
        assert_eq!(
            message_of(&mut window, "viewer.review_answered").await["accepted"],
            true
        );
        let output = finished(child).await;
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), printed);
    }

    // Claude Code's hook: the plan of its event goes to the same pane, and a
    // request for changes is a denial with the review, with status 0.
    let event = json!({
        "hook_event_name": "PermissionRequest", "tool_name": "ExitPlanMode",
        "cwd": dir, "tool_input": {"plan": revised},
    })
    .to_string();
    let child = review(&server, &dir, &["--hook", "claude-code"], &event);
    let (again, wait, record) = waiting_review(&mut window).await;
    assert_eq!(again, plan_pane, "{record}");
    send(
        &mut window,
        json!({"type": "viewer.review_answer", "screenId": "s-cli", "paneId": again,
               "waitId": wait, "outcome": "changes_requested", "review": changes}),
    )
    .await;
    assert_eq!(
        message_of(&mut window, "viewer.review_answered").await["accepted"],
        true
    );
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let decision: serde_json::Value = serde_json::from_slice(&output.stdout).expect("decision");
    assert_eq!(
        decision,
        json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
               "decision": {"behavior": "deny", "message": changes}}})
    );
    // Outside a Splitty terminal the hook decides nothing, and only the
    // binary's exit path shows that it still exits 0.
    let mut outside = cli(&server, &dir);
    outside.env_remove("SPLITTY_PANE_ID");
    let mut child = outside
        .args(["review", "--hook", "claude-code"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("splitty review");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(event.as_bytes())
        .expect("event");
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Claude Code asks the user itself"),
        "{output:?}"
    );

    let child = review(&server, &dir, &["--text", "--title", "Plan"], revised);
    let (pane, _, _) = waiting_review(&mut window).await;
    send(
        &mut window,
        json!({"type": "mux.op", "screenId": "s-cli", "op": {"type": "close_pane", "paneId": pane}}),
    )
    .await;
    let output = finished(child).await;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("closed without an answer"),
        "{output:?}"
    );
}
