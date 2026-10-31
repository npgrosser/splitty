//! `splitty attach`: one terminal session of a server shown in this terminal,
//! as one more client of the session protocol next to the windows that show
//! it. The server keeps owning the session; detaching leaves it running.
//!
//! The server is the daemon of this machine, reached like `remote-bridge`
//! reaches it (token file, ticket, WebSocket), or one named by `--addr` and
//! `--token`. `SPLITTY_SOCK` is not used, so the pane list and the stream
//! always come from the same server. Without either, the terminal controls a
//! session keeper of this machine directly, until a starting daemon takes it
//! over and the attach continues through the daemon.

mod input;
#[cfg(unix)]
mod keeper;
mod session;
mod target;
#[cfg(unix)]
mod terminal;

pub use input::DetachKey;

use super::CliError;

/// What `splitty attach` was asked for.
pub struct Options {
    pub target: Option<String>,
    pub detach_key: DetachKey,
    pub addr: Option<String>,
    pub token: Option<String>,
}

/// How an attach ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Detached,
    Exited(i64),
    Signaled(String),
    /// Closed without an exit code, for example by a window.
    Closed,
    /// The terminal went away.
    HungUp,
    /// Another client took the session's keeper over, and no daemon did.
    TakenOver,
}

impl Outcome {
    /// The line printed once the terminal is restored; none after a hangup,
    /// with nobody left to read it.
    pub fn note(&self) -> Option<String> {
        match self {
            Self::Detached => Some("[detached]".into()),
            Self::Exited(code) => Some(format!("[exited with code {code}]")),
            Self::Signaled(signal) => Some(format!("[ended by signal {signal}]")),
            Self::Closed => Some("[session closed]".into()),
            Self::HungUp => None,
            Self::TakenOver => Some("[taken over by another client]".into()),
        }
    }
}

const NO_DAEMON: &str =
    "no running Splitty daemon found (splitty attach uses daemon.json, or --addr with --token)";

/// Attach until the session or the user ends it.
pub async fn run(options: Options) -> Result<Outcome, CliError> {
    #[cfg(unix)]
    {
        run_in_terminal(options).await
    }
    #[cfg(not(unix))]
    {
        let _ = options;
        Err(unavailable())
    }
}

#[cfg(any(not(unix), test))]
fn unavailable() -> CliError {
    CliError::Failed("splitty attach is not available on this platform".into())
}

#[cfg(unix)]
async fn run_in_terminal(options: Options) -> Result<Outcome, CliError> {
    use futures::StreamExt;

    if !terminal::is_terminal() {
        return Err(CliError::Failed("splitty attach needs a terminal".into()));
    }
    let Some((addr, token)) = server(
        options.addr.as_deref(),
        options.token.as_deref(),
        daemon_port,
        token_file,
    )?
    else {
        return run_on_keeper(options).await;
    };
    let api = super::ServerInfo {
        transport: super::Transport::Tcp {
            addr: addr.clone(),
            token: token.clone(),
        },
        source: "attach",
    };
    let state = super::api_get(&api, "/api/mux/state").await?;
    let panes = target::panes(&state);
    let session_id = session_for(options.target.as_deref(), &panes, pick)?;
    refuse_own_session(&session_id, std::env::var("SPLITTY_SESSION_ID").ok())?;

    let socket = super::socket::open(&addr, &token)
        .await
        .map_err(CliError::Failed)?;
    let size = terminal::size(&std::io::stdout()).unwrap_or((80, 24));
    let alternate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raw = terminal::RawMode::enable(std::io::stdin(), std::io::stdout(), alternate.clone())
        .map_err(|e| CliError::Failed(format!("cannot put the terminal into raw mode: {e}")))?;
    // Read only once the terminal is raw, so no read waits for a whole line.
    let mut events = terminal::events().map_err(|e| CliError::Failed(e.to_string()))?;
    let (to_server, from_server) = socket.split();
    let attach = session::Attach {
        session_id,
        detach_key: options.detach_key,
        size,
        skip_replay: false,
    };
    let outcome = session::run(
        attach,
        to_server,
        from_server,
        &mut events,
        &mut terminal::Screen::new(std::io::stdout(), alternate),
    )
    .await;
    drop(raw);
    outcome
}

