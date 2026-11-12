//! The terminal modes a session's output leaves set, followed so that a replay
//! can start in them.
//!
//! The scrollback keeps only the newest output. A mode a program set before the
//! oldest retained byte (bracketed paste, mouse reporting, the kitty keyboard
//! flags, the alternate screen, …) is not in a replay, so a client's fresh
//! terminal would never learn it. [`ModeTracker`] reads every chunk the session
//! records, notes each change of the modes below with the output offset of the
//! sequence that made it, and [`ModeTracker::restore`] returns the sequences
//! that put a fresh terminal into the state the output from a given offset on
//! starts in:
//!
//! - the alternate screen as it was at that offset. Its switches within the
//!   retained output are kept, so the output before the first of them lands on
//!   the screen it was written to;
//! - every other mode as its last change before that offset left it. A mode
//!   changed at or after the offset is left to the replay, which carries the
//!   change; until then the terminal has it as a fresh one does, which for the
//!   input modes does not matter, since only their state after the replay
//!   counts. A kitty keyboard stack counts as one mode: when it changed within
//!   the retained output, the replay's pushes and pops start from an empty
//!   stack.
//!
//! Synchronized output (`?2026`) belongs to one frame and is not followed.
//! Where terminals differ, the tracker follows xterm: its soft reset, and a
//! reset of a mouse encoding that is not in effect, which changes nothing.
//!
//! The tap reads 7-bit sequences only: in UTF-8 output a byte such as 0x9b is
//! part of a character, not CSI. It skips OSC, DCS, APC, PM and SOS strings up
//! to their terminator, an ESC ends a string as it does in a terminal, CAN and
//! SUB cancel a sequence, and a sequence split between chunks counts once.

use std::collections::VecDeque;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// The parameter and intermediate bytes of a CSI sequence the tracker reads;
/// a longer sequence changes no mode.
const CSI_MAX: usize = 64;

/// The levels a kitty keyboard stack holds, its base included, as terminals
/// that implement the protocol bound it; a push beyond evicts the oldest.
const KEYBOARD_LEVELS: usize = 8;

/// The screen switches within the retained output the tracker keeps. Beyond,
/// the oldest pair is forgotten: a program that switches screens that often
/// gets the screen of the oldest byte wrong only between those two switches.
const SCREEN_SWITCHES: usize = 1024;

/// The DEC private modes kept as on/off flags, in the order they are restored:
/// their number and their state in a fresh terminal, `None` where terminals
/// differ. The application keypad (`?66`) is also set by `ESC =` and `ESC >`.
const FLAGS: [(u32, Option<bool>); 9] = [
    (1, Some(false)),    // application cursor keys
    (66, Some(false)),   // application keypad
    (7, Some(true)),     // autowrap
    (25, Some(true)),    // cursor visible
    (2004, Some(false)), // bracketed paste
    (1004, Some(false)), // focus reports
    (1007, None),        // alternate scroll
    (2031, Some(false)), // color-scheme reports
    (2048, Some(false)), // in-band size reports
];

/// Where the tap is in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    /// After an ESC at `start`.
    Escape {
        start: u64,
    },
    /// After ESC and an intermediate byte, as in a character set designation.
    EscapeIntermediate,
    /// In a CSI sequence that began at `start`; its bytes so far are in `csi`.
    Csi {
        start: u64,
    },
    /// In a CSI sequence too long to change a mode, up to its final byte.
    CsiIgnore,
    /// In an OSC (`osc`, which BEL also ends), DCS, APC, PM or SOS string.
    String {
        osc: bool,
    },
}

/// A mode's value and the offset of the sequence that last changed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Tracked<T> {
    value: T,
    at: u64,
}

impl<T: Copy + PartialEq> Tracked<T> {
    fn change(&mut self, value: T, at: u64) {
        if self.value != value {
            *self = Self { value, at };
        }
    }

    /// The value the output from `oldest` on starts with, when a change before
    /// it left one a fresh terminal does not have.
    fn restored(&self, oldest: u64, fresh: T) -> Option<T> {
        (self.at < oldest && self.value != fresh).then_some(self.value)
    }
}

/// A kitty keyboard flags stack.
#[derive(Clone, Debug, PartialEq, Eq)]
struct KeyboardStack {
    /// The flags of each level, the base first; never empty.
    levels: Vec<u8>,
    at: u64,
}

impl Default for KeyboardStack {
    fn default() -> Self {
        Self {
            levels: vec![0],
            at: 0,
        }
    }
}

impl KeyboardStack {
    /// `CSI > flags u`.
    fn push(&mut self, flags: u8, at: u64) {
        if self.levels.len() == KEYBOARD_LEVELS {
            self.levels.remove(0);
        }
        self.levels.push(flags);
        self.at = at;
    }

