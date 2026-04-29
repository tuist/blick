//! Parsing the agent's textual response into a [`ReviewReport`].
//!
//! Agents — even ones we ask for "JSON only" — frequently wrap the JSON in
//! markdown fences, prefix it with prose ("Sure, here's the review…"), or
//! include free-form `{}` placeholders inside finding bodies. We accept all
//! of those rather than failing the run.

use crate::error::BlickError;

use super::types::ReviewReport;

/// Parse a raw agent response, accepting a few common formats:
/// 1. plain JSON
/// 2. JSON wrapped in ```` ```…``` ```` fences
/// 3. JSON preceded or followed by free-form prose
pub fn parse_report(raw: &str) -> Result<ReviewReport, BlickError> {
    if let Ok(report) = serde_json::from_str::<ReviewReport>(raw) {
        return Ok(report);
    }

    if let Some(json) = extract_json_block(raw)
        && let Ok(report) = serde_json::from_str::<ReviewReport>(&json)
    {
        return Ok(report);
    }

    Err(BlickError::Api(format!(
        "agent response was not valid review JSON: {}",
        raw.trim()
    )))
}

fn extract_json_block(raw: &str) -> Option<String> {
    if let Some(stripped) = raw.strip_prefix("```") {
        let stripped = stripped
            .split_once('\n')
            .map(|(_, remainder)| remainder)
            .unwrap_or(stripped);
        if let Some((json, _)) = stripped.split_once("\n```") {
            return Some(json.trim().to_owned());
        }
    }

    // Try every `{` in the raw string as a candidate start: walk forward
    // tracking brace depth (with awareness of JSON string literals so a `{`
    // or `}` inside `"..."` doesn't throw the counter off) and return the
    // first balanced object that parses as a `ReviewReport`. This is
    // deliberately more permissive than a single-pass walk so prose with
    // stray braces ahead of the real JSON can't drop the whole report.
    let bytes = raw.as_bytes();
    for (start, _) in raw.match_indices('{') {
        if let Some(end) = balanced_object_end(&bytes[start..])
            && let Some(slice) = raw.get(start..start + end)
            && serde_json::from_str::<ReviewReport>(slice).is_ok()
        {
            return Some(slice.to_owned());
        }
    }
    None
}

/// Find the byte offset, relative to `bytes`, just past the closing `}` of
/// the first balanced JSON object, treating `"..."` (with `\\` and `\"`
/// escapes) as opaque so braces inside string literals don't perturb the
/// depth counter. Returns `None` if `bytes` doesn't start with `{` or never
/// closes.
fn balanced_object_end(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let report = parse_report(
            r#"{"summary":"Looks risky.","findings":[{"severity":"high","file":"src/lib.rs","line":12,"title":"panic path","body":"This can panic on empty input."}]}"#,
        )
        .expect("plain json should parse");
        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn parses_prose_prefixed_json_with_curly_placeholders_in_body() {
        // Reproduces a CI failure: the agent prefixed its output with
        // free-form prose, and a finding body contained string literals
        // with `{}` and `{err}` formatting placeholders. The previous
        // brace-counter walked into those without tracking string
        // boundaries and never found a balanced close.
        let raw = "I'll analyze this diff…\n\
            Let me reason out loud first.\n\n\
            {\"summary\":\"x\",\"findings\":[\
            {\"severity\":\"low\",\"file\":\"src/learn.rs\",\"line\":89,\
            \"title\":\"silent error\",\
            \"body\":\"`eprintln!(\\\"  ⚠ skipping PR #{}: {err}\\\", pr.number);` — comment\"}\
            ]}\n\nLet me know if you want me to elaborate.";
        let report = parse_report(raw).expect("prose-prefixed JSON should parse");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].title, "silent error");
    }

    #[test]
    fn parses_fenced_json() {
        let report = parse_report(
            r#"```json
{"summary":"No findings.","findings":[]}
```"#,
        )
        .expect("fenced json should parse");
        assert!(report.findings.is_empty());
    }

    #[test]
    fn parses_fenced_json_without_language_tag() {
        let report = parse_report("```\n{\"summary\":\"ok\",\"findings\":[]}\n```")
            .expect("bare fences should parse");
        assert_eq!(report.summary, "ok");
    }

    #[test]
    fn rejects_when_response_has_no_json() {
        let err = parse_report("here is some prose without any JSON").unwrap_err();
        assert!(matches!(err, BlickError::Api(_)));
    }

    #[test]
    fn balanced_object_end_handles_escaped_quotes() {
        // The body ends after the second `}` — the escaped quote inside the
        // string literal must not close the string.
        let raw = b"{\"a\":\"x\\\"y{nope}\",\"b\":1}rest";
        let end = balanced_object_end(raw).unwrap();
        assert_eq!(&raw[..end], b"{\"a\":\"x\\\"y{nope}\",\"b\":1}");
    }

    #[test]
    fn balanced_object_end_returns_none_when_unbalanced() {
        assert!(balanced_object_end(b"{\"a\":1").is_none());
        assert!(balanced_object_end(b"not-an-object").is_none());
    }
}
