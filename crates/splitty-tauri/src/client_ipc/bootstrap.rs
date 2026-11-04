//! Private initialization capsule. Only native confirmation places a document
//! proof inside its closure; public connection assignments contain no proof.
use serde_json::json;

/// Install with Tauri's main-frame initialization-script API. `entry` is the
/// exact native-assigned packaged entry URL, never renderer-provided input.
pub(crate) fn script(bootstrap: [u8; 32], receiver: &str, entry: &str) -> String {
    include_str!("bootstrap.js").replacen(
        "__SPLITTY_BOOTSTRAP_CONFIG__",
        &json!({ "bootstrap": bootstrap, "receiver": receiver, "entry": entry }).to_string(),
        1,
    )
}

/// Evaluate only after native document commit. Native generation/token checks
/// reject stale evaluations, and repeated evaluation of one generation is safe.
pub(crate) fn challenge_script(receiver: &str, generation: u64, challenge: [u8; 32]) -> String {
    format!(
        "window[{}]?.({});",
        json!(receiver),
        json!({ "generation": generation, "challenge": challenge })
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_encode_native_values_as_json_without_code_interpolation() {
        let rendered = script([7; 32], "receiver\"\\\n", "tauri://localhost/index.html");
        assert!(!rendered.contains("__SPLITTY_BOOTSTRAP_CONFIG__"));
        assert!(rendered.contains("receiver\\\"\\\\\\n"));
        let challenge = challenge_script("receiver\"", 7, [3; 32]);
        assert!(challenge.starts_with("window[\"receiver\\\"\"]?.("));
        assert!(challenge.contains("\"generation\":7"));
    }
}
