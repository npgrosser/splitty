//! `splitty review`: show a file, or a text from standard input, in a viewer
//! pane next to the caller's pane and wait for the user's verdict in a window
//! (`cli`, Review Command). The request waits as long as the user takes;
//! ending the command ends the wait. With `--hook claude-code` the command is
//! Claude Code's hook for the approval of a plan (`cli`, Review Hook For
//! Claude Code): it reviews the plan of the hook's event and prints Claude
//! Code's decision, or nothing when it cannot review.
use super::comments::{json_text, safe_text};
use super::{request_within, send_mux_op, CliError, ServerInfo};
use crate::client_session::viewer::{MAX_TEXT_BYTES, MAX_TITLE_CHARS};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
    let wait = show(server, review).await?;
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

/// Show the plan of Claude Code's `request`, wait for the user's verdict and
/// return Claude Code's decision for it.
pub async fn run_hook(
    server: &ServerInfo,
    review: &Review,
    request: &PlanRequest,
) -> Result<String, CliError> {
    let wait = show(server, review).await?;
    let answer = request_within(server, "POST", "/api/viewer/review/wait", Some(wait), None).await;
    let (verdict, text) = verdict_of(answer)?;
    Ok(hook_decision(request, verdict, text))
}

/// Show the document beside the caller and say so on standard error; the
/// body of the request that then waits for its review.
async fn show(server: &ServerInfo, review: &Review) -> Result<Value, CliError> {
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
    Ok(wait)
}

/// The verdict and the review of the answer to a wait, or why the command
/// fails: a closed pane, a document replaced before the wait, an old server,
/// a lost connection, or an answer without the review its verdict needs.
fn verdict_of(
    answer: Result<(u16, Value), CliError>,
) -> Result<(Verdict, Option<String>), CliError> {
    let (status, body) = answer.map_err(|error| match error {
        CliError::Request(cause) => CliError::Failed(format!(
            "The connection to the Splitty server ended before an answer: {cause}"
        )),
        other => other,
    })?;
    let verdict = match (status, body["outcome"].as_str(), body["code"].as_str()) {
        (200, Some("approved"), _) => Verdict::Approved,
        (200, Some("changes_requested"), _) => Verdict::ChangesRequested,
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
        Some(Value::String(review)) => Some(review.clone()),
        Some(_) => return Err(CliError::Failed("The answer holds no review".into())),
    };
    if verdict == Verdict::ChangesRequested && review.is_none() {
        return Err(CliError::Failed("The answer holds no review".into()));
    }
    Ok((verdict, review))
}

/// What the command prints for the answer to its wait, or why it fails: the
/// review of either verdict, nothing for an approval without one, or the
/// outcome as JSON.
fn outcome(answer: Result<(u16, Value), CliError>, json: bool) -> Result<Answer, CliError> {
    let (verdict, review) = verdict_of(answer)?;
    let output = if json {
        let name = match verdict {
            Verdict::Approved => "approved",
            Verdict::ChangesRequested => "changes_requested",
        };
        let mut printed = json!({"outcome": name});
        if let Some(review) = &review {
            printed["review"] = json!(review);
        }
        Some(format!("{}\n", json_text(&printed)))
    } else {
        review.map(|review| {
            let mut review = safe_text(&review);
            if !review.ends_with('\n') {
                review.push('\n');
            }
            review
        })
    };
    Ok(Answer { verdict, output })
}

// ── Claude Code's hook ──────────────────────────────────────────────────────

/// The largest hook event read from standard input, in bytes.
const MAX_HOOK_EVENT_BYTES: usize = 1024 * 1024;
/// The tool of Claude Code that asks to approve a plan.
const PLAN_TOOL: &str = "ExitPlanMode";
/// The title of the text document a plan is shown as.
pub const PLAN_TITLE: &str = "Plan";

/// The hook events of Claude Code that a plan review decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// Claude Code is about to ask the user; runs only when it would.
    PermissionRequest,
    /// Before the tool runs, ahead of every permission check.
    PreToolUse,
}

