//! Unit: which changes of a root record are marked `notify` (program-status,
//! Status Notifications), on a test clock.

use std::time::{Duration, Instant};

use splitty_core::pty::status::{BlockKind, ProgramStatus, StatusState};

use super::{Tracker, NOTIFY_INTERVAL};

fn record(state: StatusState) -> ProgramStatus {
    ProgramStatus {
        state,
        kind: None,
        progress: None,
        app: Some("claude-code".into()),
        title: None,
        msg: None,
        updated_at: 0,
    }
}

fn blocked(kind: BlockKind, msg: &str) -> ProgramStatus {
    ProgramStatus {
        kind: Some(kind),
        msg: Some(msg.into()),
        ..record(StatusState::Blocked)
    }
}

/// A tracker and a clock that only moves when told.
struct Clock {
    tracker: Tracker,
    start: Instant,
    now: Duration,
}

impl Clock {
    fn new() -> Self {
        Self {
            tracker: Tracker::default(),
            start: Instant::now(),
            now: Duration::ZERO,
        }
    }

    fn at(&mut self, seconds: f64) -> &mut Self {
        self.now = Duration::from_secs_f64(seconds);
        self
    }

    fn report(&mut self, record: Option<ProgramStatus>) -> bool {
        self.tracker
            .change(record.as_ref(), false, self.start + self.now)
    }
}

#[test]
fn a_permission_request_is_marked() {
    let mut clock = Clock::new();
    assert!(!clock.report(Some(record(StatusState::Working))));
    assert!(clock
        .at(1.0)
        .report(Some(blocked(BlockKind::Permission, "Allow edit?"))));
}

#[test]
fn flapping_marks_only_the_first_block_in_ten_seconds() {
    let mut clock = Clock::new();
    let mut marked = Vec::new();
    for second in 0..10 {
        let state = if second % 2 == 0 {
            record(StatusState::Working)
        } else {
            blocked(BlockKind::Question, "Continue?")
        };
        if clock.at(f64::from(second)).report(Some(state)) {
            marked.push(second);
        }
    }
    assert_eq!(marked, vec![1], "only the first change to blocked");
    assert!(!clock.at(10.5).report(Some(record(StatusState::Working))));
    assert!(
        clock
            .at(1.0 + NOTIFY_INTERVAL.as_secs_f64())
            .report(Some(blocked(BlockKind::Question, "Continue?"))),
        "ten seconds after the last mark"
    );
}

#[test]
fn a_change_only_in_progress_is_not_marked() {
    let mut clock = Clock::new();
    let mut first = blocked(BlockKind::Permission, "Allow edit?");
    first.progress = Some(10);
    assert!(clock.report(Some(first.clone())));
    let mut second = first;
    second.progress = Some(60);
    second.updated_at = 5;
    assert!(!clock.at(20.0).report(Some(second)));
}

#[test]
fn a_new_kind_or_message_while_blocked_is_marked_once_the_interval_passed() {
    let mut clock = Clock::new();
    assert!(clock.report(Some(blocked(BlockKind::Permission, "Allow edit?"))));
    assert!(
        !clock
            .at(1.0)
            .report(Some(blocked(BlockKind::Question, "Allow edit?"))),
        "within ten seconds of the last mark"
    );
    assert!(clock
        .at(11.0)
        .report(Some(blocked(BlockKind::Auth, "Allow edit?"))));
    assert!(clock
        .at(22.0)
        .report(Some(blocked(BlockKind::Auth, "Sign in again"))));
}

#[test]
fn done_and_error_after_a_marked_block_are_marked() {
    let mut clock = Clock::new();
    assert!(clock.report(Some(blocked(BlockKind::Permission, "Allow edit?"))));
    assert!(!clock.at(1.0).report(Some(record(StatusState::Working))));
    assert!(clock.at(2.0).report(Some(record(StatusState::Done))));
    assert!(clock.at(3.0).report(Some(record(StatusState::Error))));
    assert!(!clock.at(4.0).report(None), "a removal is never marked");
    assert!(
        !clock.at(5.0).report(Some(record(StatusState::Idle))),
        "idle is never marked"
    );
}

#[test]
fn changes_read_again_are_not_marked() {
    let mut tracker = Tracker::default();
    let now = Instant::now();
    let block = blocked(BlockKind::Permission, "Allow edit?");
    assert!(!tracker.change(Some(&block), true, now));
    // Read again, it counts as sent: the same record from live output
    // differs in nothing.
    assert!(!tracker.change(Some(&block), false, now));
    assert!(tracker.change(Some(&record(StatusState::Done)), false, now));
}
