//! The data shape of an agent's learn proposal, and the lenient JSON parser
//! used to decode the agent's response.

use serde::{Deserialize, Serialize};

use crate::error::BlickError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Theme {
    pub(super) title: String,
    pub(super) rationale: String,
    #[serde(default)]
    pub(super) evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ProposedEdit {
    pub(super) path: String,
    pub(super) contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LearnProposal {
    #[serde(default)]
    pub(super) themes: Vec<Theme>,
    #[serde(default)]
    pub(super) edits: Vec<ProposedEdit>,
}

pub(super) fn parse_proposal(raw: &str) -> Result<LearnProposal, BlickError> {
    if let Ok(p) = serde_json::from_str::<LearnProposal>(raw) {
        return Ok(p);
    }
    if let Some(json) = extract_json_block(raw)
        && let Ok(p) = serde_json::from_str::<LearnProposal>(&json)
    {
        return Ok(p);
    }
    Err(BlickError::Api(format!(
        "agent response was not a valid learn proposal: {}",
        raw.trim()
    )))
}

fn extract_json_block(raw: &str) -> Option<String> {
    // Same approach as `review::parse::extract_json_block`: try every `{`
    // as a start, with a depth walker that ignores braces inside JSON
    // string literals, and return the first balanced object that
    // round-trips through `serde_json` as a `LearnProposal`. Robust to
    // prose prefixes that contain stray braces.
    let bytes = raw.as_bytes();
    for (start, _) in raw.match_indices('{') {
        let Some(end) = balanced_object_end(&bytes[start..]) else {
            continue;
        };
        let Some(slice) = raw.get(start..start + end) else {
            continue;
        };
        if serde_json::from_str::<LearnProposal>(slice).is_ok() {
            return Some(slice.to_owned());
        }
    }
    None
}

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
    fn parse_proposal_accepts_plain_and_fenced_json() {
        let plain = r#"{"themes":[{"title":"t","rationale":"r","evidence":[]}],"edits":[]}"#;
        let p = parse_proposal(plain).unwrap();
        assert_eq!(p.themes.len(), 1);

        let noisy = "Sure, here is the proposal:\n```json\n{\"themes\":[],\"edits\":[]}\n```\n";
        let p = parse_proposal(noisy).unwrap();
        assert!(p.themes.is_empty());
    }

    #[test]
    fn parse_proposal_rejects_non_json() {
        assert!(parse_proposal("definitely not json").is_err());
    }

    #[test]
    fn extract_json_block_handles_prose_and_braces_in_strings() {
        assert_eq!(
            extract_json_block("noise before {\"a\":1} noise after").as_deref(),
            Some("{\"a\":1}")
        );
        assert_eq!(
            extract_json_block("{\"a\":{\"b\":2},\"c\":3}").as_deref(),
            Some("{\"a\":{\"b\":2},\"c\":3}")
        );
        assert!(extract_json_block("plain text, no json").is_none());
    }

    #[test]
    fn balanced_object_end_ignores_braces_in_strings() {
        let raw = b"{\"a\":\"x{y}z\"}rest";
        let end = balanced_object_end(raw).unwrap();
        assert_eq!(&raw[..end], b"{\"a\":\"x{y}z\"}");
    }

    #[test]
    fn balanced_object_end_returns_none_for_unbalanced_input() {
        assert!(balanced_object_end(b"{nope").is_none());
        assert!(balanced_object_end(b"no opener").is_none());
    }
}
