//! Historical terminal output for a client that renders it in a real terminal
//! (`splitty attach`): requests for terminal reports and clipboard writes are
//! removed, so replaying the scrollback neither types the terminal's answers
//! into the session nor overwrites a newer copy. The program status probe
//! counts as such a request. Every other sequence, also a
//! color setter, a title or a hyperlink, is kept.
//!
//! The same rules as the web client's `frontend/src/lib/terminal/replay_output.ts`.
//! Works on bytes, so UTF-8 and sequences split across replay frames survive.
//! A sequence ends where a terminal ends it, also at CAN, SUB and an ESC that
//! does not end a string: one that never ended asks for nothing, and what
//! follows it is judged on its own.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// ENQ asks the terminal for its answerback message.
const ENQ: u8 = 0x05;
/// CAN and SUB cancel a sequence in progress.
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
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
            self.step(byte, &mut output);
        }
        output
    }

    fn step(&mut self, byte: u8, output: &mut Vec<u8>) {
        if self.pending.is_empty() {
            if byte == ESC {
                self.pending.push(byte);
            } else if byte != ENQ {
                output.push(byte);
            }
            return;
        }
        let string = is_string(self.pending.get(1).copied());
        if byte == CAN || byte == SUB {
            output.append(&mut self.pending);
            output.push(byte);
            return;
        }
        if byte == ESC && !string {
            output.append(&mut self.pending);
            self.pending.push(byte);
            return;
        }
        if string && self.pending.len() > 2 && self.pending.last() == Some(&ESC) && byte != b'\\' {
            // An ESC that is not ST ended the string and starts a sequence of its own.
            self.pending.pop();
            output.append(&mut self.pending);
            self.pending.push(ESC);
            self.step(byte, output);
            return;
        }
        self.pending.push(byte);
        let kind = self.pending[1];
        let length = self.pending.len();
        if length == 2 && (kind == CSI || is_string(Some(kind))) {
            return;
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
            return;
        }
        if !has_side_effect(&self.pending) {
            output.extend_from_slice(&self.pending);
        }
        self.pending.clear();
    }

    /// What is left at the end of the replay: the bytes of a sequence that has
    /// not ended yet, unchanged. Live output follows the replay without a gap
    /// and ends that sequence, so the sequence is live; cancelling it showed
    /// the rest of a colour such as `ESC[38;2;153;153;153m` as text.
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

/// Filter a complete buffered replay without allocating a second scrollback.
/// Read and write cursors only move forward; retained bytes never overwrite
/// bytes the parser has yet to inspect. A trailing incomplete sequence is kept.
pub fn filter_in_place(data: &mut Vec<u8>) {
    let mut write = 0;
    let mut pending: Option<usize> = None;
    for read in 0..data.len() {
        let byte = data[read];
        let Some(mut start) = pending else {
            if byte == ESC {
                pending = Some(read);
            } else if byte != ENQ {
                data[write] = byte;
                write += 1;
            }
            continue;
        };
        let string = read > start + 1 && is_string(data.get(start + 1).copied());
        if byte == CAN || byte == SUB {
            data.copy_within(start..read + 1, write);
            write += read + 1 - start;
            pending = None;
            continue;
        }
        if byte == ESC && !string {
            data.copy_within(start..read, write);
            write += read - start;
            pending = Some(read);
            continue;
        }
        if string && read > start + 2 && data[read - 1] == ESC && byte != 0x5c {
            data.copy_within(start..read - 1, write);
            write += read - 1 - start;
            start = read - 1;
            pending = Some(start);
            if byte == ESC {
                data[write] = ESC;
                write += 1;
                pending = Some(read);
                continue;
            }
        }
        let length = read + 1 - start;
        let kind = data[start + 1];
        if length == 2 && (kind == CSI || is_string(Some(kind))) {
            continue;
        }
        let complete = match kind {
            OSC | DCS | APC | PM => {
                (kind == OSC && byte == BEL)
                    || (length > 2 && data[read - 1] == ESC && byte == 0x5c)
            }
            CSI => length > 2 && (0x40..=0x7e).contains(&byte),
            _ => true,
        };
        if complete {
            if !has_side_effect(&data[start..read + 1]) {
                data.copy_within(start..read + 1, write);
                write += length;
            }
            pending = None;
        }
    }
    if let Some(start) = pending {
        let end = data.len();
        data.copy_within(start..end, write);
        write += end - start;
    }
    data.truncate(write);
}

