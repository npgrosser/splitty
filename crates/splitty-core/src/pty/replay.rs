//! Historical terminal output for a client that renders it in a real terminal
//! (`splitty attach`): requests for terminal reports and clipboard writes are
//! removed, so replaying the scrollback neither types the terminal's answers
//! into the session nor overwrites a newer copy. Every other sequence, also a
//! color setter, a title or a hyperlink, is kept.
//!
//! The same rules as the web client's `frontend/src/lib/terminal/replay_output.ts`.
//! Works on bytes, so UTF-8 and sequences split across replay frames survive.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// ENQ asks the terminal for its answerback message.
const ENQ: u8 = 0x05;
/// CAN cancels a sequence in progress.
const CAN: u8 = 0x18;
const CSI: u8 = b'[';
const OSC: u8 = b']';
const DCS: u8 = b'P';
const APC: u8 = b'_';
const PM: u8 = b'^';

/// Filters one replay. Feed it every replayed frame with [`ReplayFilter::push`],
/// then call [`ReplayFilter::finish`] once before live output follows.
#[derive(Debug, Default)]
pub struct ReplayFilter {
    pending: Vec<u8>,
}

impl ReplayFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes of `data` to show, holding back an escape sequence that has
    /// not ended yet.
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(data.len());
        for &byte in data {
            if self.pending.is_empty() {
                if byte == ESC {
                    self.pending.push(byte);
                } else if byte != ENQ {
                    output.push(byte);
                }
                continue;
            }
            self.pending.push(byte);
            let kind = self.pending[1];
            let length = self.pending.len();
            if length == 2 && matches!(kind, CSI | OSC | DCS | APC | PM) {
                continue;
            }
            let complete = match kind {
                OSC | DCS | APC | PM => {
                    (kind == OSC && byte == BEL)
                        || (length > 2 && self.pending[length - 2] == ESC && byte == b'\\')
                }
                CSI => length > 2 && (0x40..=0x7e).contains(&byte),
                _ => true,
            };
            if !complete {
                continue;
            }
            if !has_side_effect(&self.pending) {
                output.extend_from_slice(&self.pending);
            }
            self.pending.clear();
        }
        output
    }

    /// What is left at the end of the replay: a partial sequence is kept, but
    /// cancelled, so a trailing escape from the scrollback cannot combine with
    /// live output into a request.
    pub fn finish(&mut self) -> Vec<u8> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let mut tail = std::mem::take(&mut self.pending);
        tail.push(CAN);
        tail
    }
}

/// Whether a complete escape sequence asks the terminal for something.
fn has_side_effect(sequence: &[u8]) -> bool {
    match sequence[1] {
        OSC => osc_has_side_effect(&sequence[2..]),
        CSI => csi_has_side_effect(&sequence[2..]),
        DCS => sequence[2..].starts_with(b"+q") || sequence[2..].starts_with(b"$q"),
        kind => sequence.len() == 2 && kind == b'Z',
    }
}

/// Clipboard operations, and queries of the dynamic and palette colors.
fn osc_has_side_effect(body: &[u8]) -> bool {
    let Some(separator) = body.iter().position(|&b| b == b';') else {
        return false;
    };
    let command = &body[..separator];
    if command == b"52" {
        return true;
    }
    let color = command == b"4"
        || (command.len() == 2 && command[0] == b'1' && command[1].is_ascii_digit());
    color && body.contains(&b'?')
}

/// Device attributes, status and cursor reports, mode reports and window
/// reports. `body` holds what follows `ESC [`, the final byte included.
fn csi_has_side_effect(body: &[u8]) -> bool {
    let Some((&last, rest)) = body.split_last() else {
        return false;
    };
    let digits = |params: &[u8]| params.iter().all(|&b| b.is_ascii_digit() || b == b';');
    match last {
        // `CSI [?>=] Ps c`: primary, secondary and tertiary device attributes.
        b'c' => digits(
            rest.strip_prefix(b"?")
                .or(rest.strip_prefix(b">"))
                .or(rest.strip_prefix(b"="))
                .unwrap_or(rest),
        ),
        // `CSI [?] Ps n`: device status and cursor position reports.
        b'n' => digits(rest.strip_prefix(b"?").unwrap_or(rest)),
        // `CSI [?] Ps $ p`: mode reports (DECRQM).
        b'p' => rest
            .strip_suffix(b"$")
            .is_some_and(|params| digits(params.strip_prefix(b"?").unwrap_or(params))),
        // `CSI Ps t` with the report variants of the window operations.
        b't' => matches!(
            rest,
            b"11" | b"13" | b"14" | b"15" | b"18" | b"19" | b"20" | b"21"
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` replayed as two frames split at `split`, then finished.
    fn replay(text: &str, split: usize) -> Vec<u8> {
        let bytes = text.as_bytes();
        let mut filter = ReplayFilter::new();
        let mut out = filter.push(&bytes[..split]);
        out.extend(filter.push(&bytes[split..]));
        out.extend(filter.finish());
        out
    }

    #[test]
    fn requests_and_clipboard_writes_are_removed_even_across_frame_boundaries() {
        for query in [
            "\x1b]11;?\x07",
            "\x1b]11;?\x1b\\",
            "\x1b]10;?\x07",
            "\x1b]52;c;Y29waWVk\x07",
            "\x1b]52;c;Y29waWVk\x1b\\",
            "\x1b]52;c;?\x07",
            "\x1b]4;0;?\x07",
            "\x1b[6n",
            "\x1b[?6n",
            "\x1b[c",
            "\x1b[>0c",
            "\x1b[?2026$p",
            "\x1b[18t",
            "\x1bP$qm\x1b\\",
            "\x1bZ",
            "\x05",
        ] {
            let text = format!("Grüße{query}❯ ");
            for split in 0..=text.len() {
                assert_eq!(
                    replay(&text, split),
                    "Grüße❯ ".as_bytes(),
                    "{query:?} split at {split}"
                );
            }
        }
    }

    #[test]
    fn colors_cursor_titles_alternate_screen_and_hyperlinks_are_kept() {
        let text = "\x1b[31mred\x1b[0m\x1b[2;3H\x1b]11;rgb:0f/0f/0f\x07\x1b]2;title?\x07\x1b[?1049h\x1b]8;;https://example.com/?q=x\x1b\\link\x1b[8;24;80t";
        for split in 0..=text.len() {
            assert_eq!(replay(text, split), text.as_bytes(), "split at {split}");
        }
    }

    #[test]
    fn an_unfinished_escape_is_cancelled_before_live_output() {
        assert_eq!(replay("prompt\x1b]11;", 8), b"prompt\x1b]11;\x18");
        assert!(ReplayFilter::new().finish().is_empty());
    }
}
