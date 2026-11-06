//! `splitty review`: show a file, or a text from standard input, in a viewer
//! pane next to the caller's pane and wait until the user approves it or
//! sends a review in a window (`cli`, Review Command). The request waits as
//! long as the user takes; ending the command ends the wait.
use super::comments::{json_text, safe_text};
use super::{request_within, send_mux_op, CliError, ServerInfo};
use crate::client_session::viewer::{MAX_TEXT_BYTES, MAX_TITLE_CHARS};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;

const UPDATE: &str = "This Splitty server cannot wait for reviews yet. Update it (for a host: Update… in the Hosts panel) and reopen its terminal.";

/// A checked review: the op that shows the document, the caller, and how the
/// command names the document.
#[derive(Debug)]
pub struct Review {
    screen: String,
    pane: String,
    op: Value,
    named: String,
}

/// The pane that waits; a review needs one.
fn caller(pane: &str) -> Result<&str, CliError> {
    if pane.is_empty() {
        return Err(CliError::Usage(
            "Run this command in a Splitty terminal, or supply --pane <id> and --screen <id>"
                .into(),
        ));
    }
    Ok(pane)
}

/// The review of the file at `path`, shown as `splitty open` shows it.
pub fn file(path: &Path, screen: &str, pane: &str) -> Result<Review, CliError> {
    let pane = caller(pane)?;
    let request = super::viewer::prepare(path, screen, pane, false)?;
    Ok(Review {
        screen: request.screen,
        pane: pane.to_owned(),
        op: request.op,
        named: request.path,
    })
}

/// The review of `text` as a text document titled `title`, rendered as
/// Markdown when `markdown` is set, which replaces the text of the caller's
/// earlier document of that title.
pub fn text(
    text: String,
    title: &str,
    markdown: bool,
    screen: &str,
    pane: &str,
    cwd: Option<&Path>,
) -> Result<Review, CliError> {
    let pane = caller(pane)?;
    let chars = title.chars().count();
    if chars == 0 || chars > MAX_TITLE_CHARS || title.chars().any(char::is_control) {
        return Err(CliError::Usage(format!(
            "A title has 1 to {MAX_TITLE_CHARS} characters and no control characters"
        )));
    }
    let mut op = json!({
        "type": "open_viewer", "paneId": pane, "placement": "right",
        "source": {"kind": "text", "title": title, "text": text},
        "replace": true,
    });
    if markdown {
        op["source"]["format"] = json!("markdown");
    }
    if let Some(cwd) = cwd
        .and_then(Path::to_str)
        .filter(|cwd| cwd.starts_with('/'))
    {
        op["cwd"] = json!(cwd);
    }
    Ok(Review {
        screen: screen.to_owned(),
        pane: pane.to_owned(),
        op,
        named: title.to_owned(),
    })
}

/// The text of a review from `input`: at most 256 KiB of UTF-8.
pub fn read_text(input: impl Read) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(MAX_TEXT_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|e| CliError::Usage(format!("Cannot read standard input: {e}")))?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(CliError::Usage(
            "The text is larger than 256 KiB, the most a text document holds".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|_| CliError::Usage("The text must be valid UTF-8".into()))
}

/// The exit status of `splitty review` when the user requests changes
/// (`cli`, Review Command): 1 and 2 stay failures and usage errors.
pub const CHANGES_REQUESTED_STATUS: i32 = 3;

/// The user's verdict on the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approved,
    ChangesRequested,
}

/// What the command prints for an answer, and the verdict it exits with.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    verdict: Verdict,
    /// For standard output: the review, or the outcome as JSON.
    output: Option<String>,
}

/// Show the document, wait for the user's answer, print it and return the
/// verdict, with which the caller exits.
pub async fn run(server: &ServerInfo, review: &Review, json: bool) -> Result<Verdict, CliError> {
    let shown = send_mux_op(server, &review.screen, review.op.clone())
        .await
        .map_err(|error| match error {
            CliError::Api(400, message)
                if message.starts_with("invalid op:")
                    && message.contains("unknown variant `open_viewer`") =>
            {
                CliError::Failed(UPDATE.into())
            }
            CliError::Api(_, message) => CliError::Failed(message),
            other => other,
        })?;
    let Some(viewer) = shown["paneId"].as_str() else {
        return Err(CliError::Failed(UPDATE.into()));
    };
    // The pane's screen as the server found it.
    let screen = shown["screenId"]
        .as_str()
        .filter(|screen| !screen.is_empty())
        .unwrap_or(&review.screen);
    eprintln!(
        "Waiting for the review of {} in viewer pane {viewer}; interrupt to stop waiting.",
        safe_text(&review.named)
    );
    let mut wait = json!({"screenId": screen, "paneId": review.pane, "viewerPaneId": viewer});
    // The answer then judges the revision this command showed, not one that
    // another command put into the pane in between.
    if let Some(revision) = shown["viewerRevision"].as_u64() {
        wait["viewerRevision"] = json!(revision);
    }
    let answer = request_within(server, "POST", "/api/viewer/review/wait", Some(wait), None).await;
    let answer = outcome(answer, json)?;
    if let Some(output) = &answer.output {
        // Written and flushed here: the caller may exit right after.
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(output.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|e| CliError::Failed(format!("Cannot print the review: {e}")))?;
    }
    Ok(answer.verdict)
}

