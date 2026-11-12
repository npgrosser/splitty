//! Unit: the report parser and the records (program-status, Status Reports,
//! Report Values, Report Limits, Status Text Sanitizing, Status Records).

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;

use super::*;
use crate::pty::osc::{OscFinding, OscScanner};

const NOW: u64 = 1_700_000_000_000;

/// A store fed the reports of `output` through the OSC scanner.
fn fed(output: &[u8]) -> StatusStore {
    let mut store = StatusStore::new();
    feed(&mut store, output);
    store
}

fn feed(store: &mut StatusStore, output: &[u8]) {
    let mut scanner = OscScanner::new();
    scanner.feed(output);
    for finding in scanner.drain() {
        if let OscFinding::Status(payload) = finding {
            store.apply(&payload, false, NOW);
        }
    }
}

fn report(payload: &str) -> Vec<u8> {
    format!("\x1b]7501;{payload}\x07").into_bytes()
}

fn root(store: &StatusStore) -> ProgramStatus {
    store.root().record.expect("a root record")
}

#[test]
fn a_report_sets_the_root_record() {
    let store = fed(&report("state=working:app=claude-code"));
    let record = root(&store);
    assert_eq!(record.state, StatusState::Working);
    assert_eq!(record.app.as_deref(), Some("claude-code"));
    assert_eq!(record.updated_at, NOW);
    assert!(!store.root().read_again);
}

#[test]
fn a_report_split_between_reads_is_read_once() {
    let data = b"\x1b]7501;state=blocked:kind=permission\x1b\\";
    for split in 0..=data.len() {
        let mut scanner = OscScanner::new();
        scanner.feed(&data[..split]);
        scanner.feed(&data[split..]);
        let reports: Vec<_> = scanner
            .drain()
            .into_iter()
            .filter(|finding| matches!(finding, OscFinding::Status(_)))
            .collect();
        assert_eq!(
            reports,
            vec![OscFinding::Status(
                b"state=blocked:kind=permission".to_vec()
            )],
            "split at {split}"
        );
    }
}

#[test]
fn a_report_with_an_unknown_or_missing_state_is_ignored() {
    let mut store = fed(&report("state=working"));
    let revision = store.revision();
    for ignored in ["state=thinking", "app=x", "state=", "state=Working", ""] {
        assert!(
            !store.apply(ignored.as_bytes(), false, NOW + 1),
            "{ignored}"
        );
        assert_eq!(root(&store).state, StatusState::Working);
        assert_eq!(store.revision(), revision);
    }
}

#[test]
fn unknown_keys_and_malformed_pairs_are_skipped() {
    let store = fed(b"\x1b]7501;state=done:color=red:junk\x1b\\");
    assert_eq!(root(&store).state, StatusState::Done);
    // Whitespace around keys and values, and the last of a repeated key.
    let store = fed(&report(
        " state = blocked :state=error: averyveryverylongkey=1",
    ));
    assert_eq!(root(&store).state, StatusState::Error);
}

#[test]
fn ids_follow_the_protocol() {
    for valid in [
        "a",
        "agent-1/sub_2/x.y+z",
        &"a".repeat(32),
        &["b"; 8].join("/"),
    ] {
        let mut store = StatusStore::new();
        store.apply(format!("state=working:id={valid}").as_bytes(), false, NOW);
        assert!(store.has_child(valid), "{valid}");
    }
    for invalid in [
        "a//b",
        "/a",
        "a/",
        "",
        "a b",
        "ä",
        &"a".repeat(33),
        &["b"; 9].join("/"),
        &["a".repeat(25).as_str(); 5].join("/"),
    ] {
        let mut store = StatusStore::new();
        store.apply(format!("state=working:id={invalid}").as_bytes(), false, NOW);
        assert!(store.is_empty(), "{invalid:?} is ignored");
    }
}