    /// `CSI < count u`: popping the last level resets all flags.
    fn pop(&mut self, count: u32, at: u64) {
        let before = self.levels.clone();
        // Ends after as many steps as there are levels, whatever `count` is.
        for _ in 0..count {
            if self.levels.len() > 1 {
                self.levels.pop();
            } else {
                self.levels[0] = 0;
                break;
            }
        }
        if self.levels != before {
            self.at = at;
        }
    }

    /// `CSI = flags ; mode u`: replace (1), set (2) or clear (3) the current
    /// level's flags.
    fn set(&mut self, flags: u8, mode: u32, at: u64) {
        let Some(top) = self.levels.last_mut() else {
            return;
        };
        let value = match mode {
            1 => flags,
            2 => *top | flags,
            3 => *top & !flags,
            _ => return,
        };
        if *top != value {
            *top = value;
            self.at = at;
        }
    }

    fn reset(&mut self, at: u64) {
        if self.levels != [0] {
            *self = Self {
                levels: vec![0],
                at,
            };
        }
    }

    /// Rebuild the stack in a fresh terminal, whose stack is empty.
    fn restore(&self, oldest: u64, out: &mut Vec<u8>) {
        if self.at >= oldest || self.levels == [0] {
            return;
        }
        let Some((base, pushed)) = self.levels.split_first() else {
            return;
        };
        if *base != 0 {
            csi(out, &format!("={base};1u"));
        }
        for flags in pushed {
            csi(out, &format!(">{flags}u"));
        }
    }
}

/// The normal and the alternate screen, with the switches within the retained
/// output.
#[derive(Debug, Default)]
struct Screens {
    /// The screen before the first of `switches`: `Some(mode)` for the
    /// alternate screen, entered with that mode.
    before: Option<u32>,
    /// The switches at or after the oldest retained byte, oldest first: where
    /// each happened and the screen it switched to. They alternate between the
    /// two screens.
    switches: VecDeque<(u64, Option<u32>)>,
}

impl Screens {
    fn current(&self) -> Option<u32> {
        self.switches
            .back()
            .map_or(self.before, |&(_, screen)| screen)
    }

    /// Switch to the alternate screen (`Some`) or back; a switch to the screen
    /// in use changes nothing, as in xterm.
    fn switch(&mut self, to: Option<u32>, at: u64) {
        if to.is_some() == self.current().is_some() {
            return;
        }
        self.switches.push_back((at, to));
        if self.switches.len() > SCREEN_SWITCHES {
            self.switches.drain(..2);
        }
    }

    /// Forget the switches before `oldest`: their result is the screen there.
    fn prune(&mut self, oldest: u64) {
        while let Some(&(at, screen)) = self.switches.front() {
            if at >= oldest {
                break;
            }
            self.before = screen;
            self.switches.pop_front();
        }
    }

    /// The screen in use at `oldest`.
    fn at(&self, oldest: u64) -> Option<u32> {
        self.switches
            .iter()
            .take_while(|(at, _)| *at < oldest)
            .last()
            .map_or(self.before, |&(_, screen)| screen)
    }
}

/// Follows a session's terminal modes; see the module documentation.
#[derive(Debug)]
pub struct ModeTracker {
    state: State,
    csi: Vec<u8>,
    /// The offset of the next byte.
    position: u64,
    screens: Screens,
    /// The kitty keyboard stacks of the normal and the alternate screen.
    keyboard: [KeyboardStack; 2],
    flags: [Tracked<Option<bool>>; FLAGS.len()],
    /// `?9`, `?1000`, `?1002` or `?1003`; the last one set is in effect.
    mouse: Tracked<Option<u32>>,
    /// `?1005`, `?1006`, `?1015` or `?1016`.
    encoding: Tracked<Option<u32>>,
    /// `CSI Ps SP q`, 0 for the terminal's default.
    cursor_style: Tracked<u32>,
    /// `CSI > 4 ; Pv m`.
    modify_other_keys: Tracked<u32>,
}

