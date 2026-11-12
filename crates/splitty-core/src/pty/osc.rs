//! Finite state machine that reads a PTY output stream for the sequences the
//! session tracks: OSC 0/2 (window title), OSC 133 (semantic prompts), OSC 7501
//! (program status reports), the full reset `ESC c` and standalone bells.
//!
//! Every OSC is read to its end, interpreted or not: BEL or `ESC \` end it,
//! CAN and SUB cancel it, and an ESC followed by anything but `\` cancels it
//! and begins the next sequence. The BEL that ends an OSC is never a bell,
//! also when the scanner skips the OSC because it does not read its number or
//! because its payload is over the limit. Only the 7-bit introducer `ESC ]`
//! begins an OSC: in UTF-8 output the byte 0x9d is part of a character.

use std::time::Instant;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// The limit of a title or OSC 133 payload; a longer one is skipped.
const OSC_MAX_PAYLOAD: usize = 4096;

/// The limit of a whole OSC 7501 sequence, from `ESC ]` to its terminator
/// (Program Status Protocol, revision 0.4); a longer report is dropped.
pub const STATUS_MAX_SEQUENCE: usize = 4096;

/// The digits an OSC number may have; a longer number is skipped.
const NUMBER_MAX: usize = 8;

/// An OSC whose payload the scanner collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// OSC 0 and 2.
    Title,
    /// OSC 133.
    Semantic,
    /// OSC 7501.
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OscState {
    Idle,
    /// After an ESC outside a string.
    GotESC,
    /// After `ESC ]`, reading the OSC number.
    InNumber,
    /// Collecting the payload of an OSC the scanner reads.
    InPayload(Kind),
    /// Reading to the end of an OSC without collecting: an unknown number, or
    /// a payload over its limit.
    Skip,
    /// After an ESC inside an OSC, whose payload is collected (`Some`) or
    /// skipped: `\` ends the OSC, anything else cancels it.
    StringEsc(Option<Kind>),
}

/// Actions reported by OSC 133 semantic prompt sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticAction {
    /// `133;A` — shell is rendering the prompt.
    PromptStart,
    /// `133;B` — user is typing a command (input region).
    InputStart,
    /// `133;C` — command is executing, producing output.
    OutputStart,
    /// `133;D[;N]` — command finished with optional exit code N.
    CommandFinished { exit_code: Option<i32> },
}

/// What the scanner found in the output, in the order of the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscFinding {
    /// An OSC 0 or 2 set the title, now in [`OscScanner::title`].
    Title,
    /// An OSC 133 action.
    Semantic(SemanticAction),
    /// The payload of an OSC 7501 sequence within its size limit: everything
    /// between `7501;` and the terminator.
    Status(Vec<u8>),
    /// `ESC c`, the full reset.
    FullReset,
    /// A BEL that ends no OSC.
    Bell,
}