#[test]
fn kind_progress_and_app_count_only_where_they_apply() {
    let record = root(&fed(&report("state=working:kind=permission")));
    assert_eq!((record.state, record.kind), (StatusState::Working, None));
    let record = root(&fed(&report("state=working:progress=150")));
    assert_eq!(
        (record.state, record.progress),
        (StatusState::Working, None)
    );
    for (value, expected) in [("0", Some(0)), ("100", Some(100)), ("42", Some(42))] {
        let record = root(&fed(&report(&format!("state=blocked:progress={value}"))));
        assert_eq!(record.progress, expected, "{value}");
    }
    for value in ["-1", "+5", "4.5", "1000", "x"] {
        let record = root(&fed(&report(&format!("state=working:progress={value}"))));
        assert_eq!(record.progress, None, "{value}");
    }
    let record = root(&fed(&report("state=done:progress=50")));
    assert_eq!(record.progress, None, "only with working and blocked");
    let record = root(&fed(&report("state=blocked:kind=question")));
    assert_eq!(record.kind, Some(BlockKind::Question));
    let record = root(&fed(&report("state=blocked:kind=approval")));
    assert_eq!(record.kind, None);
    let record = root(&fed(&report(&format!("state=idle:app={}", "a".repeat(33)))));
    assert_eq!(record.app, None);
    let record = root(&fed(&report("state=idle:app=a/b")));
    assert_eq!(record.app, None);
}

#[test]
fn a_control_character_in_a_text_drops_the_report() {
    let mut store = fed(&report("state=working"));
    for text in [
        "line\nbreak",
        "esc \x1b[31m",
        "tab\t",
        "del\x7f",
        "c1 \u{9b}",
    ] {
        let encoded = STANDARD.encode(text);
        for key in ["msg", "title"] {
            assert!(!store.apply(
                format!("state=blocked:{key}={encoded}").as_bytes(),
                false,
                NOW
            ));
            assert_eq!(root(&store).state, StatusState::Working, "{text:?}");
        }
    }
    // A clear that breaks a rule is dropped as well.
    let encoded = STANDARD.encode("\x1b");
    assert!(!store.apply(format!("state=clear:msg={encoded}").as_bytes(), false, NOW));
    assert_eq!(root(&store).state, StatusState::Working);
}

#[test]
fn base64_is_read_with_and_without_padding() {
    for engine in [STANDARD, STANDARD_NO_PAD] {
        let encoded = engine.encode("Allow edit?");
        let record = root(&fed(&report(&format!("state=blocked:msg={encoded}"))));
        assert_eq!(record.msg.as_deref(), Some("Allow edit?"));
    }
    let mut store = fed(&report("state=working"));
    for invalid in ["not base64!", "QQ=x", "/w=="] {
        assert!(
            !store.apply(format!("state=done:msg={invalid}").as_bytes(), false, NOW),
            "{invalid}: invalid base64 or not UTF-8"
        );
    }
    assert_eq!(root(&store).state, StatusState::Working);
}

#[test]
fn text_limits_hold_at_their_exact_boundaries() {
    let accepted = |key: &str, encoded: &str| {
        let mut store = StatusStore::new();
        store.apply(format!("state=done:{key}={encoded}").as_bytes(), false, NOW)
    };
    // 192 bytes encode to exactly 256; 193 to 260.
    let title = STANDARD.encode("t".repeat(192));
    assert_eq!(title.len(), 256);
    assert!(accepted("title", &title));
    assert!(!accepted("title", &STANDARD.encode("t".repeat(193))));
    // 2048 bytes encode to exactly 2732 with padding, 2731 without.
    let msg = STANDARD.encode("m".repeat(2048));
    assert_eq!(msg.len(), 2732);
    assert!(accepted("msg", &msg));
    assert!(accepted("msg", &STANDARD_NO_PAD.encode("m".repeat(2048))));
    // 2049 bytes also encode to 2732, within the encoded limit, and are over
    // the decoded one.
    let over = STANDARD.encode("m".repeat(2049));
    assert_eq!(over.len(), 2732);
    assert!(!accepted("msg", &over));
    assert!(!accepted("msg", &format!("{msg}A")));
    // Decoded limits count bytes, not characters.
    assert!(accepted("title", &STANDARD.encode("ü".repeat(96))));
    assert!(!accepted("title", &STANDARD.encode("ü".repeat(97))));
}

#[test]
fn hidden_characters_are_removed_from_texts() {
    let encoded = STANDARD.encode("ok\u{202e}txt.exe");
    let record = root(&fed(&report(&format!("state=blocked:msg={encoded}"))));
    assert_eq!(record.msg.as_deref(), Some("oktxt.exe"));
    let all = "\u{061c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}\u{200b}\u{2060}\u{feff}\u{2028}\u{2029}";
    let encoded = STANDARD.encode(format!("a{all}b"));
    let record = root(&fed(&report(&format!("state=done:title={encoded}"))));
    assert_eq!(record.title.as_deref(), Some("ab"));
    let encoded = STANDARD.encode(all);
    let record = root(&fed(&report(&format!("state=done:title={encoded}:msg="))));
    assert_eq!((record.title, record.msg), (None, None), "empty is absent");
}