/// What the command prints for the answer to its wait, or why it fails: the
/// review of either verdict, nothing for an approval without one, or the
/// outcome as JSON.
fn outcome(answer: Result<(u16, Value), CliError>, json: bool) -> Result<Answer, CliError> {
    let (status, body) = answer.map_err(|error| match error {
        CliError::Request(cause) => CliError::Failed(format!(
            "The connection to the Splitty server ended before an answer: {cause}"
        )),
        other => other,
    })?;
    let (verdict, name) = match (status, body["outcome"].as_str(), body["code"].as_str()) {
        (200, Some("approved"), _) => (Verdict::Approved, "approved"),
        (200, Some("changes_requested"), _) => (Verdict::ChangesRequested, "changes_requested"),
        (410, _, Some("closed")) => {
            return Err(CliError::Failed(
                "The review was closed without an answer.".into(),
            ))
        }
        (409, _, Some("changed")) => return Err(CliError::Failed(
            "Another document took the viewer pane before the review began; nothing was reviewed."
                .into(),
        )),
        (404 | 405, _, None) => return Err(CliError::Failed(UPDATE.into())),
        _ => {
            return Err(CliError::Failed(
                body["error"].as_str().unwrap_or("unknown error").into(),
            ))
        }
    };
    let review = match body.get("review") {
        None => None,
        Some(Value::String(review)) => Some(review.as_str()),
        Some(_) => return Err(CliError::Failed("The answer holds no review".into())),
    };
    if verdict == Verdict::ChangesRequested && review.is_none() {
        return Err(CliError::Failed("The answer holds no review".into()));
    }
    let output = if json {
        let mut printed = json!({"outcome": name});
        if let Some(review) = review {
            printed["review"] = json!(review);
        }
        Some(format!("{}\n", json_text(&printed)))
    } else {
        review.map(|review| {
            let mut review = safe_text(review);
            if !review.ends_with('\n') {
                review.push('\n');
            }
            review
        })
    };
    Ok(Answer { verdict, output })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cli, Review Command (A plan reviewed, Approved, Approved with notes,
    /// Outcome as JSON, The pane closed): the review of either verdict on
    /// standard output, nothing for an approval without one, the outcome as
    /// JSON, the verdict for the exit status, and a failure with its reason
    /// for a closed pane, a second wait, a document replaced before the wait,
    /// a lost connection, an answer without its review and an old server.
    #[test]
    fn each_outcome_prints_its_answer_or_fails_with_its_reason() {
        let review = "# Review of Plan\n\nVerdict: changes requested\n\n## #1 Line 3, comment\n> step 4\n\nSplit it.\n";
        let changes = || {
            Ok((
                200,
                json!({"outcome": "changes_requested", "review": review}),
            ))
        };
        let approved = || Ok((200, json!({"outcome": "approved"})));
        let notes = || Ok((200, json!({"outcome": "approved", "review": review})));
        let answer = |verdict, output: Option<&str>| Answer {
            verdict,
            output: output.map(str::to_owned),
        };
        assert_eq!(
            outcome(changes(), false).expect("changes"),
            answer(Verdict::ChangesRequested, Some(review))
        );
        assert_eq!(
            outcome(approved(), false).expect("approved"),
            answer(Verdict::Approved, None)
        );
        assert_eq!(
            outcome(notes(), false).expect("notes"),
            answer(Verdict::Approved, Some(review))
        );
        assert_eq!(
            outcome(approved(), true).expect("JSON"),
            answer(Verdict::Approved, Some("{\"outcome\":\"approved\"}\n"))
        );
        for (answered, name, verdict) in [
            (changes(), "changes_requested", Verdict::ChangesRequested),
            (notes(), "approved", Verdict::Approved),
        ] {
            let printed = outcome(answered, true).expect("JSON");
            assert_eq!(printed.verdict, verdict);
            let printed = printed.output.expect("output");
            assert_eq!(
                serde_json::from_str::<Value>(&printed).expect("JSON"),
                json!({"outcome": name, "review": review})
            );
            assert!(printed.starts_with(&format!("{{\"outcome\":\"{name}\"")));
        }
        assert_eq!(
            outcome(
                Ok((
                    200,
                    json!({"outcome": "changes_requested", "review": "No line break\u{1b}[2J"})
                )),
                false
            )
            .expect("changes")
            .output
            .as_deref(),
            Some("No line break\u{fffd}[2J\n"),
            "a terminal acts on no character of the review"
        );

        for (answer, reason) in [
            (
                Ok((
                    410,
                    json!({"code": "closed", "error": "viewer pane v1 was closed before an answer"}),
                )),
                "closed without an answer",
            ),
            (
                Ok((
                    409,
                    json!({"code": "busy", "error": "a process already waits for a review of v1"}),
                )),
                "already waits",
            ),
            (
                Ok((
                    409,
                    json!({"code": "changed", "error": "viewer pane v1 shows another revision of its document than the one to review"}),
                )),
                "Another document took the viewer pane",
            ),
            (
                Ok((
                    429,
                    json!({"code": "busy", "error": "16 processes already wait for a review"}),
                )),
                "16 processes",
            ),
            (Ok((404, json!({"error": ""}))), "Update it"),
            (
                Err(CliError::Request(
                    "connection closed before message completed".into(),
                )),
                "connection to the Splitty server ended",
            ),
            (
                Ok((200, json!({"outcome": "changes_requested"}))),
                "no review",
            ),
            (
                Ok((200, json!({"outcome": "approved", "review": 7}))),
                "no review",
            ),
            (
                Ok((200, json!({"outcome": "feedback", "review": "x"}))),
                "unknown error",
            ),
        ] {
            match outcome(answer, false) {
                Err(CliError::Failed(message)) => assert!(message.contains(reason), "{message}"),
                other => panic!("not a failure ({reason}): {other:?}"),
            }
        }
    }

    /// cli, Review Command (The next revision of the plan, A plan rendered):
    /// a text goes with `replace`, its title, the Markdown format unless
    /// plain text is asked for, and the caller's pane; a file as
    /// `splitty open` shows it; both need the caller's pane.
    #[test]
    fn a_review_shows_its_document_next_to_the_caller() {
        let review = text(
            "1. Step one\n".into(),
            "Plan",
            true,
            "s1",
            "p1",
            Some(Path::new("/home/me/app")),
        )
        .expect("text");
        assert_eq!(review.screen, "s1");
        assert_eq!(review.pane, "p1");
        assert_eq!(review.named, "Plan");
        assert_eq!(
            review.op,
            json!({
                "type": "open_viewer", "paneId": "p1", "placement": "right",
                "source": {"kind": "text", "title": "Plan", "text": "1. Step one\n", "format": "markdown"},
                "replace": true, "cwd": "/home/me/app",
            })
        );
        let without = text(String::new(), "Review", false, "", "p1", None).expect("text");
        assert!(without.op.get("cwd").is_none());
        assert!(without.op["source"].get("format").is_none(), "plain text");
        for title in ["", "Plan\n", &"x".repeat(MAX_TITLE_CHARS + 1)] {
            assert!(
                matches!(
                    text(String::new(), title, true, "s1", "p1", None),
                    Err(CliError::Usage(_))
                ),
                "{title:?}"
            );
        }
        assert!(text(
            String::new(),
            &"ä".repeat(MAX_TITLE_CHARS),
            true,
            "s1",
            "p1",
            None
        )
        .is_ok());
        assert!(matches!(
            text(String::new(), "Plan", true, "s1", "", None),
            Err(CliError::Usage(message)) if message.contains("--pane")
        ));

        let dir = tempfile::tempdir().expect("fixture");
        let plan = dir.path().join("PLAN.md");
        std::fs::write(&plan, "# Plan").expect("file");
        let review = file(&plan, "s1", "p1").expect("file");
        assert_eq!(review.op["source"]["kind"], "file");
        assert_eq!(review.op["placement"], "right");
        assert_eq!(review.op["paneId"], "p1");
        assert!(review.op.get("replace").is_none());
        assert!(matches!(file(&plan, "s1", ""), Err(CliError::Usage(_))));
    }

    /// cli, Review Command: standard input holds at most 256 KiB of UTF-8.
    #[test]
    fn the_text_is_at_most_256_kib_of_utf8() {
        let at_limit = "x".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            read_text(at_limit.as_bytes()).expect("at the limit"),
            at_limit
        );
        assert_eq!(read_text(std::io::empty()).expect("empty"), "");
        assert!(matches!(
            read_text(format!("{at_limit}x").as_bytes()),
            Err(CliError::Usage(message)) if message.contains("256 KiB")
        ));
        assert!(matches!(read_text(&[0xff][..]), Err(CliError::Usage(_))));
    }
}
