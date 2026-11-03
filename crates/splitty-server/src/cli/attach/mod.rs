//! `splitty attach`: one terminal session of a server shown in this terminal,
//! as one more client of the session protocol next to the windows that show
//! it. The server keeps owning the session; detaching leaves it running.
//!
//! The server is the daemon of this machine, reached like `remote-bridge`
//! reaches it (verified private Unix socket), or an explicit browser gateway
//! named by `--addr` and `--token`. `SPLITTY_SOCK` is not used, so the pane list and the stream
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
    if options.addr.is_none() && options.token.is_none() {
        let Some(info) = native_daemon()? else {
            return run_on_keeper(options).await;
        };
        let connector = crate::native::client::Connector::default();
        let mut api = connector.local(&info).await.map_err(native_error)?;
        let reply = api
            .request(
                crate::service::catalog::Operation::MuxState,
                serde_json::Map::new(),
            )
            .await
            .map_err(native_error)?;
        if reply.status != 200 {
            return Err(CliError::Failed(reply.body.chars().take(4096).collect()));
        }
        let decoded = splitty_transport::codec::reserve_json(
            reply.body.as_bytes(),
            &api.sender().context().budget,
            false,
        )
        .map_err(native_error)?;
        let state = serde_json::from_str(&reply.body).map_err(native_error)?;
        api.shutdown().await;
        let panes = target::panes(&state);
        let session_id = session_for(options.target.as_deref(), &panes, pick)?;
        drop((panes, state, reply, decoded));
        refuse_own_session(&session_id, std::env::var("SPLITTY_SESSION_ID").ok())?;
        let client = connector.local(&info).await.map_err(native_error)?;
        return show_native(client, session_id, options.detach_key).await;
    }
    let (addr, token) = explicit_server(options.addr.as_deref(), options.token.as_deref())?
        .expect("explicit options were provided");
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
            Some(info) => match crate::native::client::Connector::default()
                .local(&info)
                .await
            {
                Ok(client) => {
                    run_native(
                        client,
                        session::Attach {
                            session_id,
                            detach_key: options.detach_key,
                            size: terminal::size(&std::io::stdout()).unwrap_or(size),
                            skip_replay: true,
                        },
                        &mut events,
                        &mut screen,
                    )
                    .await
                }
                Err(error) => Err(native_error(error)),
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
async fn daemon_after_takeover() -> Option<splitty_core::daemon::DaemonInfo> {
    for _ in 0..50 {
        if let Ok(Some(info)) = native_daemon() {
            return Some(info);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    None
}

#[cfg(unix)]
fn native_error(error: impl std::fmt::Display) -> CliError {
    CliError::Failed(error.to_string())
}

#[cfg(unix)]
fn native_daemon() -> Result<Option<splitty_core::daemon::DaemonInfo>, CliError> {
    let info = splitty_core::daemon::read_info_checked()
        .map_err(native_error)?
        .filter(|info| splitty_core::daemon::is_running(info.pid));
    if info.as_ref().is_some_and(|info| info.legacy()) {
        return Err(CliError::Failed(
            "legacy daemon has no native endpoint; explicitly restart/update it before attaching"
                .into(),
        ));
    }
    Ok(info)
}

#[cfg(unix)]
async fn show_native(
    client: crate::native::client::Client,
    session_id: String,
    detach_key: DetachKey,
) -> Result<Outcome, CliError> {
    let size = terminal::size(&std::io::stdout()).unwrap_or((80, 24));
    let alternate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raw = terminal::RawMode::enable(std::io::stdin(), std::io::stdout(), alternate.clone())
        .map_err(native_error)?;
    let mut events = terminal::events().map_err(native_error)?;
    let result = run_native(
        client,
        session::Attach {
            session_id,
            detach_key,
            size,
            skip_replay: false,
        },
        &mut events,
        &mut terminal::Screen::new(std::io::stdout(), alternate),
    )
    .await;
    drop(raw);
    result
}

#[cfg(unix)]
async fn run_native(
    client: crate::native::client::Client,
    attach: session::Attach,
    events: &mut tokio::sync::mpsc::Receiver<session::Event>,
    screen: &mut impl std::io::Write,
) -> Result<Outcome, CliError> {
    let sink = crate::native::client::SessionSink::new(client.sender());
    let source = Box::pin(futures::stream::unfold(client, |mut client| async move {
        client
            .recv()
            .await
            .map(|message| (Ok::<_, std::io::Error>(message), client))
    }));
    session::run(attach, sink, source, events, screen).await
}

/// Explicit browser gateway attachment is the only token/WebSocket path.
fn explicit_server(
    addr: Option<&str>,
    token: Option<&str>,
) -> Result<Option<(String, String)>, CliError> {
    match (addr, token) {
        (Some(addr), Some(token)) if !addr.is_empty() && !token.is_empty() => {
            Ok(Some((addr.into(), token.into())))
        }
        (None, None) => Ok(None),
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
    fn tcp_attachment_is_explicit_and_cannot_resolve_a_daemon_or_token_file() {
        assert_eq!(
            explicit_server(Some("127.0.0.1:9900"), Some("t")).unwrap(),
            Some(("127.0.0.1:9900".into(), "t".into()))
        );
        for (addr, token) in [
            (Some("127.0.0.1:1"), None),
            (None, Some("t")),
            (Some(""), Some("t")),
        ] {
            assert!(matches!(
                explicit_server(addr, token),
                Err(CliError::Usage(_))
            ));
        }
        assert_eq!(explicit_server(None, None).unwrap(), None);
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