/// A plan that Claude Code asks the user to approve.
#[derive(Debug)]
pub struct PlanRequest {
    event: HookEvent,
    /// The tool's input, which an approval gives back.
    tool_input: Value,
    plan: String,
    /// Claude Code's working directory, for relative links in the plan.
    cwd: Option<PathBuf>,
}

impl PlanRequest {
    /// The plan shown as `splitty review --text --title Plan` shows a text.
    pub fn review(&self, screen: &str, pane: &str) -> Result<Review, CliError> {
        text(
            self.plan.clone(),
            PLAN_TITLE,
            true,
            screen,
            pane,
            self.cwd.as_deref(),
        )
    }
}

/// The plan of the hook event on `input`, or why the hook leaves the
/// decision to Claude Code (design D1 of `review-claude-plan-hook`): the
/// tool's `plan`, or the file its `planFilePath` names.
pub fn plan_request(input: impl Read) -> Result<PlanRequest, String> {
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(MAX_HOOK_EVENT_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read the hook's event: {e}"))?;
    if bytes.len() > MAX_HOOK_EVENT_BYTES {
        return Err("The hook's event is larger than 1 MiB".into());
    }
    let event: Value =
        serde_json::from_slice(&bytes).map_err(|_| "The hook's event is not JSON".to_owned())?;
    let kind = match event["hook_event_name"].as_str() {
        Some("PermissionRequest") => HookEvent::PermissionRequest,
        Some("PreToolUse") => HookEvent::PreToolUse,
        other => {
            return Err(format!(
                "The hook decides PermissionRequest and PreToolUse, not {}",
                safe_text(other.unwrap_or("an event without a name"))
            ))
        }
    };
    if event["tool_name"].as_str() != Some(PLAN_TOOL) {
        return Err(format!("The hook decides only the tool {PLAN_TOOL}"));
    }
    let tool_input = event
        .get("tool_input")
        .filter(|input| input.is_object())
        .cloned()
        .ok_or_else(|| "The hook's event holds no tool input".to_owned())?;
    let plan = match tool_input["plan"].as_str() {
        Some(plan) if !plan.trim().is_empty() => {
            if plan.len() > MAX_TEXT_BYTES {
                return Err(
                    "The plan is larger than 256 KiB, the most a text document holds".into(),
                );
            }
            plan.to_owned()
        }
        _ => plan_file(&tool_input["planFilePath"])?,
    };
    let cwd = event["cwd"]
        .as_str()
        .map(PathBuf::from)
        .filter(|cwd| cwd.is_absolute());
    Ok(PlanRequest {
        event: kind,
        tool_input,
        plan,
        cwd,
    })
}

/// The plan in the file `path` names: an absolute path of a regular file of
/// at most 256 KiB of UTF-8.
fn plan_file(path: &Value) -> Result<String, String> {
    let path = path
        .as_str()
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "The hook's event holds no plan".to_owned())?;
    let shown = safe_text(&path.display().to_string());
    // A FIFO or a device would block or never end: only a regular file.
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Cannot read the plan file {shown}: {e}"))?;
    if !metadata.is_file() {
        return Err(format!("The plan file {shown} is not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(u64::try_from(MAX_TEXT_BYTES + 1).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
        })
        .map_err(|e| format!("Cannot read the plan file {shown}: {e}"))?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "The plan file {shown} is larger than 256 KiB, the most a text document holds"
        ));
    }
    let plan = String::from_utf8(bytes)
        .map_err(|_| format!("The plan file {shown} is not valid UTF-8"))?;
    if plan.trim().is_empty() {
        return Err(format!("The plan file {shown} is empty"));
    }
    Ok(plan)
}

