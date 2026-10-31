//! `splitty attach` without a daemon: the terminal controls a session keeper
//! directly. When a daemon starts and takes the keeper over, the attach
//! continues through the daemon without showing the output again.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use splitty_core::keeper::client::Found;
use splitty_core::keeper::wire::{self, Frame};
use splitty_core::pty::replay::ReplayFilter;
use tokio::sync::mpsc;

use super::input::{DetachKey, Utf8Input};
use super::session::Event;
use super::target::Pane;
use super::Outcome;
use crate::cli::CliError;

/// The panes of the running keepers, for the list and the targets: a keeper
/// names its pane and session, its program and, looked up by its PID, its
/// working directory.
pub fn panes(found: &[Found]) -> Vec<Pane> {
    found
        .iter()
        .map(|keeper| {
            let hello = &keeper.hello;
            let cwd = hello
                .pid
                .filter(|_| hello.exited.is_none())
                .and_then(splitty_core::pty::procinfo::process_info)
                .map(|info| info.cwd)
                .filter(|cwd| !cwd.is_empty())
                .unwrap_or_else(|| "-".into());
            Pane {
                id: hello
                    .pane_id
                    .clone()
                    .unwrap_or_else(|| hello.session_id.clone()),
                session_id: hello.session_id.clone(),
                screen: "-".into(),
                title: if hello.program.is_empty() {
                    "-".into()
                } else {
                    hello.program.clone()
                },
                cwd,
            }
        })
        .collect()
}

/// How an attach to a keeper ended: the keeper's own outcome, or a daemon
/// took the keeper over.
pub enum Ended {
    Outcome(Outcome),
    TakenOver,
}

/// A keeper this terminal controls, before it shows the session.
pub struct Claimed {
    stream: UnixStream,
    replay_end: u64,
    size: (u16, u16),
}

/// Claim `found`'s keeper without taking it over. Done before the terminal
/// goes raw, so a refusal reads like any other error.
pub fn claim(found: Found) -> Result<Claimed, CliError> {
    let replay_end = found.hello.buffer_end;
    let size = (found.hello.cols, found.hello.rows);
    let mut stream = found.into_stream();
    match Found::claim(&mut stream, false, None) {
        Ok(_) => Ok(Claimed {
            stream,
            replay_end,
            size,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::ResourceBusy => Err(CliError::Failed(
            "another client controls the session".into(),
        )),
        Err(error) => Err(CliError::Failed(error.to_string())),
    }
}

/// Show the claimed keeper's session until the user detaches, the program
/// ends, or another controller takes over.
pub async fn run(
    claimed: Claimed,
    detach_key: DetachKey,
    size: (u16, u16),
    events: &mut mpsc::Receiver<Event>,
    screen: &mut impl Write,
) -> Result<Ended, CliError> {
    let Claimed {
        stream,
        replay_end,
        size: keeper_size,
    } = claimed;
    let mut writer = stream
        .try_clone()
        .map_err(|e| CliError::Failed(e.to_string()))?;

    // The keeper's frames on a thread of their own: its socket is blocking.
    let (frames_tx, mut frames) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut stream: UnixStream = stream;
        while let Ok(Some(frame)) = wire::read_frame(&mut stream) {
            if frames_tx.send(frame).is_err() {
                return;
            }
        }
    });

    // The terminal's size, with a redraw when it is the keeper's already.
    let send = |writer: &mut UnixStream, frame: &Frame| {
        wire::write_frame(writer, frame).map_err(|_| lost())
    };
    if keeper_size == size {
        send(
            &mut writer,
            &Frame::Resize {
                cols: size.0,
                rows: size.1.saturating_add(1),
            },
        )?;
    }
    send(
        &mut writer,
        &Frame::Resize {
            cols: size.0,
            rows: size.1,
        },
    )?;

    let mut filter = Some(ReplayFilter::new());
    let mut text = Utf8Input::default();
    let mut seq = 0u32;
    let mut exited: Option<Outcome> = None;
    let mut output_ended = false;
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Some(Frame::Output { offset, data }) => {
                    let replayed = usize::try_from(replay_end.saturating_sub(offset))
                        .unwrap_or(usize::MAX)
                        .min(data.len());
                    if let Some(active) = filter.as_mut() {
                        show(screen, &active.push(&data[..replayed]))?;
                        let end = offset.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
                        if end >= replay_end {
                            show(screen, &active.finish())?;
                            filter = None;
                        }
                    }
                    show(screen, &data[replayed..])?;
                }
                Some(Frame::Exited(exit)) => {
                    exited = Some(match (exit.signal, exit.exit_code) {
                        (Some(signal), _) => Outcome::Signaled(signal),
                        (None, Some(code)) => Outcome::Exited(i64::from(code)),
                        (None, None) => Outcome::Closed,
                    });
                }
                Some(Frame::OutputEnded) => output_ended = true,
                Some(Frame::TakenOver) => return Ok(Ended::TakenOver),
                Some(_) => {}
                None => return Err(CliError::Failed("lost the connection to the session's keeper".into())),
            },
            event = events.recv() => match event {
                Some(Event::Input(bytes)) => {
                    let (typed, detached) = match detach_key.find(&bytes) {
                        Some(at) => (&bytes[..at], true),
                        None => (&bytes[..], false),
                    };
                    let data = text.push(typed);
                    if !data.is_empty() {
                        seq = seq.wrapping_add(1);
                        send(&mut writer, &Frame::Input { seq, data: data.into_bytes() })?;
                    }
                    if detached {
                        let _ = send(&mut writer, &Frame::Release);
                        return Ok(Ended::Outcome(Outcome::Detached));
                    }
                }
                Some(Event::Resized(cols, rows)) => {
                    send(&mut writer, &Frame::Resize { cols, rows })?;
                }
                Some(Event::Terminated) => {
                    let _ = send(&mut writer, &Frame::Release);
                    return Ok(Ended::Outcome(Outcome::Detached));
                }
                Some(Event::HungUp) | None => {
                    let _ = send(&mut writer, &Frame::Release);
                    return Ok(Ended::Outcome(Outcome::HungUp));
                }
            },
            // The program has ended; its last output follows soon, if at all.
            () = tokio::time::sleep(Duration::from_millis(500)), if exited.is_some() && !output_ended => {
                output_ended = true;
            }
        }
        if output_ended {
            if let Some(outcome) = exited.take() {
                // The result stays with the keeper for the next daemon.
                let _ = send(&mut writer, &Frame::Release);
                return Ok(Ended::Outcome(outcome));
            }
        }
    }
}

fn lost() -> CliError {
    CliError::Failed("lost the connection to the session's keeper".into())
}

fn show(screen: &mut impl Write, bytes: &[u8]) -> Result<(), CliError> {
    if bytes.is_empty() {
        return Ok(());
    }
    screen
        .write_all(bytes)
        .and_then(|()| screen.flush())
        .map_err(|e| CliError::Failed(format!("cannot write to the terminal: {e}")))
}