impl Default for ModeTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ModeTracker {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            csi: Vec::new(),
            position: 0,
            screens: Screens::default(),
            keyboard: Default::default(),
            flags: Default::default(),
            mouse: Tracked::default(),
            encoding: Tracked::default(),
            cursor_style: Tracked::default(),
            modify_other_keys: Tracked::default(),
        }
    }

    /// Read the next chunk of output. `oldest` is the offset of the oldest
    /// byte the scrollback holds once it has this chunk too.
    pub fn feed(&mut self, data: &[u8], oldest: u64) {
        let start = self.position;
        let mut index = 0;
        while index < data.len() {
            let rest = &data[index..];
            let skip = match self.state {
                State::Ground => rest.iter().position(|&byte| byte == ESC),
                State::String { osc } => rest
                    .iter()
                    .position(|&byte| matches!(byte, ESC | CAN | SUB) || (osc && byte == BEL)),
                _ => Some(0),
            };
            let Some(skip) = skip else {
                break;
            };
            index += skip;
            let at = start + u64::try_from(index).expect("a chunk fits a 64-bit offset");
            self.step(data[index], at);
            index += 1;
        }
        self.position = start + u64::try_from(data.len()).expect("a chunk fits a 64-bit offset");
        debug_assert!(oldest <= self.position, "the scrollback holds no future");
        self.screens.prune(oldest);
    }

    /// The sequences that put a fresh terminal into the modes the output from
    /// `oldest` on starts in: the normal screen's keyboard flags, the alternate
    /// screen with its flags, then the other modes.
    pub fn restore(&self, oldest: u64) -> Vec<u8> {
        let mut out = Vec::new();
        self.keyboard[0].restore(oldest, &mut out);
        if let Some(mode) = self.screens.at(oldest) {
            csi(&mut out, &format!("?{mode}h"));
            self.keyboard[1].restore(oldest, &mut out);
        }
        for (flag, &(mode, fresh)) in self.flags.iter().zip(FLAGS.iter()) {
            let Some(Some(on)) = flag.restored(oldest, fresh) else {
                continue;
            };
            match (mode, on) {
                (66, true) => out.extend_from_slice(b"\x1b="),
                (66, false) => out.extend_from_slice(b"\x1b>"),
                (_, true) => csi(&mut out, &format!("?{mode}h")),
                (_, false) => csi(&mut out, &format!("?{mode}l")),
            }
        }
        if let Some(style) = self.cursor_style.restored(oldest, 0) {
            csi(&mut out, &format!("{style} q"));
        }
        for tracked in [self.mouse, self.encoding] {
            if let Some(Some(mode)) = tracked.restored(oldest, None) {
                csi(&mut out, &format!("?{mode}h"));
            }
        }
        if let Some(level) = self.modify_other_keys.restored(oldest, 0) {
            csi(&mut out, &format!(">4;{level}m"));
        }
        out
    }

    fn step(&mut self, byte: u8, at: u64) {
        // In every state CAN and SUB cancel a sequence and ESC starts one.
        match byte {
            CAN | SUB => {
                self.state = State::Ground;
                return;
            }
            ESC => {
                self.state = State::Escape { start: at };
                return;
            }
            _ => {}
        }
        match self.state {
            State::Ground => {}
            State::Escape { start } => match byte {
                b'[' => {
                    self.csi.clear();
                    self.state = State::Csi { start };
                }
                b']' => self.state = State::String { osc: true },
                b'P' | b'X' | b'^' | b'_' => self.state = State::String { osc: false },
                0x20..=0x2f => self.state = State::EscapeIntermediate,
                0x30..=0x7e => {
                    self.state = State::Ground;
                    self.escape(byte, start);
                }
                // Other C0 controls act within the sequence, DEL is ignored.
                0x00..=0x1f | 0x7f => {}
                _ => self.state = State::Ground,
            },
            State::EscapeIntermediate => {
                if matches!(byte, 0x30..=0x7e | 0x80..=0xff) {
                    self.state = State::Ground;
                }
            }
            State::Csi { start } => match byte {
                0x20..=0x3f if self.csi.len() < CSI_MAX => self.csi.push(byte),
                0x20..=0x3f => self.state = State::CsiIgnore,
                0x40..=0x7e => {
                    self.state = State::Ground;
                    let body = std::mem::take(&mut self.csi);
                    self.apply_csi(&body, byte, start);
                    self.csi = body;
                }
                0x80..=0xff => self.state = State::Ground,
                _ => {}
            },
            State::CsiIgnore => {
                if matches!(byte, 0x40..=0x7e | 0x80..=0xff) {
                    self.state = State::Ground;
                }
            }
            State::String { osc } => {
                if osc && byte == BEL {
                    self.state = State::Ground;
                }
            }
        }
    }

    /// `ESC` followed by the final byte `last`.
    fn escape(&mut self, last: u8, at: u64) {
        match last {
            b'=' => self.set_flag(66, true, at),
            b'>' => self.set_flag(66, false, at),
            b'c' => self.full_reset(at),
            _ => {}
        }
    }

    /// A complete CSI sequence: `body` holds the bytes between `CSI` and its
    /// final byte `last`.
    fn apply_csi(&mut self, body: &[u8], last: u8, at: u64) {
        let (marker, rest) = match body.split_first() {
            Some((&marker @ b'<'..=b'?', rest)) => (Some(marker), rest),
            _ => (None, body),
        };
        let split = rest
            .iter()
            .position(|byte| (0x20..=0x2f).contains(byte))
            .unwrap_or(rest.len());
        let (params, intermediates) = rest.split_at(split);
        if !params
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b';' | b':'))
            || !intermediates
                .iter()
                .all(|byte| (0x20..=0x2f).contains(byte))
        {
            return;
        }
        let mut values = params.split(|&byte| byte == b';').map(parameter);
        match (marker, intermediates, last) {
            (Some(b'?'), b"", b'h' | b'l') => {
                for mode in values.flatten() {
                    self.dec_mode(mode, last == b'h', at);
                }
            }
            (None, b"!", b'p') if params.is_empty() => self.soft_reset(at),
            (None, b" ", b'q') => {
                let style = values.next().flatten().unwrap_or(0);
                if style <= 6 {
                    self.cursor_style.change(style, at);
                }
            }
            (Some(b'>'), b"", b'u') => {
                let flags = keyboard_flags(values.next().flatten().unwrap_or(0));
                self.keyboard_mut().push(flags, at);
            }
            (Some(b'<'), b"", b'u') => {
                let count = values.next().flatten().unwrap_or(1);
                self.keyboard_mut().pop(count, at);
            }
            (Some(b'='), b"", b'u') => {
                let flags = keyboard_flags(values.next().flatten().unwrap_or(0));
                let mode = values.next().flatten().unwrap_or(1);
                self.keyboard_mut().set(flags, mode, at);
            }
            // `CSI > m` resets every key modifier option, `CSI > 4 m` this one.
            (Some(b'>'), b"", b'm') if params.is_empty() => self.modify_other_keys.change(0, at),
            (Some(b'>'), b"", b'm') => match (values.next().flatten(), values.next().flatten()) {
                (Some(4), None) => self.modify_other_keys.change(0, at),
                (Some(4), Some(level @ 0..=3)) => self.modify_other_keys.change(level, at),
                _ => {}
            },
            // `CSI > 4 n` disables modifyOtherKeys.
            (Some(b'>'), b"", b'n') if params == b"4" => self.modify_other_keys.change(0, at),
            _ => {}
        }
    }

    /// `CSI ? mode h` (`on`) or `CSI ? mode l`.
    fn dec_mode(&mut self, mode: u32, on: bool, at: u64) {
        match mode {
            47 | 1047 | 1049 => self.screens.switch(on.then_some(mode), at),
            // Resetting any tracking mode turns mouse tracking off.
            9 | 1000 | 1002 | 1003 => self.mouse.change(on.then_some(mode), at),
            // A reset only takes back the encoding in effect.
            1005 | 1006 | 1015 | 1016 => {
                if on {
                    self.encoding.change(Some(mode), at);
                } else if self.encoding.value == Some(mode) {
                    self.encoding.change(None, at);
                }
            }
            _ => self.set_flag(mode, on, at),
        }
    }

    fn set_flag(&mut self, mode: u32, on: bool, at: u64) {
        if let Some(index) = FLAGS.iter().position(|&(flag, _)| flag == mode) {
            self.flags[index].change(Some(on), at);
        }
    }

    /// The kitty keyboard stack of the screen in use.
    fn keyboard_mut(&mut self) -> &mut KeyboardStack {
        &mut self.keyboard[usize::from(self.screens.current().is_some())]
    }

    /// `ESC c`: every mode as in a fresh terminal, on the normal screen.
    fn full_reset(&mut self, at: u64) {
        self.screens.switch(None, at);
        for stack in &mut self.keyboard {
            stack.reset(at);
        }
        for flag in &mut self.flags {
            flag.change(None, at);
        }
        self.mouse.change(None, at);
        self.encoding.change(None, at);
        self.cursor_style.change(0, at);
        self.modify_other_keys.change(0, at);
    }

    /// `CSI ! p`, the modes xterm's soft reset resets: the cursor visible in its
    /// default style, autowrap on, cursor keys and keypad normal, and the key
    /// modifier options at their initial values.
    fn soft_reset(&mut self, at: u64) {
        self.set_flag(25, true, at);
        self.set_flag(7, true, at);
        self.set_flag(1, false, at);
        self.set_flag(66, false, at);
        self.cursor_style.change(0, at);
        self.modify_other_keys.change(0, at);
    }
}