/// Reads a PTY output stream for titles, semantic prompts, status reports,
/// full resets and bells; see the module documentation.
///
/// Recognized sequences:
///   - `ESC ] 0 ; <payload> BEL`       -- set icon name + window title
///   - `ESC ] 2 ; <payload> BEL`       -- set window title
///   - `ESC ] 0 ; <payload> ESC \`     -- same, ST terminator
///   - `ESC ] 2 ; <payload> ESC \`     -- same, ST terminator
///   - `ESC ] 133 ; <action>[;params] BEL/ST` -- semantic prompts
///   - `ESC ] 7501 ; <pairs> BEL/ST`   -- program status reports
///   - `ESC c`                         -- full reset
pub struct OscScanner {
    state: OscState,
    /// Accumulated OSC number digits.
    num_buf: Vec<u8>,
    buf: Vec<u8>,
    /// The bytes of the current OSC so far, from its `ESC ]` on.
    length: usize,
    title: String,
    change_at: Option<Instant>,
    /// What the scanner found since the last drain, in byte order.
    findings: Vec<OscFinding>,
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            state: OscState::Idle,
            num_buf: Vec::with_capacity(NUMBER_MAX),
            buf: Vec::new(),
            length: 0,
            title: String::new(),
            change_at: None,
            findings: Vec::new(),
        }
    }

    /// Process each byte through the FSM, updating the title when a
    /// complete OSC 0/2 sequence is found.
    /// Returns `true` if the title changed during this call.
    ///
    /// Everything found is collected in byte order and can be retrieved via
    /// [`drain`](Self::drain), or by kind via [`drain_semantic`](Self::drain_semantic)
    /// and [`drain_bell`](Self::drain_bell).
    pub fn feed(&mut self, data: &[u8]) -> bool {
        let mut changed = false;
        for &b in data {
            changed |= self.step(b);
        }
        changed
    }

    /// One byte; true when it completed a title.
    fn step(&mut self, b: u8) -> bool {
        match self.state {
            OscState::Idle => {
                if b == ESC {
                    self.state = OscState::GotESC;
                } else if b == BEL {
                    self.findings.push(OscFinding::Bell);
                }
                false
            }
            OscState::GotESC => {
                self.escape(b);
                false
            }
            OscState::InNumber => {
                self.length += 1;
                match b {
                    b';' => {
                        self.buf.clear();
                        self.state = match self.num_buf.as_slice() {
                            b"0" | b"2" => OscState::InPayload(Kind::Title),
                            b"133" => OscState::InPayload(Kind::Semantic),
                            b"7501" => OscState::InPayload(Kind::Status),
                            _ => OscState::Skip,
                        };
                    }
                    b'0'..=b'9' if self.num_buf.len() < NUMBER_MAX => self.num_buf.push(b),
                    _ => return self.in_string(b, None),
                }
                false
            }
            OscState::InPayload(kind) => {
                self.length += 1;
                if matches!(b, BEL | ESC | CAN | SUB) {
                    return self.in_string(b, Some(kind));
                }
                let full = match kind {
                    // The shortest terminator, BEL, must still fit.
                    Kind::Status => self.length >= STATUS_MAX_SEQUENCE,
                    Kind::Title | Kind::Semantic => self.buf.len() >= OSC_MAX_PAYLOAD,
                };
                if full {
                    self.buf.clear();
                    self.state = OscState::Skip;
                } else {
                    self.buf.push(b);
                }
                false
            }
            OscState::Skip => self.in_string(b, None),
            OscState::StringEsc(kind) => {
                if b == b'\\' {
                    self.length += 1;
                    self.state = OscState::Idle;
                    kind.is_some_and(|kind| self.complete(kind))
                } else {
                    // The ESC cancelled the OSC and begins the next sequence.
                    self.buf.clear();
                    self.escape(b);
                    false
                }
            }
        }
    }

    /// A byte after an ESC outside a string.
    fn escape(&mut self, b: u8) {
        self.state = match b {
            b']' => {
                self.num_buf.clear();
                self.length = 2;
                OscState::InNumber
            }
            b'c' => {
                self.findings.push(OscFinding::FullReset);
                OscState::Idle
            }
            ESC => OscState::GotESC,
            // A BEL within an escape sequence still rings.
            BEL => {
                self.findings.push(OscFinding::Bell);
                OscState::GotESC
            }
            _ => OscState::Idle,
        };
    }

    /// A byte inside an OSC that is not collected: the terminators end the
    /// OSC, any other byte is skipped. True when it completed a title.
    fn in_string(&mut self, b: u8, kind: Option<Kind>) -> bool {
        match b {
            BEL => {
                self.state = OscState::Idle;
                kind.is_some_and(|kind| self.complete(kind))
            }
            ESC => {
                self.state = OscState::StringEsc(kind);
                false
            }
            CAN | SUB => {
                self.buf.clear();
                self.state = OscState::Idle;
                false
            }
            _ => {
                self.buf.clear();
                self.state = OscState::Skip;
                false
            }
        }
    }

    /// The OSC whose payload is in `buf` ended; `length` counts its whole
    /// sequence. True when it set the title.
    fn complete(&mut self, kind: Kind) -> bool {
        let payload = std::mem::take(&mut self.buf);
        match kind {
            Kind::Title => {
                self.title = String::from_utf8_lossy(&payload).into_owned();
                self.change_at = Some(Instant::now());
                self.findings.push(OscFinding::Title);
                true
            }
            Kind::Semantic => {
                if let Some(action) = parse_semantic_payload(&payload) {
                    self.findings.push(OscFinding::Semantic(action));
                }
                false
            }
            Kind::Status => {
                if self.length <= STATUS_MAX_SEQUENCE {
                    self.findings.push(OscFinding::Status(payload));
                }
                false
            }
        }
    }

    /// Returns the last extracted title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the timestamp of the last title change.
    pub fn change_at(&self) -> Option<Instant> {
        self.change_at
    }

    /// Drain and return everything found since the last drain, in byte order.
    pub fn drain(&mut self) -> Vec<OscFinding> {
        std::mem::take(&mut self.findings)
    }

    /// Drain and return the semantic actions found since the last drain,
    /// leaving the other findings.
    pub fn drain_semantic(&mut self) -> Vec<SemanticAction> {
        let mut actions = Vec::new();
        self.findings.retain(|finding| match finding {
            OscFinding::Semantic(action) => {
                actions.push(action.clone());
                false
            }
            _ => true,
        });
        actions
    }

    /// Drain and return the number of standalone BEL characters since the
    /// last drain, leaving the other findings.
    pub fn drain_bell(&mut self) -> u32 {
        let before = self.findings.len();
        self.findings
            .retain(|finding| !matches!(finding, OscFinding::Bell));
        u32::try_from(before - self.findings.len()).unwrap_or(u32::MAX)
    }
}