/// Without a daemon: a keeper of this machine, controlled directly.
#[cfg(unix)]
async fn run_on_keeper(options: Options) -> Result<Outcome, CliError> {
    use futures::StreamExt;

    let found = splitty_core::keeper::client::find(&splitty_core::keeper::dir::keepers_dir());
    if found.is_empty() {
        return Err(CliError::Failed(NO_DAEMON.into()));
    }
    let panes = keeper::panes(&found);
    let session_id = session_for(options.target.as_deref(), &panes, pick)?;
    refuse_own_session(&session_id, std::env::var("SPLITTY_SESSION_ID").ok())?;
    let found = found
        .into_iter()
        .find(|keeper| keeper.hello.session_id == session_id)
        .ok_or_else(|| CliError::Failed(format!("no kept session is `{session_id}`")))?;
    let claimed = keeper::claim(found)?;

    let size = terminal::size(&std::io::stdout()).unwrap_or((80, 24));
    let alternate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raw = terminal::RawMode::enable(std::io::stdin(), std::io::stdout(), alternate.clone())
        .map_err(|e| CliError::Failed(format!("cannot put the terminal into raw mode: {e}")))?;
    let mut events = terminal::events().map_err(|e| CliError::Failed(e.to_string()))?;
    let mut screen = terminal::Screen::new(std::io::stdout(), alternate);
    let ended = keeper::run(claimed, options.detach_key, size, &mut events, &mut screen).await;
    let outcome = match ended {
        Ok(keeper::Ended::Outcome(outcome)) => Ok(outcome),
        Ok(keeper::Ended::TakenOver) => match daemon_after_takeover().await {
            // A starting daemon took the keeper: carry on through it, without
            // showing what the terminal shows already.
            Some((addr, token)) => match super::socket::open(&addr, &token).await {
                Ok(socket) => {
                    let (to_server, from_server) = socket.split();
                    let attach = session::Attach {
                        session_id,
                        detach_key: options.detach_key,
                        size: terminal::size(&std::io::stdout()).unwrap_or(size),
                        skip_replay: true,
                    };
                    session::run(attach, to_server, from_server, &mut events, &mut screen).await
                }
                Err(error) => Err(CliError::Failed(error)),
            },
            None => Ok(Outcome::TakenOver),
        },
        Err(error) => Err(error),
    };
    drop(raw);
    outcome
}

