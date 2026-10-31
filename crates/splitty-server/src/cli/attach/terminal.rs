//! The terminal `splitty attach` runs in: raw mode while attached, afterwards
//! the modes a program in the session may have left on turned off and the
//! previous settings back; its size; its input and signals as events.

use std::io::{IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nix::sys::termios::{self, SetArg, Termios};
use tokio::sync::mpsc;

use super::session::Event;

/// Leaves the alternate screen. Sent only when the session's output left the
/// terminal there: xterm also restores the saved cursor on it, which would
/// move the cursor of a terminal on its normal screen.
pub const LEAVE_ALTERNATE: &[u8] = b"\x1b[?1049l";

/// Turns off what a program may have turned on: a hidden cursor, mouse
/// reporting, bracketed paste, focus reporting, application cursor keys and
/// keypad, the kitty and xterm keyboard protocols, the cursor shape and text
/// attributes. Each is harmless when it was not on.
pub const RESET: &[u8] = b"\x1b[?25h\
\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\
\x1b[?2004l\x1b[?1004l\x1b[?1l\x1b>\x1b[<u\x1b[>4;0m\x1b[0 q\x1b[0m";

/// Whether standard input and output are both a terminal.
pub fn is_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

nix::ioctl_read_bad!(window_size, nix::libc::TIOCGWINSZ, nix::libc::winsize);

/// The terminal's columns and rows, if it reports a size.
pub fn size(terminal: &impl AsFd) -> Option<(u16, u16)> {
    let mut size = nix::libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one `winsize` into the struct it is given,
    // which lives for the whole call; the descriptor is borrowed and open.
    unsafe { window_size(terminal.as_fd().as_raw_fd(), &mut size) }.ok()?;
    (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
}

/// The terminal as the session's output reaches it, remembering whether that
/// output left it on the alternate screen.
pub struct Screen<W: Write> {
    output: W,
    tracker: AlternateScreen,
    alternate: Arc<AtomicBool>,
}

impl<W: Write> Screen<W> {
    pub fn new(output: W, alternate: Arc<AtomicBool>) -> Self {
        Self {
            output,
            tracker: AlternateScreen::default(),
            alternate,
        }
    }
}

impl<W: Write> Write for Screen<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.output.write(bytes)?;
        self.tracker.update(&bytes[..written]);
        self.alternate.store(self.tracker.on, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

/// Follows the switches between the normal and the alternate screen in a
/// byte stream, also when a switch is split between two writes.
#[derive(Debug, Default)]
pub struct AlternateScreen {
    on: bool,
    tail: Vec<u8>,
}

impl AlternateScreen {
    const SWITCHES: [(&'static [u8], bool); 6] = [
        (b"\x1b[?1049h", true),
        (b"\x1b[?1049l", false),
        (b"\x1b[?1047h", true),
        (b"\x1b[?1047l", false),
        (b"\x1b[?47h", true),
        (b"\x1b[?47l", false),
    ];
    /// The longest switch; a tail shorter than it never holds a whole one.
    const LONGEST: usize = 8;

    pub fn update(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.tail);
        data.extend_from_slice(bytes);
        let mut last: Option<(usize, bool)> = None;
        for (switch, on) in Self::SWITCHES {
            if let Some(at) = data.windows(switch.len()).rposition(|w| w == switch) {
                if last.is_none_or(|(previous, _)| at > previous) {
                    last = Some((at, on));
                }
            }
        }
        if let Some((_, on)) = last {
            self.on = on;
        }
        self.tail = data[data.len().saturating_sub(Self::LONGEST - 1)..].to_vec();
    }
}

/// Raw mode on a terminal until dropped. Dropping it, on every way out, a
/// panic included, leaves the alternate screen if the session's output left
/// the terminal there, writes [`RESET`] and restores the previous settings.
pub struct RawMode<I: AsFd, O: Write> {
    input: I,
    output: O,
    saved: Termios,
    alternate: Arc<AtomicBool>,
}

impl<I: AsFd, O: Write> RawMode<I, O> {
    pub fn enable(input: I, output: O, alternate: Arc<AtomicBool>) -> std::io::Result<Self> {
        let saved = termios::tcgetattr(&input)?;
        let mut raw = saved.clone();
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(&input, SetArg::TCSADRAIN, &raw)?;
        Ok(Self {
            input,
            output,
            saved,
            alternate,
        })
    }
}

impl<I: AsFd, O: Write> Drop for RawMode<I, O> {
    fn drop(&mut self) {
        // A terminal that hung up takes none of it; there is nothing left to restore.
        if self.alternate.load(Ordering::Relaxed) {
            let _ = self.output.write_all(LEAVE_ALTERNATE);
        }
        let _ = self.output.write_all(RESET);
        let _ = self.output.flush();
        let _ = termios::tcsetattr(&self.input, SetArg::TCSADRAIN, &self.saved);
    }
}

/// The terminal's input, its size changes, a hangup and SIGTERM, as events.
pub fn events() -> std::io::Result<mpsc::Receiver<Event>> {
    use tokio::signal::unix::{signal, SignalKind};

    let (sender, receiver) = mpsc::channel(64);
    let input = sender.clone();
    // A blocking read: standard input is a terminal, which Tokio cannot poll
    // portably. The thread ends with the process.
    std::thread::Builder::new()
        .name("attach-input".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = [0u8; 4096];
            loop {
                let event = match stdin.read(&mut buffer) {
                    Ok(0) | Err(_) => Event::HungUp,
                    Ok(count) => Event::Input(buffer[..count].to_vec()),
                };
                let ended = matches!(event, Event::HungUp);
                if input.blocking_send(event).is_err() || ended {
                    return;
                }
            }
        })?;

    let mut resized = signal(SignalKind::window_change())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                Some(()) = resized.recv() => match size(&std::io::stdout()) {
                    Some((cols, rows)) => Event::Resized(cols, rows),
                    None => continue,
                },
                Some(()) = hangup.recv() => Event::HungUp,
                Some(()) = terminate.recv() => Event::Terminated,
                else => return,
            };
            if sender.send(event).await.is_err() {
                return;
            }
        }
    });
    Ok(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-process with a real PTY: raw mode leaves the terminal as it found
    /// it and turns off what a full-screen program left on.
    #[test]
    fn raw_mode_restores_the_settings_and_resets_the_modes() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let pty = nix::pty::openpty(None, None).expect("openpty");
        let before = termios::tcgetattr(&pty.slave).expect("settings");
        let slave = std::fs::File::from(pty.slave);
        let output = slave.try_clone().expect("output");
        // Restoring waits until the terminal has taken what was written, so
        // the master side is read all along, as a terminal emulator would.
        let mut master = std::fs::File::from(pty.master);
        let (sender, written) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = master.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    return;
                }
            }
        });
        let read = |expected: usize| {
            let mut bytes = Vec::new();
            while bytes.len() < expected {
                let chunk = written
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("bytes from the terminal");
                bytes.extend(chunk);
            }
            bytes
        };

        let alternate = Arc::new(AtomicBool::new(false));
        {
            let _raw = RawMode::enable(&slave, &output, alternate.clone()).expect("raw mode");
            let during = termios::tcgetattr(&slave).expect("settings");
            assert!(!during
                .local_flags
                .intersects(termios::LocalFlags::ICANON | termios::LocalFlags::ECHO));
        }
        let after = termios::tcgetattr(&slave).expect("settings");
        // The kernel sets PENDIN itself when canonical mode returns (macOS):
        // pending input is to be reprocessed, which is state, not a setting.
        let settings = |flags: termios::LocalFlags| flags.difference(termios::LocalFlags::PENDIN);
        assert_eq!(settings(after.local_flags), settings(before.local_flags));
        assert_eq!(after.input_flags, before.input_flags);
        assert_eq!(after.output_flags, before.output_flags);
        assert_eq!(after.control_flags, before.control_flags);
        assert_eq!(read(RESET.len()), RESET);

        alternate.store(true, Ordering::Relaxed);
        drop(RawMode::enable(&slave, &output, alternate).expect("raw mode"));
        let expected = [LEAVE_ALTERNATE, RESET].concat();
        assert_eq!(read(expected.len()), expected);
    }

    #[test]
    fn the_alternate_screen_is_followed_across_writes() {
        let alternate = Arc::new(AtomicBool::new(false));
        let mut screen = Screen::new(Vec::new(), alternate.clone());
        let on = b"vim\x1b[?1049h\x1b[?1h";
        for (index, byte) in on.iter().enumerate() {
            screen.write_all(std::slice::from_ref(byte)).expect("write");
            assert_eq!(
                alternate.load(Ordering::Relaxed),
                index >= 10,
                "after {index} bytes"
            );
        }
        let off = b"\x1b[?1049l$ ";
        screen.write_all(off).expect("write");
        assert!(!alternate.load(Ordering::Relaxed));
        let both = b"\x1b[?47h text \x1b[?47l\x1b[?1047h";
        screen.write_all(both).expect("write");
        assert!(alternate.load(Ordering::Relaxed), "the last switch counts");
        assert_eq!(
            screen.output,
            [&on[..], off, both].concat(),
            "output passes unchanged"
        );
    }

    #[test]
    fn the_size_of_a_terminal_and_of_no_terminal() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let size = nix::pty::Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = nix::pty::openpty(Some(&size), None).expect("openpty");
        assert_eq!(super::size(&pty.slave), Some((100, 30)));
        let file = tempfile::tempfile().expect("file");
        assert_eq!(super::size(&file), None);
    }
}