/// Whether the sequence introduced by `kind` is a string: OSC, DCS, APC or PM.
fn is_string(kind: Option<u8>) -> bool {
    matches!(kind, Some(OSC | DCS | APC | PM))
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

/// Clipboard operations, queries of the dynamic and palette colors, and the
/// program status probe (`OSC 7501 ; ?`), which a current server answers
/// itself and removes from the output, but an older one forwarded. `body`
/// holds what follows `ESC ]`, the terminator included.
fn osc_has_side_effect(body: &[u8]) -> bool {
    if body == b"7501;?\x07" || body == b"7501;?\x1b\\" {
        return true;
    }
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

/// Device attributes, status and cursor reports, mode reports, window
/// reports, the kitty keyboard query and the version report. `body` holds what follows `ESC [`, the final byte included.
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
            b"11" | b"13" | b"14" | b"15" | b"16" | b"18" | b"19" | b"20" | b"21"
        ),
        // `CSI ? u`: the kitty keyboard protocol's query of its flags.
        b'u' => rest == b"?",
        // `CSI > Ps q`: the terminal's name and version (XTVERSION).
        b'q' => rest.strip_prefix(b">").is_some_and(digits),
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
        let mut in_place = bytes.to_vec();
        filter_in_place(&mut in_place);
        assert_eq!(in_place, out, "in-place parser agrees at every split");
        out
    }

    #[test]
    fn in_place_filter_matches_streaming_parser_on_mixed_control_bytes() {
        let fragments: &[&[u8]] = &[
            b"plain",
            b"\x1b",
            b"\x1b]52;c;copy",
            b"\x07",
            b"\x1b[6n",
            b"\x18",
            b"\x1a",
            b"\x1b[31m",
            b"\x1bP$qm",
            b"\x05",
            b"\xff\xc3\xa4",
            b"\x1b\\",
            b"\x1b[",
            b"\x1b]2;title",
        ];
        for seed in 0..100 {
            let mut data = vec![];
            let mut cursor = seed;
            for _ in 0..64 {
                cursor = (cursor * 71 + 19) % 997;
                data.extend_from_slice(fragments[cursor % fragments.len()]);
            }
            let mut filter = ReplayFilter::new();
            let mut expected = filter.push(&data);
            expected.extend(filter.finish());
            filter_in_place(&mut data);
            assert_eq!(data, expected, "seed {seed}");
        }
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
            "\x1b[16t",
            "\x1b[?u",
            "\x1b[>q",
            "\x1b[>0q",
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

    /// terminal-sessions, Client Handling of Session Streams (Status probe in
    /// a replay from an older server): the probe with either terminator is
    /// dropped, also split across frames, while a status report passes.
    #[test]
    fn the_status_probe_is_removed_and_a_status_report_kept() {
        for probe in ["\x1b]7501;?\x07", "\x1b]7501;?\x1b\\"] {
            let text = format!("a{probe}b");
            for split in 0..=text.len() {
                assert_eq!(replay(&text, split), b"ab", "{probe:?} split at {split}");
            }
        }
        for kept in [
            "\x1b]7501;state=done\x07",
            "\x1b]7501;state=working:app=claude-code\x1b\\",
            "\x1b]7501;?:states=working\x07",
        ] {
            let text = format!("a{kept}b");
            for split in 0..=text.len() {
                assert_eq!(
                    replay(&text, split),
                    text.as_bytes(),
                    "{kept:?} split at {split}"
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
    fn a_request_after_a_sequence_that_never_ended_is_judged_on_its_own() {
        for before in [
            "\x1b[38;5",
            "\x1b]0;tit",
            "\x1b]52;c;Y29w\x18",
            "\x1bP+q544e\x1a",
        ] {
            for query in ["\x1b[c", "\x1b]11;?\x07", "\x1b[6n"] {
                let text = format!("{before}{query}❯ ");
                let expected = format!("{before}❯ ");
                for split in 0..=text.len() {
                    assert_eq!(
                        replay(&text, split),
                        expected.as_bytes(),
                        "{text:?} split at {split}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_output_after_a_cancelled_clipboard_write_is_kept() {
        let text = "\x1b]52;c;Y29w\x18\x1b[1mbold\x1b[0m\x1b]0;title\x1b\\";
        for split in 0..=text.len() {
            assert_eq!(replay(text, split), text.as_bytes(), "split at {split}");
        }
    }

    #[test]
    fn a_sequence_the_replay_ends_in_is_handed_on_for_live_output_to_end() {
        assert_eq!(replay("prompt\x1b[38;2;153", 8), b"prompt\x1b[38;2;153");
        assert_eq!(replay("prompt\x1b]11;", 8), b"prompt\x1b]11;");
        assert!(ReplayFilter::new().finish().is_empty());
    }
}