/// The daemon that took a keeper over, once it serves: a daemon claims its
/// keepers before it writes `daemon.json`.
#[cfg(unix)]
async fn daemon_after_takeover() -> Option<(String, String)> {
    for _ in 0..50 {
        if let Some(port) = daemon_port() {
            return token_file()
                .ok()
                .map(|token| (format!("127.0.0.1:{port}"), token));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    None
}

#[cfg(unix)]
fn token_file() -> Result<String, String> {
    splitty_core::auth::resolve("")
        .map(|(token, _)| token)
        .map_err(|e| e.to_string())
}

/// The daemon's port, when `daemon.json` names a live daemon.
#[cfg(unix)]
fn daemon_port() -> Option<u16> {
    splitty_core::daemon::read_info()
        .filter(|info| splitty_core::daemon::is_running(info.pid))
        .map(|info| info.port)
}

/// The server to attach to and its token: `--addr` with `--token`, or the
/// daemon of this machine with the token file, which is read only then. None
/// without either: the keepers of this machine are next.
fn server(
    addr: Option<&str>,
    token: Option<&str>,
    daemon: impl FnOnce() -> Option<u16>,
    token_file: impl FnOnce() -> Result<String, String>,
) -> Result<Option<(String, String)>, CliError> {
    match (addr, token) {
        (Some(addr), Some(token)) if !addr.is_empty() && !token.is_empty() => {
            Ok(Some((addr.to_owned(), token.to_owned())))
        }
        (None, None) => {
            let Some(port) = daemon() else {
                return Ok(None);
            };
            let token = token_file().map_err(CliError::Failed)?;
            Ok(Some((format!("127.0.0.1:{port}"), token)))
        }
        _ => Err(CliError::Usage(
            "--addr and --token must both be non-empty".into(),
        )),
    }
}

/// The session a target names, or without one the session of the pane the
/// user picks. Never the pane the command runs in by default
/// (`SPLITTY_PANE_ID`): attaching to it would show the session inside itself.
fn session_for(
    target: Option<&str>,
    panes: &[target::Pane],
    pick: impl FnOnce(&[target::Pane]) -> Result<String, CliError>,
) -> Result<String, CliError> {
    match target {
        Some(target) => target::resolve(target, panes),
        None => pick(panes),
    }
}

/// A session's output shown inside itself would feed itself.
fn refuse_own_session(session_id: &str, own: Option<String>) -> Result<(), CliError> {
    if own.as_deref() == Some(session_id) {
        return Err(CliError::Failed(
            "cannot attach to the session this command runs in".into(),
        ));
    }
    Ok(())
}

/// The session of the pane the user picks from the numbered list.
#[cfg(unix)]
fn pick(panes: &[target::Pane]) -> Result<String, CliError> {
    use std::io::{BufRead, Write};

    if panes.is_empty() {
        return Err(CliError::Failed("the server has no terminal panes".into()));
    }
    let mut stdout = std::io::stdout();
    let _ = write!(
        stdout,
        "{}Attach to (1-{}): ",
        target::list(panes),
        panes.len()
    );
    let _ = stdout.flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| CliError::Failed(e.to_string()))?;
    target::pick(&answer, panes)
        .map(|pane| pane.session_id.clone())
        .ok_or_else(|| CliError::Failed("no pane picked".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_is_the_explicit_one_or_the_daemon_never_splitty_sock() {
        let no_daemon = || panic!("must not look for the daemon");
        let no_token_file = || -> Result<String, String> { panic!("must not read the token file") };
        assert_eq!(
            server(Some("127.0.0.1:9900"), Some("t"), no_daemon, no_token_file).expect("explicit"),
            Some(("127.0.0.1:9900".to_owned(), "t".to_owned()))
        );
        for (addr, token) in [
            (Some("127.0.0.1:1"), None),
            (None, Some("t")),
            (Some(""), Some("t")),
        ] {
            assert!(matches!(
                server(addr, token, no_daemon, no_token_file),
                Err(CliError::Usage(_))
            ));
        }
        assert_eq!(
            server(None, None, || Some(9800), || Ok("file".into())).expect("daemon"),
            Some(("127.0.0.1:9800".to_owned(), "file".to_owned()))
        );
        // Inside a pane of an embedded server only `SPLITTY_SOCK` would lead
        // anywhere, and it is not consulted: without a daemon, the keepers
        // are next, and without those the command fails (`run_on_keeper`).
        assert_eq!(
            server(None, None, || None, no_token_file).expect("no daemon"),
            None
        );
    }

    #[test]
    fn without_a_target_the_user_picks_even_inside_a_pane() {
        let pane = target::Pane {
            id: "3f9a1111-0000-4000-8000-000000000001".into(),
            session_id: "sess-a".into(),
            screen: "dev".into(),
            title: "-".into(),
            cwd: "-".into(),
        };
        let panes = [pane];
        let picked = session_for(None, &panes, |offered| {
            assert_eq!(offered.len(), 1, "the list offers every pane");
            Ok("sess-picked".into())
        });
        assert_eq!(picked.expect("picked"), "sess-picked");
        let named = session_for(Some("3f9a"), &panes, |_| panic!("a target needs no list"));
        assert_eq!(named.expect("named"), "sess-a");
    }

    #[test]
    fn the_own_session_is_refused() {
        assert!(refuse_own_session("s1", Some("s1".into())).is_err());
        assert!(refuse_own_session("s1", Some("s2".into())).is_ok());
        assert!(refuse_own_session("s1", None).is_ok());
    }

    #[test]
    fn every_ending_but_a_hangup_has_a_closing_line() {
        assert_eq!(Outcome::Detached.note().as_deref(), Some("[detached]"));
        assert_eq!(
            Outcome::Exited(3).note().as_deref(),
            Some("[exited with code 3]")
        );
        assert_eq!(
            Outcome::Signaled("SIGTERM".into()).note().as_deref(),
            Some("[ended by signal SIGTERM]")
        );
        assert_eq!(Outcome::Closed.note().as_deref(), Some("[session closed]"));
        assert_eq!(Outcome::HungUp.note(), None);
        assert_eq!(
            Outcome::TakenOver.note().as_deref(),
            Some("[taken over by another client]")
        );
    }

    #[test]
    fn other_platforms_have_no_attach() {
        assert!(matches!(unavailable(), CliError::Failed(m) if m.contains("not available")));
    }
}