#[test]
fn a_report_replaces_its_record_whole() {
    let msg = STANDARD.encode("Allow edit?");
    let mut store = fed(&report(&format!(
        "state=blocked:kind=permission:msg={msg}:app=claude-code"
    )));
    let first = store.revision();
    feed(&mut store, &report("state=working"));
    let record = root(&store);
    assert_eq!(record.state, StatusState::Working);
    assert_eq!((record.kind, record.msg, record.app), (None, None, None));
    assert_ne!(store.revision(), first);
}

#[test]
fn clear_removes_a_subtree_or_everything() {
    let mut store = fed(&report("state=working"));
    for id in ["a", "a/b", "ab", "a/b/c"] {
        store.apply(format!("state=working:id={id}").as_bytes(), false, NOW);
    }
    let revision = store.revision();
    assert!(
        !store.apply(b"state=clear:id=a", false, NOW),
        "the root stays"
    );
    assert!(store.has_child("ab"));
    assert!(!store.has_child("a") && !store.has_child("a/b") && !store.has_child("a/b/c"));
    assert_eq!(store.len(), 2);
    assert_eq!(store.revision(), revision);

    assert!(store.apply(b"state=clear", false, NOW));
    assert!(store.is_empty());
    assert_eq!(store.root().record, None);
    assert!(
        !store.apply(b"state=clear", false, NOW),
        "nothing left to change"
    );
}

#[test]
fn the_least_recently_reported_child_goes_at_the_limit() {
    let mut store = fed(&report("state=working"));
    for child in 0..63 {
        store.apply(format!("state=working:id=c{child}").as_bytes(), false, NOW);
    }
    assert_eq!(store.len(), MAX_RECORDS);
    // Reported again, c0 is no longer the least recent.
    store.apply(b"state=done:id=c0", false, NOW);
    store.apply(b"state=working:id=new", false, NOW);
    assert_eq!(store.len(), MAX_RECORDS);
    assert!(store.has_child("new"));
    assert!(store.has_child("c0"));
    assert!(!store.has_child("c1"), "the least recently reported child");
    assert!(store.root().record.is_some(), "the root is never evicted");

    // A new root at the limit takes the place of a child.
    let mut store = StatusStore::new();
    for child in 0..64 {
        store.apply(format!("state=working:id=c{child}").as_bytes(), false, NOW);
    }
    assert!(store.apply(b"state=idle", false, NOW));
    assert_eq!(store.len(), MAX_RECORDS);
    assert!(!store.has_child("c0"));
}

#[test]
fn records_of_the_program_end_with_it_and_a_reset_ends_all() {
    let mut store = fed(&report("state=blocked"));
    store.apply(b"state=done:id=finished", false, NOW);
    store.apply(b"state=idle:id=resting", false, NOW);
    store.apply(b"state=error:id=failed", false, NOW);
    assert!(store.end_program());
    assert_eq!(store.root().record, None);
    assert!(store.has_child("finished") && store.has_child("failed"));
    assert!(!store.has_child("resting"));

    let mut store = fed(&report("state=done"));
    assert!(!store.end_program(), "done survives");
    assert_eq!(root(&store).state, StatusState::Done);
    assert!(store.reset());
    assert!(store.is_empty());
}

#[test]
fn the_root_remembers_whether_it_was_read_again() {
    let mut store = StatusStore::new();
    store.apply(b"state=blocked", true, NOW);
    assert!(store.root().read_again);
    store.apply(b"state=blocked", false, NOW + 5);
    let root = store.root();
    assert!(!root.read_again);
    assert_eq!(root.record.map(|record| record.updated_at), Some(NOW + 5));
}

#[test]
fn the_record_serializes_as_the_wire_shape() {
    let msg = STANDARD.encode("Allow edit?");
    let record = root(&fed(&report(&format!(
        "state=blocked:kind=permission:progress=10:app=claude-code:msg={msg}"
    ))));
    assert_eq!(
        serde_json::to_value(&record).expect("json"),
        serde_json::json!({
            "state": "blocked",
            "kind": "permission",
            "progress": 10,
            "app": "claude-code",
            "msg": "Allow edit?",
            "updatedAt": NOW,
        })
    );
    let idle = root(&fed(&report("state=idle")));
    assert_eq!(
        serde_json::to_value(&idle).expect("json"),
        serde_json::json!({"state": "idle", "updatedAt": NOW})
    );
}
