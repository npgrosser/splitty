//! Offline, version-matched workflow instructions, embedded in the CLI.

pub(super) fn text() -> String {
    format!(
        "# Splitty agent guide\n\nCLI version: {}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        include_str!("agent_guide.md")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_covers_the_workflow_without_requiring_a_server_or_a_skill() {
        let guide = text();
        assert!(guide.starts_with("# Splitty agent guide\n"));
        assert!(guide.contains(env!("CARGO_PKG_VERSION")));
        for term in [
            "splitty jobs --json",
            "splitty run --tab --background",
            "python3 -m http.server 8000 --bind 127.0.0.1",
            "jobId",
            "requestId",
            "--request-id REQUEST_ID",
            "literal argv",
            "--cwd DIR",
            "--split left|right|above|below",
            "splitty inspect JOB_ID --json",
            "--since NEXT_CURSOR",
            "nextCursor",
            "hasMore",
            "lostBytes",
            "outputComplete",
            "splitty input JOB_ID --json",
            "exact UTF-8",
            "splitty stop JOB_ID --json",
            "splitty forget JOB_ID --json",
            "splitty split -H",
            "NOT service readiness",
            "SPLITTY_AGENT_SOCK",
            "NOT reading or typing",
            "NOT an OS sandbox",
            "Do not disable your",
            "untrusted data",
            "does NOT replay jobs",
            "old grants/cursors expire",
            "splitty download build/report.pdf",
        ] {
            assert!(guide.contains(term), "guide missing {term}");
        }
        assert!(guide.find("splitty jobs --json") < guide.find("splitty run --tab"));
    }
}