/// A numeric parameter: `None` when empty, and no known mode when it has
/// sub-parameters or does not fit.
fn parameter(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.contains(&b':') {
        return Some(u32::MAX);
    }
    Some(bytes.iter().fold(0u32, |value, &digit| {
        value
            .saturating_mul(10)
            .saturating_add(u32::from(digit - b'0'))
    }))
}

/// The kitty keyboard protocol's five flags.
fn keyboard_flags(value: u32) -> u8 {
    u8::try_from(value & 0x1f).unwrap_or(0)
}

fn csi(out: &mut Vec<u8>, body: &str) {
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(body.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that move every mode set before them out of a 4 KiB scrollback.
    const FILLER: [u8; 8192] = [b'x'; 8192];

    /// `before`, then enough output to push it out of a 4 KiB scrollback, then
    /// `within`; the restore for the output that 4 KiB scrollback holds.
    fn restore_after(before: &[u8], within: &[u8]) -> Vec<u8> {
        let mut tracker = ModeTracker::new();
        let mut total = 0u64;
        for chunk in [before, &FILLER[..], within] {
            total += u64::try_from(chunk.len()).expect("length");
            tracker.feed(chunk, total.saturating_sub(4096));
        }
        tracker.restore(total.saturating_sub(4096))
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn a_fresh_or_unwrapped_session_restores_nothing() {
        assert!(ModeTracker::new().restore(0).is_empty());
        let mut tracker = ModeTracker::new();
        tracker.feed(b"\x1b[?1049h\x1b[?2004h\x1b[>1u", 0);
        assert!(tracker.restore(0).is_empty(), "the replay holds them");
    }

    /// terminal-sessions, Terminal Modes in the Replay (Modes set before the
    /// buffered output): each mode, set before the oldest byte, is restored.
    #[test]
    fn every_tracked_mode_is_restored() {
        for (set, restored) in [
            (&b"\x1b[?1049h"[..], &b"\x1b[?1049h"[..]),
            (b"\x1b[?1047h", b"\x1b[?1047h"),
            (b"\x1b[?47h", b"\x1b[?47h"),
            (b"\x1b[?7l", b"\x1b[?7l"),
            (b"\x1b[?1h", b"\x1b[?1h"),
            (b"\x1b=", b"\x1b="),
            (b"\x1b[?66h", b"\x1b="),
            (b"\x1b[?25l", b"\x1b[?25l"),
            (b"\x1b[5 q", b"\x1b[5 q"),
            (b"\x1b[?2004h", b"\x1b[?2004h"),
            (b"\x1b[?1004h", b"\x1b[?1004h"),
            (b"\x1b[?9h", b"\x1b[?9h"),
            (b"\x1b[?1000h", b"\x1b[?1000h"),
            (b"\x1b[?1002h", b"\x1b[?1002h"),
            (b"\x1b[?1003h", b"\x1b[?1003h"),
            (b"\x1b[?1005h", b"\x1b[?1005h"),
            (b"\x1b[?1006h", b"\x1b[?1006h"),
            (b"\x1b[?1015h", b"\x1b[?1015h"),
            (b"\x1b[?1016h", b"\x1b[?1016h"),
            (b"\x1b[?1007h", b"\x1b[?1007h"),
            (b"\x1b[?1007l", b"\x1b[?1007l"),
            (b"\x1b[?2031h", b"\x1b[?2031h"),
            (b"\x1b[?2048h", b"\x1b[?2048h"),
            (b"\x1b[>4;2m", b"\x1b[>4;2m"),
            (b"\x1b[>4;1m", b"\x1b[>4;1m"),
            (b"\x1b[>1u", b"\x1b[>1u"),
            (b"\x1b[=5u", b"\x1b[=5;1u"),
        ] {
            assert_eq!(
                text(&restore_after(set, b"")),
                text(restored),
                "after {:?}",
                text(set)
            );
        }
    }

    #[test]
    fn modes_back_at_their_fresh_state_are_not_restored() {
        for set in [
            &b"\x1b[?2004h\x1b[?2004l"[..],
            b"\x1b[?25l\x1b[?25h",
            b"\x1b[?7l\x1b[?7h",
            b"\x1b=\x1b>",
            b"\x1b[5 q\x1b[0 q",
            b"\x1b[>4;2m\x1b[>4m",
            b"\x1b[>4;2m\x1b[>m",
            b"\x1b[>4;2m\x1b[>4n",
            b"\x1b[?1049h\x1b[?1049l",
            b"\x1b[>1u\x1b[<u",
            b"\x1b[?1000h\x1b[?1000l",
            b"\x1b[?1006h\x1b[?1006l",
        ] {
            assert_eq!(text(&restore_after(set, b"")), "", "after {:?}", text(set));
        }
    }

    /// Several modes in one sequence, and an unknown one among them.
    #[test]
    fn one_sequence_sets_several_modes() {
        assert_eq!(
            text(&restore_after(b"\x1b[?1;12;2004h", b"")),
            "\x1b[?1h\x1b[?2004h"
        );
    }

    /// terminal-sessions, Terminal Modes in the Replay (Mode changed within the
    /// buffered output).
    #[test]
    fn a_mode_changed_within_the_buffered_output_is_left_to_the_replay() {
        assert_eq!(
            text(&restore_after(
                b"\x1b[?2004h\x1b[?1000h",
                b"\x1b[?2004l\x1b[?2004h"
            )),
            "\x1b[?1000h"
        );
        assert_eq!(text(&restore_after(b"\x1b[>1u", b"\x1b[<u\x1b[>1u")), "");
    }

    /// terminal-sessions, Terminal Modes in the Replay (Alternate screen left
    /// within the buffered output): the screen at the oldest byte counts, not
    /// the last switch.
    #[test]
    fn the_alternate_screen_is_restored_as_it_was_at_the_oldest_byte() {
        assert_eq!(
            text(&restore_after(b"\x1b[?1049h", b"\x1b[?1049l$ ")),
            "\x1b[?1049h"
        );
        assert_eq!(
            text(&restore_after(b"\x1b[?1049h\x1b[?1049l", b"\x1b[?1049h")),
            ""
        );
        // Entering it again changes nothing, and any variant leaves it.
        assert_eq!(
            text(&restore_after(b"\x1b[?47h\x1b[?1049h", b"")),
            "\x1b[?47h"
        );
        assert_eq!(text(&restore_after(b"\x1b[?1049h\x1b[?47l", b"")), "");
    }

    /// The switches the tracker keeps are those within the retained output,
    /// and at most [`SCREEN_SWITCHES`] of them.
    #[test]
    fn the_screen_switches_are_pruned_and_bounded() {
        let mut tracker = ModeTracker::new();
        let mut total = 0u64;
        for _ in 0..3000 {
            for switch in [&b"\x1b[?1049h"[..], b"\x1b[?1049l"] {
                total += 8;
                tracker.feed(switch, total.saturating_sub(1 << 20));
            }
        }
        assert!(tracker.screens.switches.len() <= SCREEN_SWITCHES);
        assert!(!tracker.screens.switches.is_empty());
        tracker.feed(&[b'x'; 64], total + 64 - 16);
        assert_eq!(
            tracker.screens.switches.len(),
            0,
            "all before the oldest byte"
        );
        assert_eq!(tracker.screens.before, None);
        // Within a small window the screen is known exactly.
        let mut tracker = ModeTracker::new();
        tracker.feed(b"\x1b[?1049hxxxx\x1b[?1049lyyyy\x1b[?1049h", 0);
        assert_eq!(tracker.screens.switches.len(), 3);
        assert_eq!(text(&tracker.restore(9)), "\x1b[?1049h");
        assert_eq!(text(&tracker.restore(13)), "");
        assert_eq!(text(&tracker.restore(24)), "");
        // A switch the oldest byte cuts counts as before it.
        assert_eq!(text(&tracker.restore(25)), "\x1b[?1049h");
    }

    /// terminal-sessions, Terminal Modes in the Replay (Kitty keyboard flags of
    /// both screens).
    #[test]
    fn kitty_flags_are_restored_per_screen_around_the_switch() {
        assert_eq!(
            text(&restore_after(
                b"\x1b[>1u\x1b[?1049h\x1b[>5u\x1b[?2004h",
                b""
            )),
            "\x1b[>1u\x1b[?1049h\x1b[>5u\x1b[?2004h"
        );
        // Leaving the alternate screen returns to the normal screen's stack.
        assert_eq!(
            text(&restore_after(
                b"\x1b[>1u\x1b[?1049h\x1b[>5u\x1b[<u\x1b[?1049l",
                b""
            )),
            "\x1b[>1u"
        );
        // The alternate screen's stack counts only when the replay starts there.
        assert_eq!(
            text(&restore_after(b"\x1b[?1049h\x1b[>5u\x1b[?1049l", b"")),
            ""
        );
    }

    #[test]
    fn kitty_flags_follow_push_pop_and_set() {
        for (sequences, restored) in [
            (&b"\x1b[>1u\x1b[>3u"[..], &b"\x1b[>1u\x1b[>3u"[..]),
            (b"\x1b[>1u\x1b[>3u\x1b[<u", b"\x1b[>1u"),
            (b"\x1b[>1u\x1b[>3u\x1b[<2u", b""),
            (b"\x1b[>1u\x1b[<5u", b""),
            (b"\x1b[>1u\x1b[<0u", b"\x1b[>1u"),
            (b"\x1b[>u", b"\x1b[>0u"),
            (b"\x1b[>1u\x1b[=4;2u", b"\x1b[>5u"),
            (b"\x1b[>7u\x1b[=2;3u", b"\x1b[>5u"),
            (b"\x1b[>7u\x1b[=8u", b"\x1b[>8u"),
            (b"\x1b[>7u\x1b[=8;4u", b"\x1b[>7u"),
            (b"\x1b[=3u\x1b[>1u", b"\x1b[=3;1u\x1b[>1u"),
            (b"\x1b[=3u\x1b[<u", b""),
            (b"\x1b[>255u", b"\x1b[>31u"),
        ] {
            assert_eq!(
                text(&restore_after(sequences, b"")),
                text(restored),
                "after {:?}",
                text(sequences)
            );
        }
    }

    #[test]
    fn a_kitty_stack_evicts_its_oldest_level() {
        let mut pushes = Vec::new();
        for flags in 1..=10 {
            pushes.extend_from_slice(format!("\x1b[>{flags}u").as_bytes());
        }
        assert_eq!(
            text(&restore_after(&pushes, b"")),
            "\x1b[=3;1u\x1b[>4u\x1b[>5u\x1b[>6u\x1b[>7u\x1b[>8u\x1b[>9u\x1b[>10u"
        );
    }

    /// terminal-sessions, Terminal Modes in the Replay (Synchronized output is
    /// not restored).
    #[test]
    fn synchronized_output_is_not_restored() {
        assert_eq!(text(&restore_after(b"\x1b[?2026h", b"")), "");
        assert_eq!(
            text(&restore_after(b"\x1b[?2026;2004h", b"")),
            "\x1b[?2004h"
        );
    }

    /// terminal-sessions, Tracked Terminal Modes (Full reset).
    #[test]
    fn a_full_reset_clears_every_mode() {
        let modes = b"\x1b[?1049h\x1b[>1u\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?25l\
            \x1b[?1h\x1b=\x1b[3 q\x1b[>4;2m\x1b[?1007l";
        let mut reset = modes.to_vec();
        reset.extend_from_slice(b"\x1bc");
        assert_eq!(text(&restore_after(&reset, b"")), "");
        // The alternate screen's stack goes too.
        let mut tracker = ModeTracker::new();
        tracker.feed(b"\x1b[>1u\x1b[?1049h\x1b[>5u\x1bc\x1b[?1049h", 0);
        assert!(tracker.keyboard.iter().all(|stack| stack.levels == [0]));
    }

    /// terminal-sessions, Tracked Terminal Modes (Soft reset): xterm's set.
    #[test]
    fn a_soft_reset_resets_what_xterm_resets() {
        assert_eq!(
            text(&restore_after(
                b"\x1b[?25l\x1b[?7l\x1b[?1h\x1b=\x1b[4 q\x1b[>4;2m\
                  \x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1049h\x1b[>1u\x1b[!p",
                b""
            )),
            "\x1b[?1049h\x1b[>1u\x1b[?2004h\x1b[?1004h\x1b[?1002h"
        );
        // `CSI ! p` with a parameter is no soft reset.
        assert_eq!(text(&restore_after(b"\x1b[?25l\x1b[1!p", b"")), "\x1b[?25l");
    }

    /// terminal-sessions, Tracked Terminal Modes (The last mouse tracking mode
    /// counts).
    #[test]
    fn the_last_mouse_tracking_mode_counts() {
        assert_eq!(
            text(&restore_after(b"\x1b[?1002h\x1b[?1003h", b"")),
            "\x1b[?1003h"
        );
        assert_eq!(
            text(&restore_after(b"\x1b[?1002h\x1b[?1003h\x1b[?1002l", b"")),
            ""
        );
        // A reset of an encoding not in effect keeps the one that is.
        assert_eq!(
            text(&restore_after(b"\x1b[?1000h\x1b[?1006h\x1b[?1015l", b"")),
            "\x1b[?1000h\x1b[?1006h"
        );
        assert_eq!(
            text(&restore_after(b"\x1b[?1005h\x1b[?1006h", b"")),
            "\x1b[?1006h"
        );
    }

    /// terminal-sessions, Tracked Terminal Modes (Sequences end where strings
    /// end).
    #[test]
    fn strings_are_skipped_up_to_their_terminator() {
        for (output, restored) in [
            (
                &b"\x1bP[?2004h\x1b\\\x1b]2;[?1049h\x07\x1b[?25l"[..],
                &b"\x1b[?25l"[..],
            ),
            // BEL ends an OSC string only; an ESC ends any string.
            (b"\x1bPq#0\x07[?2004h\x1b\\\x1b[?25l", b"\x1b[?25l"),
            (b"\x1b_Gf=24;AAAA\x07=\x1b\\\x1b[?1h", b"\x1b[?1h"),
            (b"\x1b^private=\x1b\\\x1bXsos>\x1b\\\x1b=", b"\x1b="),
            (b"\x1b]0;title\x1b[?2004h", b"\x1b[?2004h"),
            // CAN cancels a string, and what follows is text.
            (b"\x1b]2;tit\x18[?2004h", b""),
        ] {
            assert_eq!(
                text(&restore_after(output, b"")),
                text(restored),
                "after {:?}",
                text(output)
            );
        }
    }

    /// terminal-sessions, Tracked Terminal Modes (Sequence split between
    /// reads), for every split point of a mixed stretch of output.
    #[test]
    fn a_sequence_split_between_chunks_counts_once() {
        let output =
            "ü\x1b]2;tit\x1b\\\x1b(B\x1b[?2004h\x1bP+q\x07\x1b\\\x1b[>1u\x1b[38;2;1;2;3m\x1b=";
        let expected = text(&restore_after(output.as_bytes(), b""));
        assert_eq!(expected, "\x1b[>1u\x1b=\x1b[?2004h");
        let bytes = output.as_bytes();
        let oldest = u64::try_from(bytes.len() + FILLER.len() - 4096).expect("offset");
        for split in 0..=bytes.len() {
            let mut tracker = ModeTracker::new();
            tracker.feed(&bytes[..split], 0);
            tracker.feed(&bytes[split..], 0);
            tracker.feed(&FILLER, oldest);
            assert_eq!(text(&tracker.restore(oldest)), expected, "split at {split}");
        }
    }

    /// terminal-sessions, Tracked Terminal Modes (Eight-bit control bytes).
    #[test]
    fn eight_bit_controls_are_text() {
        assert_eq!(text(&restore_after("\u{9b}?2004h".as_bytes(), b"")), "");
        assert_eq!(text(&restore_after(b"\x9b?2004h\x9d2;t\x07", b"")), "");
    }

    #[test]
    fn controls_cancel_or_pass_through_a_sequence() {
        // CAN and SUB cancel; another C0 control acts within it.
        assert_eq!(text(&restore_after(b"\x1b[?2004\x18h", b"")), "");
        assert_eq!(text(&restore_after(b"\x1b[?2004\x1ah", b"")), "");
        assert_eq!(text(&restore_after(b"\x1b[?20\r04h", b"")), "\x1b[?2004h");
        // An ESC starts the next sequence.
        assert_eq!(text(&restore_after(b"\x1b[?20\x1b[?1h", b"")), "\x1b[?1h");
        // ESC with an intermediate takes its final byte along.
        assert_eq!(text(&restore_after(b"\x1b(=\x1b#>", b"")), "");
    }

    #[test]
    fn malformed_or_overlong_sequences_change_nothing() {
        let mut long = b"\x1b[?".to_vec();
        long.extend(std::iter::repeat_n(b"1;", 40).flatten());
        long.extend_from_slice(b"2004h\x1b[?1h");
        assert_eq!(text(&restore_after(&long, b"")), "\x1b[?1h");
        for output in [
            &b"\x1b[2004h"[..],
            b"\x1b[?2004$h",
            b"\x1b[?20?04h",
            b"\x1b[?2004:1h",
            b"\x1b[?99999999999h",
            b"\x1b[7 q",
            b"\x1b[>4;9m",
            b"\x1b[>1;2m",
            b"\x1b[?u",
            b"\x1b[?2004\xc3h",
        ] {
            assert_eq!(text(&restore_after(output, b"")), "", "{:?}", text(output));
        }
    }

    /// The restore passes the replay filters unchanged: it holds no report
    /// request.
    #[test]
    fn the_restore_passes_the_replay_filters() {
        let restore = restore_after(
            b"\x1b[>1u\x1b[?1049h\x1b[=5;1u\x1b[>3u\x1b[?1h\x1b=\x1b[?7l\x1b[?25l\x1b[2 q\
              \x1b[?2004h\x1b[?1004h\x1b[?1003h\x1b[?1006h\x1b[?1007h\x1b[?2031h\x1b[?2048h\x1b[>4;2m",
            b"",
        );
        assert_eq!(
            text(&restore),
            "\x1b[>1u\x1b[?1049h\x1b[=5;1u\x1b[>3u\x1b[?1h\x1b=\x1b[?7l\x1b[?25l\x1b[?2004h\
             \x1b[?1004h\x1b[?1007h\x1b[?2031h\x1b[?2048h\x1b[2 q\x1b[?1003h\x1b[?1006h\x1b[>4;2m"
        );
        let mut in_place = restore.clone();
        super::super::replay::filter_in_place(&mut in_place);
        assert_eq!(in_place, restore);
        let mut filter = super::super::replay::ReplayFilter::new();
        let mut streamed = filter.push(&restore);
        streamed.extend(filter.finish());
        assert_eq!(streamed, restore);
    }
}
