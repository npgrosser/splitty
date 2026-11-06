//! Waits for a review (`viewer-annotations`, Waiting For A Review): the held
//! `POST /api/viewer/review/wait` requests and the answers windows give them
//! with `viewer.review_answer` (`mux-protocol`, Review Answers).
//!
//! The registry lives in memory only. A waiting request registers here, then
//! names its wait in the viewer record with a trusted operation and ends it
//! with another one when it returns or is dropped. The first answer that
//! names a registered wait and its pane takes the wait out of the registry
//! and settles it; every later answer finds nothing.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use splitty_core::ids::{hex_id, PaneId};
use tokio::sync::oneshot;

/// Waits held at once on one server.
pub const MAX_WAITS: usize = 16;
/// The largest review an answer carries, in bytes of UTF-8.
pub const MAX_REVIEW_BYTES: usize = 1024 * 1024;
/// The longest pane or wait ID an acknowledgement echoes.
const MAX_ECHOED_ID: usize = 128;

/// The user's verdict, with the review as the window built it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Approved; with notes when the window sent a review along.
    Approved(Option<String>),
    /// Changes requested; the review may hold no annotation.
    ChangesRequested(String),
}

/// Why a wait was not registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// [`MAX_WAITS`] waits are held.
    Full,
    /// The viewer pane already has a waiting process.
    PaneBusy,
}

struct Entry {
    pane_id: PaneId,
    answer: oneshot::Sender<Outcome>,
}

/// The waits of one server, by wait ID.
#[derive(Default)]
pub struct Reviews {
    waits: Mutex<HashMap<String, Entry>>,
}

/// A registered wait; dropping it takes the wait out of the registry.
pub struct Registration {
    reviews: Arc<Reviews>,
    wait_id: String,
    answer: Option<oneshot::Receiver<Outcome>>,
}

impl Registration {
    /// The wait's ID: `wait-` and 16 lowercase hex characters.
    pub fn wait_id(&self) -> &str {
        &self.wait_id
    }

    /// The answer, once a window settles the wait; `None` when the wait left
    /// the registry without one, which only a second call can see. The
    /// future borrows nothing, so the registration can move on while it runs.
    pub fn answer(
        &mut self,
    ) -> impl std::future::Future<Output = Option<Outcome>> + Send + 'static {
        let answer = self.answer.take();
        async move { answer?.await.ok() }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.reviews.lock().remove(&self.wait_id);
    }
}

impl Reviews {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // The map stays consistent under a panic: every change is one call.
        self.waits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register a wait for a review of `pane_id`.
    pub fn register(self: &Arc<Self>, pane_id: PaneId) -> Result<Registration, Refusal> {
        let mut waits = self.lock();
        if waits.values().any(|entry| entry.pane_id == pane_id) {
            return Err(Refusal::PaneBusy);
        }
        if waits.len() >= MAX_WAITS {
            return Err(Refusal::Full);
        }
        let wait_id = loop {
            let id = hex_id("wait");
            if !waits.contains_key(&id) {
                break id;
            }
        };
        let (answer, answered) = oneshot::channel();
        waits.insert(wait_id.clone(), Entry { pane_id, answer });
        Ok(Registration {
            reviews: self.clone(),
            wait_id,
            answer: Some(answered),
        })
    }

    /// The number of waits held.
    pub fn held(&self) -> usize {
        self.lock().len()
    }

    /// Settle the wait `wait_id` of viewer pane `pane_id` with `outcome`:
    /// true when this answer settled it, false for a wait that has ended, is
    /// unknown or belongs to another pane.
    pub fn settle(&self, pane_id: &PaneId, wait_id: &str, outcome: Outcome) -> bool {
        let mut waits = self.lock();
        if waits
            .get(wait_id)
            .is_none_or(|entry| &entry.pane_id != pane_id)
        {
            return false;
        }
        let Some(entry) = waits.remove(wait_id) else {
            return false;
        };
        drop(waits);
        // Fails only when the waiting request is gone: nobody got the answer.
        entry.answer.send(outcome).is_ok()
    }

    /// Answer the message `viewer.review_answer` in `value`: the
    /// acknowledgement for its sender, `accepted` only when the answer
    /// settled a wait. An answer that breaks the rules settles nothing.
    pub fn answer(&self, value: &Value) -> Answered {
        let echo = |field: &str| {
            value[field]
                .as_str()
                .filter(|id| id.len() <= MAX_ECHOED_ID)
                .unwrap_or_default()
                .to_owned()
        };
        let accepted = parse(value).is_some_and(|(pane_id, wait_id, outcome)| {
            self.settle(&PaneId::from(pane_id), wait_id, outcome)
        });
        Answered {
            kind: "viewer.review_answered",
            pane_id: echo("paneId"),
            wait_id: echo("waitId"),
            accepted,
        }
    }
}

/// The pane, the wait and the outcome of a well-formed answer.
fn parse(value: &Value) -> Option<(&str, &str, Outcome)> {
    value["screenId"].as_str()?;
    let pane_id = value["paneId"].as_str()?;
    let wait_id = value["waitId"].as_str()?;
    let review = match value.get("review") {
        None => None,
        Some(Value::String(review)) if !review.is_empty() && review.len() <= MAX_REVIEW_BYTES => {
            Some(review.clone())
        }
        Some(_) => return None,
    };
    let outcome = match (value["outcome"].as_str()?, review) {
        ("approved", review) => Outcome::Approved(review),
        ("changes_requested", Some(review)) => Outcome::ChangesRequested(review),
        _ => return None,
    };
    Some((pane_id, wait_id, outcome))
}

