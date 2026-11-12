//! Program status records: what programs in a session report about their own
//! state through the Program Status Protocol (OSC 7501, revision 0.4,
//! <https://www.superlogical.com/rex/docs/build/program-status>).
//!
//! A report is `OSC 7501 ; key=value:key=value`. [`StatusStore::apply`]
//! parses its payload, as the session's OSC scanner found it, and keeps the
//! records: a root record from reports without an `id`, child records by
//! `id`. A report replaces its record whole; `state=clear` removes a record
//! and the records beneath it, or every record without an `id`. Pairs without
//! `=` are skipped and unknown keys ignored. A report whose `state` is missing
//! or unknown, whose `id` is invalid, that breaks a limit, whose base64 does
//! not decode to UTF-8 or whose decoded text holds a control character is
//! dropped whole, before any record changes. Bidirectional controls,
//! invisible characters and line and paragraph separators are removed from
//! the texts kept.
//!
//! The session removes `working`, `blocked` and `idle` records at a prompt
//! mark, when the program in a shell ends and when the output ends
//! ([`StatusStore::end_program`]), and every record at a full reset. Only the
//! root record reaches clients; the children are kept for `clear` and the
//! record limit.

use std::collections::HashMap;

use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// The records a session holds at most, the root included.
pub const MAX_RECORDS: usize = 64;
/// A key's limit; a longer key is no key the protocol defines.
const KEY_MAX: usize = 16;
const TITLE_ENCODED_MAX: usize = 256;
const TITLE_DECODED_MAX: usize = 192;
const MSG_ENCODED_MAX: usize = 2732;
const MSG_DECODED_MAX: usize = 2048;
const APP_MAX: usize = 32;
const ID_MAX: usize = 128;
const ID_SEGMENT_MAX: usize = 32;
const ID_DEPTH: usize = 8;

/// Standard base64, with or without padding.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// A record's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl StatusState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }

    /// Whether the record ends with the program: at a prompt mark, when the
    /// shell is the foreground process again and when the output ends.
    pub fn ends_with_program(self) -> bool {
        matches!(self, Self::Idle | Self::Working | Self::Blocked)
    }
}

/// What a `blocked` program waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Permission,
    Question,
    Auth,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// A program status record, as `ProgramStatusRecord` in `docs/protocol.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramStatus {
    pub state: StatusState,
    /// Only with `blocked`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BlockKind>,
    /// 0 to 100, only with `working` and `blocked`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Decoded and sanitized plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Decoded and sanitized plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    /// Unix milliseconds of the report, server clock.
    pub updated_at: u64,
}

/// A session's root record as the session holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootStatus {
    /// Changes whenever the root record is reported, replaced or removed.
    pub revision: u64,
    pub record: Option<ProgramStatus>,
    /// Whether the last change of the root record came from output read
    /// again (a keeper's buffer at adoption), which notifies nobody.
    pub read_again: bool,
}

/// A report, parsed and validated.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    Set {
        id: Option<String>,
        record: ProgramStatus,
    },
    Clear {
        id: Option<String>,
    },
}

#[derive(Debug)]
struct Entry {
    record: ProgramStatus,
    /// When it was last reported, in the store's own order.
    reported: u64,
}

/// A session's program status records; see the module documentation.
#[derive(Debug, Default)]
pub struct StatusStore {
    root: Option<ProgramStatus>,
    children: HashMap<String, Entry>,
    /// Counts reports, for the least recently reported child.
    reports: u64,
    revision: u64,
    read_again: bool,
}

