//! HTML-comment markers embedded in PR review bodies so future runs can
//! recover state (e.g. the last SHA we reviewed) by reading our own
//! previous comment.

/// Prefix of the `blick:last-reviewed=<sha>` marker.
pub const LAST_REVIEWED_MARKER_PREFIX: &str = "<!-- blick:last-reviewed=";
/// Suffix that closes the marker.
pub const LAST_REVIEWED_MARKER_SUFFIX: &str = " -->";

/// Extract the SHA encoded in the *last* `blick:last-reviewed=<sha>` marker
/// in `body`. Multiple markers can appear if a previous comment was edited
/// repeatedly; we take the last one because it reflects the most recent run.
pub fn parse_last_reviewed_marker(body: &str) -> Option<String> {
    let start = body.rfind(LAST_REVIEWED_MARKER_PREFIX)? + LAST_REVIEWED_MARKER_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(LAST_REVIEWED_MARKER_SUFFIX)?;
    let sha = rest[..end].trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marker_at_end_of_body() {
        let body = "### Blick review\n\n…\n\n<!-- blick:last-reviewed=abc1234 -->";
        assert_eq!(parse_last_reviewed_marker(body).as_deref(), Some("abc1234"));
    }

    #[test]
    fn returns_none_when_marker_is_missing() {
        assert!(parse_last_reviewed_marker("no marker here").is_none());
    }

    #[test]
    fn takes_last_marker_when_multiple_present() {
        let body =
            "<!-- blick:last-reviewed=oldsha -->\nlater\n<!-- blick:last-reviewed=newsha -->";
        assert_eq!(parse_last_reviewed_marker(body).as_deref(), Some("newsha"));
    }

    #[test]
    fn returns_none_for_empty_sha() {
        // A truncated marker shouldn't be treated as a valid SHA.
        let body = "<!-- blick:last-reviewed= -->";
        assert!(parse_last_reviewed_marker(body).is_none());
    }

    #[test]
    fn trims_whitespace_around_sha() {
        let body = "<!-- blick:last-reviewed=  abc123   -->";
        assert_eq!(parse_last_reviewed_marker(body).as_deref(), Some("abc123"));
    }
}
