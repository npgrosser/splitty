//! Finite state machine for extracting OSC 0/2 (window title) and OSC 133
//! (semantic prompts) escape sequences from a PTY output byte stream.
//! Supports BEL, ST, and 8-bit OSC introducers.

use std::time::Instant;

const OSC_MAX_PAYLOAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OscState {
    Idle,
    GotESC,
    GotBracket,
    /// Accumulating the OSC number digits (e.g. "0", "2", "133").
    InNumber,
    /// Collecting payload for OSC 0/2 title sequences.
    InTitlePayload,
    /// Collecting payload for OSC 133 semantic sequences.
    InSemanticPayload,
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

/// Extracts OSC 0/2 title sequences and OSC 133 semantic prompt sequences
/// from a PTY output stream.
///
/// Recognized sequences:
///   - `ESC ] 0 ; <payload> BEL`       -- set icon name + window title
///   - `ESC ] 2 ; <payload> BEL`       -- set window title
///   - `ESC ] 0 ; <payload> ESC \`     -- same, ST terminator
///   - `ESC ] 2 ; <payload> ESC \`     -- same, ST terminator
///   - `0x9D <digit> ; <payload> ...`  -- 8-bit OSC introducer (rare)
///   - `ESC ] 133 ; <action>[;params] BEL/ST` -- semantic prompts
pub struct OscScanner {
    state: OscState,
    /// Accumulated OSC number digits.
    num_buf: Vec<u8>,
    buf: Vec<u8>,
    title: String,
    change_at: Option<Instant>,
    /// Semantic actions collected during the current `feed()` call.
    semantic_actions: Vec<SemanticAction>,
    /// Number of standalone BEL characters (0x07) seen since last drain.
    bell_count: u32,
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            state: OscState::Idle,
            num_buf: Vec::with_capacity(4),
            buf: Vec::new(),
            title: String::new(),
            change_at: None,
            semantic_actions: Vec::new(),
            bell_count: 0,
        }
    }

    /// Process each byte through the FSM, updating the title when a
    /// complete OSC 0/2 sequence is found.
    /// Returns `true` if the title changed during this call.
    ///
    /// Any OSC 133 semantic actions are collected internally and can be
    /// retrieved via [`drain_semantic`].
    pub fn feed(&mut self, data: &[u8]) -> bool {
        let mut changed = false;
        for &b in data {
            match self.state {
                OscState::Idle => {
                    if b == 0x1b {
                        self.state = OscState::GotESC;
                    } else if b == 0x9d {
                        // 8-bit OSC introducer (equivalent to ESC ])
                        self.state = OscState::GotBracket;
                    } else if b == 0x07 {
                        // Standalone BEL (not an OSC terminator)
                        self.bell_count += 1;
                    }
                }
                OscState::GotESC => {
                    if b == b']' {
                        self.state = OscState::GotBracket;
                    } else {
                        self.state = OscState::Idle;
                    }
                }
                OscState::GotBracket => {
                    if b.is_ascii_digit() {
                        self.num_buf.clear();
                        self.num_buf.push(b);
                        self.state = OscState::InNumber;
                    } else {
                        self.state = OscState::Idle;
                    }
                }
                OscState::InNumber => {
                    if b == b';' {
                        // Number is complete — decide which payload state
                        self.buf.clear();
                        if self.num_buf == b"0" || self.num_buf == b"2" {
                            self.state = OscState::InTitlePayload;
                        } else if self.num_buf == b"133" {
                            self.state = OscState::InSemanticPayload;
                        } else {
                            // Unknown OSC number — skip
                            self.state = OscState::Idle;
                        }
                    } else if b.is_ascii_digit() && self.num_buf.len() < 4 {
                        self.num_buf.push(b);
                    } else {
                        // Not a digit and not ';' — abort
                        self.state = OscState::Idle;
                    }
                }
                OscState::InTitlePayload => {
                    if b == 0x07 {
                        // BEL terminator
                        self.title = String::from_utf8_lossy(&self.buf).into_owned();
                        self.change_at = Some(Instant::now());
                        self.state = OscState::Idle;
                        changed = true;
                    } else if b == 0x1b {
                        // Could be ST (ESC \) -- commit title immediately
                        // before transitioning to GotESC.
                        self.title = String::from_utf8_lossy(&self.buf).into_owned();
                        self.change_at = Some(Instant::now());
                        self.state = OscState::GotESC;
                        changed = true;
                    } else if self.buf.len() < OSC_MAX_PAYLOAD {
                        self.buf.push(b);
                    } else {
                        // Payload too large -- abort.
                        self.state = OscState::Idle;
                        self.buf.clear();
                    }
                }
                OscState::InSemanticPayload => {
                    if b == 0x07 {
                        // BEL terminator
                        if let Some(action) = parse_semantic_payload(&self.buf) {
                            self.semantic_actions.push(action);
                        }
                        self.state = OscState::Idle;
                    } else if b == 0x1b {
                        // Could be ST (ESC \) -- commit immediately
                        if let Some(action) = parse_semantic_payload(&self.buf) {
                            self.semantic_actions.push(action);
                        }
                        self.state = OscState::GotESC;
                    } else if self.buf.len() < OSC_MAX_PAYLOAD {
                        self.buf.push(b);
                    } else {
                        self.state = OscState::Idle;
                        self.buf.clear();
                    }
                }
            }
        }
        changed
    }

    /// Returns the last extracted title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the timestamp of the last title change.
    pub fn change_at(&self) -> Option<Instant> {
        self.change_at
    }

    /// Drain and return all semantic actions collected since the last drain.
    pub fn drain_semantic(&mut self) -> Vec<SemanticAction> {
        std::mem::take(&mut self.semantic_actions)
    }

    /// Drain and return the number of standalone BEL characters since the last drain.
    pub fn drain_bell(&mut self) -> u32 {
        std::mem::replace(&mut self.bell_count, 0)
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

    #[test]
    fn test_osc_8bit_introducer() {
        let mut s = OscScanner::new();
        s.feed(b"\x9d0;Title\x07");
        assert_eq!(s.title(), "Title");
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
        let actions = s.drain_semantic();
        assert_eq!(actions, vec![SemanticAction::PromptStart]);
    }

    #[test]
    fn unknown_osc_number_ignored() {
        let mut s = OscScanner::new();
        s.feed(b"\x1b]99;payload\x07");
        assert_eq!(s.title(), "");
        assert!(s.drain_semantic().is_empty());
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