/// Parse the payload of an OSC 133 sequence into a `SemanticAction`.
///
/// The payload is everything after `133;`, e.g.:
///   - `A` → PromptStart
///   - `B` → InputStart
///   - `C` → OutputStart
///   - `D` → CommandFinished { exit_code: None }
///   - `D;0` → CommandFinished { exit_code: Some(0) }
///   - `D;127` → CommandFinished { exit_code: Some(127) }
fn parse_semantic_payload(payload: &[u8]) -> Option<SemanticAction> {
    if payload.is_empty() {
        return None;
    }

    let action_letter = payload[0];
    match action_letter {
        b'A' => Some(SemanticAction::PromptStart),
        b'B' => Some(SemanticAction::InputStart),
        b'C' => Some(SemanticAction::OutputStart),
        b'D' => {
            let exit_code = if payload.len() > 2 && payload[1] == b';' {
                std::str::from_utf8(&payload[2..])
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
            } else {
                None
            };
            Some(SemanticAction::CommandFinished { exit_code })
        }
        _ => None, // Unknown action letter — ignore
    }
}

impl Default for OscScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // OSC 0/2 title tests (existing functionality)
    // -----------------------------------------------------------------------

    #[test]
    fn test_osc_bel_terminator() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]0;My Title\x07");
        assert_eq!(s.title(), "My Title");
        assert!(s.change_at().is_some());
    }

    #[test]
    fn test_osc_st_terminator() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]0;My Title\x1b\\");
        assert_eq!(s.title(), "My Title");
    }

    /// program-status, OSC Sequences Read to Their End: 0x9d is part of a
    /// UTF-8 character and begins nothing, so the BEL after it is a bell.
    #[test]
    fn test_osc_8bit_introducer() {
        let mut s = OscScanner::new();
        s.feed(b"\x9d0;Title\x07");
        assert_eq!(s.title(), "");
        assert_eq!(s.drain(), vec![OscFinding::Bell]);
    }

    #[test]
    fn test_osc_type_2() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]2;Title\x07");
        assert_eq!(s.title(), "Title");
    }

    #[test]
    fn test_osc_max_payload() {
        let mut s = OscScanner::new();
        let mut data = Vec::new();
        data.extend_from_slice(b"\x1b]0;");
        data.extend(vec![b'A'; OSC_MAX_PAYLOAD + 1]);
        data.push(0x07);
        s.feed(&data);
        // Payload exceeded limit -- title should not be captured
        assert_eq!(s.title(), "");
    }

    #[test]
    fn test_osc_multiple_sequences() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]0;First\x07");
        assert_eq!(s.title(), "First");
        s.feed(b"\x1b]0;Second\x07");
        assert_eq!(s.title(), "Second");
    }

    #[test]
    fn test_osc_interleaved_data() {
        let mut s = OscScanner::new();
        s.feed(b"some normal text\x1b]0;My Title\x07more text");
        assert_eq!(s.title(), "My Title");
    }

    #[test]
    fn feed_returns_true_on_title_change() {
        let mut s = OscScanner::new();
        assert!(!s.feed(b"normal text"));
        assert!(s.feed(b"\x1b]0;My Title\x07"));
        // Same data again without a new sequence -> no change
        assert!(!s.feed(b"more text"));
        // New title -> change
        assert!(s.feed(b"\x1b]0;New Title\x07"));
    }

    // -----------------------------------------------------------------------
    // OSC 133 semantic prompt tests
    // -----------------------------------------------------------------------

    #[test]
    fn semantic_prompt_start_bel() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;A\x07");
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::PromptStart]);
    }

    #[test]
    fn semantic_input_start_bel() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;B\x07");
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::InputStart]);
    }

    #[test]
    fn semantic_output_start_bel() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;C\x07");
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::OutputStart]);
    }

    #[test]
    fn semantic_command_finished_no_exit_code() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;D\x07");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::CommandFinished { exit_code: None }]
        );
    }

    #[test]
    fn semantic_command_finished_with_exit_code_zero() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;D;0\x07");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::CommandFinished { exit_code: Some(0) }]
        );
    }

    #[test]
    fn semantic_command_finished_with_exit_code_nonzero() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;D;127\x07");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::CommandFinished {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn semantic_st_terminator() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;A\x1b\\");
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::PromptStart]);
    }

    #[test]
    fn semantic_command_finished_st_terminator() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;D;1\x1b\\");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::CommandFinished { exit_code: Some(1) }]
        );
    }

    #[test]
    fn semantic_multiple_actions_in_one_feed() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![
                SemanticAction::PromptStart,
                SemanticAction::InputStart,
                SemanticAction::OutputStart,
                SemanticAction::CommandFinished { exit_code: Some(0) },
            ]
        );
    }

    #[test]
    fn semantic_interleaved_with_title() {
        let mut s = OscScanner::new();
        let changed = s.feed(b"\x1b]133;A\x07\x1b]0;My Title\x07\x1b]133;B\x07");
        assert!(changed);
        assert_eq!(s.title(), "My Title");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::PromptStart, SemanticAction::InputStart]
        );
    }

    #[test]
    fn semantic_interleaved_with_normal_data() {
        let mut s = OscScanner::new();
        s.feed(b"hello world\x1b]133;C\x07some output here");
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::OutputStart]);
    }

    #[test]
    fn semantic_drain_clears_actions() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;A\x07");
        let actions = s.drain_semantic();
        assert_eq!(actions.len(), 1);
        // Second drain should be empty
        let actions2 = s.drain_semantic();
        assert!(actions2.is_empty());
    }

    #[test]
    fn semantic_unknown_action_ignored() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;X\x07");
        let actions = s.drain_semantic();
        assert!(actions.is_empty());
    }

    #[test]
    fn semantic_8bit_introducer() {
        let mut s = OscScanner::new();
        s.feed(b"\x9d133;A\x07");
        assert!(s.drain_semantic().is_empty(), "0x9d begins nothing");
        assert_eq!(s.drain_bell(), 1);
    }

    #[test]
    fn unknown_osc_number_ignored() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]99;payload\x07");
        assert_eq!(s.title(), "");
        assert!(s.drain_semantic().is_empty());
        assert_eq!(s.drain_bell(), 0, "the BEL ends the OSC");
    }

    // -----------------------------------------------------------------------
    // OSC read to its end (program-status, OSC Sequences Read to Their End)
    // -----------------------------------------------------------------------

    /// What `data` gives fed whole, asserted to be the same fed in two parts
    /// split at every byte.
    fn findings(data: &[u8]) -> Vec<OscFinding> {
        let mut whole = OscScanner::new();
        whole.feed(data);
        let expected = whole.drain();
        for split in 0..=data.len() {
            let mut s = OscScanner::new();
            s.feed(&data[..split]);
            s.feed(&data[split..]);
            assert_eq!(s.drain(), expected, "{data:?} split at {split}");
        }
        expected
    }

    #[test]
    fn the_bel_that_ends_an_osc_is_never_a_bell() {
        let long_title = [
            b"\x1b]2;".as_slice(),
            &[b'x'; OSC_MAX_PAYLOAD + 10],
            b"\x07",
        ]
        .concat();
        let long_number = b"\x1b]123456789012;x\x07".to_vec();
        for data in [
            b"\x1b]21337;status=working\x07".to_vec(),
            b"\x1b]9;done\x07".to_vec(),
            b"\x1b]777;notify;title;body\x07".to_vec(),
            b"\x1b]104\x07".to_vec(),
            b"\x1b];x\x07".to_vec(),
            b"\x1b]L-label\x07".to_vec(),
            long_number,
            long_title,
        ] {
            assert_eq!(
                findings(&data),
                vec![],
                "{:?}",
                String::from_utf8_lossy(&data)
            );
        }
    }

    #[test]
    fn an_oversized_status_report_is_dropped_and_its_bel_is_no_bell() {
        let report = [
            b"\x1b]7501;state=working:msg=".as_slice(),
            &[b'A'; 5000],
            b"\x07",
        ]
        .concat();
        assert_eq!(findings(&report), vec![]);
    }

    #[test]
    fn a_status_report_is_found_up_to_its_size_limit() {
        // `ESC ] 7501 ;` is 7 bytes; the terminator is BEL or `ESC \`.
        for (terminator, fits) in [(&b"\x07"[..], 4088), (b"\x1b\\", 4087)] {
            for (payload, found) in [(fits, true), (fits + 1, false)] {
                let body = vec![b'x'; payload];
                let data = [b"\x1b]7501;".as_slice(), &body, terminator].concat();
                assert_eq!(data.len(), payload + 7 + terminator.len());
                let mut s = OscScanner::new();
                s.feed(&data);
                let expected = if found {
                    vec![OscFinding::Status(body)]
                } else {
                    vec![]
                };
                assert_eq!(s.drain(), expected, "{} bytes", data.len());
            }
        }
    }

    #[test]
    fn a_bel_after_a_hyperlink_is_a_bell() {
        assert_eq!(
            findings(b"\x1b]8;;https://example.com\x07\x07"),
            vec![OscFinding::Bell]
        );
    }

    #[test]
    fn can_and_sub_cancel_an_osc_and_a_bel_after_it_rings() {
        for cancel in [0x18, 0x1a] {
            let data = [b"\x1b]2;never".as_slice(), &[cancel], b"\x07"].concat();
            assert_eq!(findings(&data), vec![OscFinding::Bell]);
        }
        assert_eq!(findings(b"\x1b]21337;x\x18\x07"), vec![OscFinding::Bell]);
    }

    #[test]
    fn utf8_text_before_a_bell_is_text() {
        assert_eq!(findings("Ý\x07".as_bytes()), vec![OscFinding::Bell]);
        assert_eq!(findings(b"\xc3\x9d\x07"), vec![OscFinding::Bell]);
    }

    #[test]
    fn an_esc_that_is_not_st_cancels_the_osc_and_begins_the_next_sequence() {
        // The title never ends; the OSC 133 after it counts.
        assert_eq!(
            findings(b"\x1b]2;title\x1b]133;A\x07"),
            vec![OscFinding::Semantic(SemanticAction::PromptStart)]
        );
        let mut s = OscScanner::new();
        s.feed(b"\x1b]2;title\x1b[0m");
        assert_eq!(s.title(), "", "a cancelled title is not set");
        // An ESC c inside a skipped OSC is a full reset.
        assert_eq!(
            findings(b"\x1b]21337;x\x1bc\x07"),
            vec![OscFinding::FullReset, OscFinding::Bell]
        );
    }

    #[test]
    fn a_full_reset_is_found_outside_strings() {
        assert_eq!(findings(b"text\x1bc"), vec![OscFinding::FullReset]);
        assert_eq!(findings(b"\x1b\x1bc"), vec![OscFinding::FullReset]);
        // `ESC ( c` designates no reset, and a soft reset is none.
        assert_eq!(findings(b"\x1b(c\x1b[!p"), vec![]);
    }

    #[test]
    fn findings_come_in_byte_order() {
        assert_eq!(
            findings(b"\x1b]133;A\x07\x1b]7501;state=working\x07\x07\x1b]2;t\x1b\\\x1bc"),
            vec![
                OscFinding::Semantic(SemanticAction::PromptStart),
                OscFinding::Status(b"state=working".to_vec()),
                OscFinding::Bell,
                OscFinding::Title,
                OscFinding::FullReset,
            ]
        );
    }

    #[test]
    fn semantic_negative_exit_code() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]133;D;-1\x07");
        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![SemanticAction::CommandFinished {
                exit_code: Some(-1)
            }]
        );
    }

    // -----------------------------------------------------------------------
    // BEL detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn standalone_bel_detected() {
        let mut s = OscScanner::new();
        s.feed(b"\x07");
        assert_eq!(s.drain_bell(), 1);
    }

    #[test]
    fn multiple_bels_counted() {
        let mut s = OscScanner::new();
        s.feed(b"\x07\x07\x07");
        assert_eq!(s.drain_bell(), 3);
    }

    #[test]
    fn bel_drain_clears_count() {
        let mut s = OscScanner::new();
        s.feed(b"\x07\x07");
        assert_eq!(s.drain_bell(), 2);
        assert_eq!(s.drain_bell(), 0);
    }

    #[test]
    fn osc_terminator_bel_not_counted_as_standalone() {
        let mut s = OscScanner::new();
        // The BEL here terminates the OSC 0 sequence — should NOT count as standalone
        s.feed(b"\x1b]0;Title\x07");
        assert_eq!(s.drain_bell(), 0);
        assert_eq!(s.title(), "Title");
    }

    #[test]
    fn standalone_bel_interleaved_with_osc() {
        let mut s = OscScanner::new();
        s.feed(b"\x07\x1b]0;Title\x07\x07");
        assert_eq!(s.drain_bell(), 2); // first and last, not the OSC terminator
        assert_eq!(s.title(), "Title");
    }

    #[test]
    fn bel_interleaved_with_semantic() {
        let mut s = OscScanner::new();
        s.feed(b"\x07\x1b]133;A\x07");
        assert_eq!(s.drain_bell(), 1); // only the standalone one
        assert_eq!(s.drain_semantic(), vec![SemanticAction::PromptStart]);
    }

    #[test]
    fn semantic_full_cycle() {
        let mut s = OscScanner::new();
        // Simulate a full shell cycle: prompt → input → output → finished
        s.feed(b"\x1b]133;A\x07");
        s.feed(b"$ ");
        s.feed(b"\x1b]133;B\x07");
        s.feed(b"ls -la\n");
        s.feed(b"\x1b]133;C\x07");
        s.feed(b"total 42\ndrwxr-xr-x  2 user user 4096 ...\n");
        s.feed(b"\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07");

        let actions = s.drain_semantic();
        assert_eq!(
            actions,
            vec![
                SemanticAction::PromptStart,
                SemanticAction::InputStart,
                SemanticAction::OutputStart,
                SemanticAction::CommandFinished { exit_code: Some(0) },
                SemanticAction::PromptStart,
            ]
        );
    }
}