impl StatusStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply the payload of an OSC 7501 sequence (what follows `7501;`),
    /// reported at `now` (Unix milliseconds). `read_again` marks output read
    /// again. True when the root record changed.
    pub fn apply(&mut self, payload: &[u8], read_again: bool, now: u64) -> bool {
        let Some(report) = parse(payload, now) else {
            return false;
        };
        match report {
            Report::Clear { id: None } => {
                self.children.clear();
                self.set_root(None, read_again)
            }
            Report::Clear { id: Some(id) } => {
                let below = format!("{id}/");
                self.children
                    .retain(|child, _| *child != id && !child.starts_with(&below));
                false
            }
            Report::Set { id: None, record } => {
                if self.root.is_none() {
                    self.make_room();
                }
                self.set_root(Some(record), read_again)
            }
            Report::Set {
                id: Some(id),
                record,
            } => {
                self.reports += 1;
                if !self.children.contains_key(&id) {
                    self.make_room();
                }
                self.children.insert(
                    id,
                    Entry {
                        record,
                        reported: self.reports,
                    },
                );
                false
            }
        }
    }

    /// The program ended, or the shell marked its prompt: `working`,
    /// `blocked` and `idle` records go. True when the root record changed.
    pub fn end_program(&mut self) -> bool {
        self.children
            .retain(|_, entry| !entry.record.state.ends_with_program());
        match &self.root {
            Some(root) if root.state.ends_with_program() => self.set_root(None, false),
            _ => false,
        }
    }

    /// A full reset: every record goes. True when the root record changed.
    pub fn reset(&mut self) -> bool {
        self.children.clear();
        self.set_root(None, false)
    }

    pub fn root(&self) -> RootStatus {
        RootStatus {
            revision: self.revision,
            record: self.root.clone(),
            read_again: self.read_again,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The number of records, the root included.
    pub fn len(&self) -> usize {
        self.children.len() + usize::from(self.root.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a child record with `id` is held.
    pub fn has_child(&self, id: &str) -> bool {
        self.children.contains_key(id)
    }

    fn set_root(&mut self, record: Option<ProgramStatus>, read_again: bool) -> bool {
        if record.is_none() && self.root.is_none() {
            return false;
        }
        self.root = record;
        self.revision = self.revision.wrapping_add(1);
        self.read_again = read_again;
        true
    }

    /// Before a record is added: at the limit, the child reported least
    /// recently goes, never the root.
    fn make_room(&mut self) {
        if self.len() < MAX_RECORDS {
            return;
        }
        let oldest = self
            .children
            .iter()
            .min_by_key(|(_, entry)| entry.reported)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            self.children.remove(&id);
        }
    }
}

/// The fields of a report, the last of each key counting.
#[derive(Default)]
struct Fields<'a> {
    state: Option<&'a [u8]>,
    id: Option<&'a [u8]>,
    kind: Option<&'a [u8]>,
    progress: Option<&'a [u8]>,
    app: Option<&'a [u8]>,
    title: Option<&'a [u8]>,
    msg: Option<&'a [u8]>,
}

fn parse(payload: &[u8], now: u64) -> Option<Report> {
    let mut fields = Fields::default();
    for pair in payload.split(|&byte| byte == b':') {
        let Some(equals) = pair.iter().position(|&byte| byte == b'=') else {
            continue;
        };
        let key = pair[..equals].trim_ascii();
        let value = pair[equals + 1..].trim_ascii();
        if key.is_empty() || key.len() > KEY_MAX || !key.iter().all(u8::is_ascii_lowercase) {
            continue;
        }
        let slot = match key {
            b"state" => &mut fields.state,
            b"id" => &mut fields.id,
            b"kind" => &mut fields.kind,
            b"progress" => &mut fields.progress,
            b"app" => &mut fields.app,
            b"title" => &mut fields.title,
            b"msg" => &mut fields.msg,
            _ => continue,
        };
        *slot = Some(value);
    }
    let id = match fields.id {
        Some(id) => Some(valid_id(id)?),
        None => None,
    };
    // Every pair is checked before a record changes, also for `clear`.
    let title = text(fields.title, TITLE_ENCODED_MAX, TITLE_DECODED_MAX)?;
    let msg = text(fields.msg, MSG_ENCODED_MAX, MSG_DECODED_MAX)?;
    let state = match fields.state? {
        b"clear" => return Some(Report::Clear { id }),
        b"idle" => StatusState::Idle,
        b"working" => StatusState::Working,
        b"done" => StatusState::Done,
        b"blocked" => StatusState::Blocked,
        b"error" => StatusState::Error,
        _ => return None,
    };
    let kind = match (state, fields.kind) {
        (StatusState::Blocked, Some(b"permission")) => Some(BlockKind::Permission),
        (StatusState::Blocked, Some(b"question")) => Some(BlockKind::Question),
        (StatusState::Blocked, Some(b"auth")) => Some(BlockKind::Auth),
        _ => None,
    };
    let progress = match state {
        StatusState::Working | StatusState::Blocked => fields.progress.and_then(progress),
        _ => None,
    };
    let app = fields
        .app
        .filter(|app| (1..=APP_MAX).contains(&app.len()) && app.iter().all(|&b| id_byte(b)))
        .map(|app| String::from_utf8_lossy(app).into_owned());
    Some(Report::Set {
        id,
        record: ProgramStatus {
            state,
            kind,
            progress,
            app,
            title,
            msg,
            updated_at: now,
        },
    })
}

/// A character an `id` segment or `app` may hold.
fn id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

/// Segments of 1 to 32 such characters joined by `/`, at most 128 bytes and
/// 8 segments.
fn valid_id(id: &[u8]) -> Option<String> {
    if id.is_empty() || id.len() > ID_MAX {
        return None;
    }
    let mut depth = 0;
    for segment in id.split(|&byte| byte == b'/') {
        depth += 1;
        if segment.is_empty()
            || segment.len() > ID_SEGMENT_MAX
            || depth > ID_DEPTH
            || !segment.iter().all(|&b| id_byte(b))
        {
            return None;
        }
    }
    Some(String::from_utf8_lossy(id).into_owned())
}

/// An integer from 0 to 100.
fn progress(value: &[u8]) -> Option<u8> {
    if value.is_empty() || value.len() > 3 || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value)
        .ok()?
        .parse::<u8>()
        .ok()
        .filter(|progress| *progress <= 100)
}

/// A base64 text within its limits: `Some(None)` when absent or empty after
/// sanitizing, `None` when the report must be dropped.
fn text(value: Option<&[u8]>, encoded_max: usize, decoded_max: usize) -> Option<Option<String>> {
    let Some(value) = value else {
        return Some(None);
    };
    if value.len() > encoded_max {
        return None;
    }
    let decoded = BASE64.decode(value).ok()?;
    if decoded.len() > decoded_max {
        return None;
    }
    let text = String::from_utf8(decoded).ok()?;
    if text
        .chars()
        .any(|c| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}'))
    {
        return None;
    }
    let text: String = text.chars().filter(|&c| !hidden(c)).collect();
    Some((!text.is_empty()).then_some(text))
}

/// Bidirectional controls, invisible characters and line and paragraph
/// separators, removed from the texts kept.
fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200b}'
            | '\u{2060}'
            | '\u{feff}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

#[cfg(test)]
mod tests;
