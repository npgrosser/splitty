//! Pure policy for generic external browser targets. Terminal output is
//! untrusted; only an explicit UI gesture may dispatch an accepted target.

const MAX_URL_BYTES: usize = 16 * 1024;

/// A bounded absolute HTTP(S) URL, normalized for a platform browser opener.
/// Parsing does not perform I/O or authorize an activation gesture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebUrl(String);

impl WebUrl {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.len() > MAX_URL_BYTES || raw.trim() != raw || raw.chars().any(char::is_control) {
            return None;
        }
        // Require an explicit authority. Unlike a web document, a native
        // terminal has no base origin for relative or abbreviated URLs.
        let (scheme, authority) = raw.split_once("://")?;
        if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            || authority.is_empty()
            || authority.starts_with(['/', '\\'])
        {
            return None;
        }
        let parsed = url::Url::parse(raw).ok()?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || parsed.as_str().len() > MAX_URL_BYTES
        {
            return None;
        }
        Some(Self(parsed.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_complete_http_targets_without_dispatch() {
        for (raw, expected) in [
            (
                "HTTPS://Example.COM:443/a/../b?q=one#two",
                "https://example.com/b?q=one#two",
            ),
            ("http://localhost:3000", "http://localhost:3000/"),
            ("http://[::1]:8080/path", "http://[::1]:8080/path"),
            ("https://example.com/界", "https://example.com/%E7%95%8C"),
            ("https://example.com/%0a", "https://example.com/%0a"),
        ] {
            assert_eq!(
                WebUrl::parse(raw).as_ref().map(WebUrl::as_str),
                Some(expected),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_non_web_relative_abbreviated_and_hostless_targets() {
        for raw in [
            "",
            "/path",
            "//example.com/path",
            "www.example.com",
            "https:example.com",
            "https:/example.com",
            "https:///example.com",
            "https://\\example.com",
            "https://",
            "https://?q=x",
            "https://#anchor",
            "https://:80/path",
            "https://[invalid]/",
            "https://example.com:99999/",
            "javascript:alert(1)",
            "file:///tmp/example",
            "vscode://file/tmp/example",
            "zed://file/tmp/example",
            "sftp://example.com/path",
            "mailto:a@example.com",
            "data:text/plain,https://example.com",
            "--help",
        ] {
            assert!(WebUrl::parse(raw).is_none(), "accepted {raw:?}");
        }
    }

    #[test]
    fn rejects_controls_and_implicit_whitespace_trimming() {
        for raw in [
            " https://example.com",
            "https://example.com ",
            "\nhttps://example.com",
            "https://exa\tmple.com",
            "https://example.com/a\0b",
            "https://example.com/\u{7f}",
            "https://example.com/\u{85}",
        ] {
            assert!(WebUrl::parse(raw).is_none(), "accepted {raw:?}");
        }
    }

    #[test]
    fn bounds_both_input_and_percent_encoded_output() {
        let prefix = "https://example.com/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_URL_BYTES - prefix.len()));
        assert_eq!(
            WebUrl::parse(&at_limit).unwrap().as_str().len(),
            MAX_URL_BYTES
        );
        assert!(WebUrl::parse(&(at_limit + "a")).is_none());
        let expands = format!("{prefix}{}", "界".repeat(MAX_URL_BYTES / 4));
        assert!(expands.len() < MAX_URL_BYTES);
        assert!(WebUrl::parse(&expands).is_none());
    }
}