/// `viewer.review_answered`, to the sender of one answer only.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    #[serde(rename = "type")]
    kind: &'static str,
    pub pane_id: String,
    pub wait_id: String,
    pub accepted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(pane: &str, wait: &str, outcome: Value) -> Value {
        let mut answer = json!({
            "type": "viewer.review_answer", "screenId": "s1", "paneId": pane, "waitId": wait,
        });
        if let (Some(answer), Some(outcome)) = (answer.as_object_mut(), outcome.as_object()) {
            answer.extend(outcome.clone());
        }
        answer
    }

    /// mux-protocol, Review Answers (The first answer): the first answer
    /// naming the wait and its pane settles it, a second finds nothing; a
    /// wait is held once per pane and 16 times per server, and leaves with
    /// its registration.
    #[tokio::test]
    async fn the_first_answer_settles_a_wait_once_per_pane() {
        let reviews = Arc::new(Reviews::default());
        let mut first = reviews.register(PaneId::from("v1")).expect("first wait");
        let id = first.wait_id().to_owned();
        assert!(id.starts_with("wait-") && id.len() == 21, "{id}");
        assert!(matches!(
            reviews.register(PaneId::from("v1")),
            Err(Refusal::PaneBusy)
        ));

        let changes = json!({"outcome": "changes_requested", "review": "# Review of PLAN.md"});
        let answered = reviews.answer(&answer("v2", &id, changes.clone()));
        assert!(!answered.accepted, "another pane");
        let answered = reviews.answer(&answer("v1", &id, changes.clone()));
        assert!(answered.accepted);
        assert_eq!(
            serde_json::to_value(&answered).expect("wire"),
            json!({"type": "viewer.review_answered", "paneId": "v1", "waitId": id, "accepted": true})
        );
        let late = reviews.answer(&answer("v1", &id, json!({"outcome": "approved"})));
        assert!(!late.accepted, "settled once");
        assert_eq!(
            first.answer().await,
            Some(Outcome::ChangesRequested("# Review of PLAN.md".into()))
        );
        assert_eq!(reviews.held(), 0);
        drop(first);

        // mux-protocol, Review Answers (An approval with notes): an approval
        // settles with and without a review.
        for (outcome, expected) in [
            (json!({"outcome": "approved"}), Outcome::Approved(None)),
            (
                json!({"outcome": "approved", "review": "# Review of Plan"}),
                Outcome::Approved(Some("# Review of Plan".into())),
            ),
        ] {
            let mut wait = reviews.register(PaneId::from("v1")).expect("wait");
            let id = wait.wait_id().to_owned();
            assert!(reviews.answer(&answer("v1", &id, outcome)).accepted);
            assert_eq!(wait.answer().await, Some(expected));
        }

        let held: Vec<_> = (0..MAX_WAITS)
            .map(|n| {
                reviews
                    .register(PaneId::from(format!("v{n}")))
                    .expect("held")
            })
            .collect();
        assert!(matches!(
            reviews.register(PaneId::from("v99")),
            Err(Refusal::Full)
        ));
        drop(held);
        assert_eq!(reviews.held(), 0, "a dropped registration leaves");
        assert!(reviews.register(PaneId::from("v99")).is_ok());
    }

    /// mux-protocol, Review Answers (A stale answer, A request for changes
    /// without a review) and the rules of an answer: an ended wait, a
    /// malformed answer, a request for changes without a review, a review
    /// that is empty, over 1 MiB or no text, and the unreleased outcome
    /// `feedback` settle nothing, and the acknowledgement echoes what it can.
    #[tokio::test]
    async fn stale_and_malformed_answers_settle_nothing() {
        let reviews = Arc::new(Reviews::default());
        let registration = reviews.register(PaneId::from("v1")).expect("wait");
        let id = registration.wait_id().to_owned();
        for (outcome, case) in [
            (
                json!({"outcome": "changes_requested"}),
                "a request for changes without a review",
            ),
            (
                json!({"outcome": "changes_requested", "review": ""}),
                "an empty review",
            ),
            (
                json!({"outcome": "approved", "review": ""}),
                "an empty review with an approval",
            ),
            (
                json!({"outcome": "changes_requested", "review": "x".repeat(MAX_REVIEW_BYTES + 1)}),
                "a review over 1 MiB",
            ),
            (
                json!({"outcome": "approved", "review": 7}),
                "a review that is no text",
            ),
            (
                json!({"outcome": "feedback", "review": "x"}),
                "the unreleased outcome feedback",
            ),
            (json!({"outcome": "later"}), "an unknown outcome"),
            (json!({}), "no outcome"),
        ] {
            assert!(
                !reviews.answer(&answer("v1", &id, outcome)).accepted,
                "{case}"
            );
        }
        let mut no_screen = answer("v1", &id, json!({"outcome": "approved"}));
        no_screen["screenId"] = Value::Null;
        assert!(!reviews.answer(&no_screen).accepted);
        let answered = reviews.answer(&json!({
            "type": "viewer.review_answer", "paneId": 3, "waitId": "w".repeat(129),
        }));
        assert_eq!(
            (answered.pane_id.as_str(), answered.wait_id.as_str()),
            ("", "")
        );
        assert_eq!(reviews.held(), 1, "the wait still waits");

        let at_limit =
            json!({"outcome": "changes_requested", "review": "x".repeat(MAX_REVIEW_BYTES)});
        assert!(reviews.answer(&answer("v1", &id, at_limit)).accepted);

        // The waiting request is gone: its wait has ended.
        let registration = reviews.register(PaneId::from("v1")).expect("wait");
        let id = registration.wait_id().to_owned();
        drop(registration);
        assert!(
            !reviews
                .answer(&answer("v1", &id, json!({"outcome": "approved"})))
                .accepted
        );
    }
}