/// Claude Code's decision on `request` for the user's verdict, as one line
/// of JSON (design D2 and D4 of `review-claude-plan-hook`): an approval
/// allows the call, with its notes after the plan; a request for changes
/// denies it with the review.
pub fn hook_decision(request: &PlanRequest, verdict: Verdict, review: Option<String>) -> String {
    let review = review.map(|review| safe_text(&review));
    let input = || match &review {
        Some(notes) => {
            let mut input = request.tool_input.clone();
            input["plan"] = json!(format!(
                "{}\n\n---\n\n{}\n",
                request.plan.trim_end(),
                notes.trim_end()
            ));
            input
        }
        None => request.tool_input.clone(),
    };
    let reason = || review.clone().unwrap_or_default();
    let output = match (request.event, verdict) {
        (HookEvent::PermissionRequest, Verdict::Approved) => {
            let mut decision = json!({"behavior": "allow"});
            if review.is_some() {
                decision["updatedInput"] = input();
            }
            json!({"hookEventName": "PermissionRequest", "decision": decision})
        }
        (HookEvent::PermissionRequest, Verdict::ChangesRequested) => json!({
            "hookEventName": "PermissionRequest",
            "decision": {"behavior": "deny", "message": reason()},
        }),
        (HookEvent::PreToolUse, Verdict::Approved) => json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": input(),
        }),
        (HookEvent::PreToolUse, Verdict::ChangesRequested) => json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason(),
        }),
    };
    format!("{}\n", json_text(&json!({"hookSpecificOutput": output})))
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

    fn event(name: &str, tool_input: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "session_id": "abc", "transcript_path": "/home/me/.claude/t.jsonl",
            "cwd": "/home/me/app", "permission_mode": "plan",
            "hook_event_name": name, "tool_name": "ExitPlanMode", "tool_input": tool_input,
        }))
        .expect("event")
    }

    /// cli, Review Hook For Claude Code (A plan approved, The plan in a
    /// file): the plan of either event, from the tool's `plan` or the file
    /// its `planFilePath` names, shown as `--text --title Plan` shows a text
    /// in the event's `cwd`.
    #[test]
    fn a_hook_event_yields_its_plan() {
        let input = json!({"plan": "# Plan\n\n1. Step one\n", "planFilePath": "/nowhere/plan.md"});
        let request = plan_request(&event("PermissionRequest", input.clone())[..]).expect("plan");
        assert_eq!(request.event, HookEvent::PermissionRequest);
        assert_eq!(request.plan, "# Plan\n\n1. Step one\n");
        assert_eq!(request.tool_input, input);
        let review = request.review("s1", "p1").expect("review");
        assert_eq!(review.op["source"]["title"], PLAN_TITLE);
        assert_eq!(review.op["source"]["format"], "markdown");
        assert_eq!(review.op["replace"], true);
        assert_eq!(review.op["cwd"], "/home/me/app");
        assert_eq!(review.op["paneId"], "p1");
        assert!(matches!(request.review("s1", ""), Err(CliError::Usage(_))));

        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("plan.md");
        std::fs::write(&file, "# From the file\n").expect("plan file");
        let path = file.to_str().expect("path");
        for input in [
            json!({"planFilePath": path}),
            json!({"plan": " \n", "planFilePath": path}),
        ] {
            let request = plan_request(&event("PreToolUse", input)[..]).expect("plan file");
            assert_eq!(request.event, HookEvent::PreToolUse);
            assert_eq!(request.plan, "# From the file\n");
        }
    }

    /// cli, Review Hook For Claude Code: another event or tool, no plan, a
    /// plan file that is relative, missing, no regular file, empty or too
    /// large, an oversized event or plan, or no JSON: no plan, with the
    /// reason.
    #[test]
    fn a_hook_event_without_a_plan_names_why() {
        let dir = tempfile::tempdir().expect("fixture");
        let empty = dir.path().join("empty.md");
        std::fs::write(&empty, "\n").expect("file");
        let large = dir.path().join("large.md");
        std::fs::write(&large, "x".repeat(MAX_TEXT_BYTES + 1)).expect("file");
        let at_limit = dir.path().join("limit.md");
        std::fs::write(&at_limit, "x".repeat(MAX_TEXT_BYTES)).expect("file");
        assert!(plan_request(
            &event(
                "PermissionRequest",
                json!({"planFilePath": at_limit.to_str()})
            )[..]
        )
        .is_ok());
        let other_tool = serde_json::to_vec(&json!({
            "hook_event_name": "PermissionRequest", "tool_name": "Bash",
            "tool_input": {"command": "ls"},
        }))
        .expect("event");
        let other_event = event("PostToolUse", json!({"plan": "x"}));
        let large_event = format!(
            "{{\"hook_event_name\":\"PreToolUse\",\"pad\":\"{}\"}}",
            "x".repeat(MAX_HOOK_EVENT_BYTES)
        );
        for (input, reason) in [
            (other_tool, "only the tool ExitPlanMode"),
            (other_event, "not PostToolUse"),
            (b"plan".to_vec(), "not JSON"),
            (large_event.into_bytes(), "1 MiB"),
            (
                serde_json::to_vec(
                    &json!({"hook_event_name": "PreToolUse", "tool_name": "ExitPlanMode"}),
                )
                .expect("event"),
                "no tool input",
            ),
            (event("PreToolUse", json!({})), "no plan"),
            (
                event("PreToolUse", json!({"planFilePath": "plan.md"})),
                "no plan",
            ),
            (
                event(
                    "PreToolUse",
                    json!({"plan": "x".repeat(MAX_TEXT_BYTES + 1)}),
                ),
                "256 KiB",
            ),
            (
                event(
                    "PreToolUse",
                    json!({"planFilePath": dir.path().join("none.md").to_str()}),
                ),
                "Cannot read the plan file",
            ),
            (
                event("PreToolUse", json!({"planFilePath": dir.path().to_str()})),
                "not a regular file",
            ),
            (
                event("PreToolUse", json!({"planFilePath": empty.to_str()})),
                "is empty",
            ),
            (
                event("PreToolUse", json!({"planFilePath": large.to_str()})),
                "256 KiB",
            ),
        ] {
            match plan_request(&input[..]) {
                Err(message) => assert!(message.contains(reason), "{reason}: {message}"),
                Ok(request) => panic!("a plan ({reason}): {request:?}"),
            }
        }
    }

    /// cli, Review Hook For Claude Code (A plan approved, Changes requested,
    /// Approved with notes, Before the tool runs): each event's decision for
    /// each verdict, the notes after the plan, the review made safe.
    #[test]
    fn each_verdict_has_its_decision() {
        let input = json!({"plan": "# Plan\n\n1. Step one\n", "planFilePath": "/home/me/.claude/plans/p.md"});
        let permission =
            plan_request(&event("PermissionRequest", input.clone())[..]).expect("plan");
        let pre = plan_request(&event("PreToolUse", input.clone())[..]).expect("plan");
        let review = "# Review of Plan\n\nVerdict: changes requested\n\nSplit step one.\u{202e}\n";
        let safe = "# Review of Plan\n\nVerdict: changes requested\n\nSplit step one.\u{fffd}\n";
        let decision = |request, verdict, review: Option<&str>| {
            let printed = hook_decision(request, verdict, review.map(str::to_owned));
            assert!(
                printed.ends_with('\n') && !printed.trim_end().contains('\n'),
                "{printed}"
            );
            serde_json::from_str::<Value>(&printed).expect("JSON")
        };

        assert_eq!(
            hook_decision(&permission, Verdict::Approved, None),
            "{\"hookSpecificOutput\":{\"decision\":{\"behavior\":\"allow\"},\"hookEventName\":\"PermissionRequest\"}}\n"
        );
        assert_eq!(
            decision(&permission, Verdict::ChangesRequested, Some(review)),
            json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "deny", "message": safe},
            }})
        );
        let mut noted = input.clone();
        noted["plan"] = json!(format!("# Plan\n\n1. Step one\n\n---\n\n{}", safe));
        assert_eq!(
            decision(&permission, Verdict::Approved, Some(review)),
            json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow", "updatedInput": noted},
            }})
        );
        assert_eq!(
            decision(&pre, Verdict::Approved, None),
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse", "permissionDecision": "allow", "updatedInput": input,
            }})
        );
        assert_eq!(
            decision(&pre, Verdict::Approved, Some(review))["hookSpecificOutput"]["updatedInput"],
            noted
        );
        assert_eq!(
            decision(&pre, Verdict::ChangesRequested, Some(review)),
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse", "permissionDecision": "deny",
                "permissionDecisionReason": safe,
            }})
        );
    }
}
