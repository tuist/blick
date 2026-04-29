//! Drop already-posted findings from a `github-review` payload before we
//! re-post on a follow-up push. Keeps blick from piling up duplicate inline
//! comments as the same PR gets reviewed multiple times.

use std::collections::HashSet;

use serde_json::Value;

use crate::error::BlickError;
use crate::github::InlineCommentKey;

/// Outcome of [`dedupe_review_payload`].
pub(super) enum DedupeOutcome {
    /// Filtered payload still has new content worth posting.
    Post(String),
    /// Every inline finding was already posted on a prior review and the
    /// body has no remaining out-of-diff findings — nothing new to say.
    Skip,
}

/// Drop any inline comment in `payload` whose `(path, line, body)` matches
/// a previously-posted blick comment. Returns `Skip` when the resulting
/// review would be redundant — every inline finding is a duplicate and the
/// body has no out-of-diff findings to add.
pub(super) fn dedupe_review_payload(
    payload: &str,
    prior: &[InlineCommentKey],
) -> Result<DedupeOutcome, BlickError> {
    if prior.is_empty() {
        return Ok(DedupeOutcome::Post(payload.to_owned()));
    }
    let prior_set: HashSet<&InlineCommentKey> = prior.iter().collect();

    let mut value: Value = serde_json::from_str(payload)
        .map_err(|err| BlickError::Api(format!("review payload is not valid JSON: {err}")))?;

    let original_comments = value
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let original_count = original_comments.len();
    let kept: Vec<Value> = original_comments
        .into_iter()
        .filter(|c| {
            let Some(path) = c.get("path").and_then(Value::as_str) else {
                return true;
            };
            let Some(line) = c.get("line").and_then(Value::as_u64) else {
                return true;
            };
            let Some(body) = c.get("body").and_then(Value::as_str) else {
                return true;
            };
            let key = InlineCommentKey {
                path: path.to_owned(),
                line,
                body: body.to_owned(),
            };
            !prior_set.contains(&key)
        })
        .collect();

    let dropped = original_count - kept.len();
    if dropped > 0 {
        eprintln!(
            "ℹ deduped {dropped} inline finding{} already posted on a prior blick review",
            if dropped == 1 { "" } else { "s" }
        );
    }

    let has_out_of_diff = value
        .get("body")
        .and_then(Value::as_str)
        .map(|b| b.contains("Findings outside this PR"))
        .unwrap_or(false);

    if kept.is_empty() && !has_out_of_diff {
        return Ok(DedupeOutcome::Skip);
    }

    value["comments"] = Value::Array(kept);
    Ok(DedupeOutcome::Post(
        serde_json::to_string(&value).expect("serializable"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_matching_inline_comments() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\n2 findings.",
            "comments": [
                {"path": "src/foo.rs", "line": 10, "side": "RIGHT", "body": "old finding"},
                {"path": "src/foo.rs", "line": 20, "side": "RIGHT", "body": "new finding"},
            ],
        })
        .to_string();
        let prior = vec![InlineCommentKey {
            path: "src/foo.rs".into(),
            line: 10,
            body: "old finding".into(),
        }];
        let DedupeOutcome::Post(out) = dedupe_review_payload(&payload, &prior).unwrap() else {
            panic!("expected Post outcome");
        };
        let value: Value = serde_json::from_str(&out).unwrap();
        let comments = value["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["body"], "new finding");
    }

    #[test]
    fn skips_when_all_comments_match_and_body_has_no_out_of_diff() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\n1 finding across 1 review.",
            "comments": [
                {"path": "src/foo.rs", "line": 10, "side": "RIGHT", "body": "dup"},
            ],
        })
        .to_string();
        let prior = vec![InlineCommentKey {
            path: "src/foo.rs".into(),
            line: 10,
            body: "dup".into(),
        }];
        assert!(matches!(
            dedupe_review_payload(&payload, &prior).unwrap(),
            DedupeOutcome::Skip
        ));
    }

    #[test]
    fn keeps_posting_when_body_has_out_of_diff_findings() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\n#### Findings outside this PR's diff\n- foo",
            "comments": [
                {"path": "src/foo.rs", "line": 10, "side": "RIGHT", "body": "dup"},
            ],
        })
        .to_string();
        let prior = vec![InlineCommentKey {
            path: "src/foo.rs".into(),
            line: 10,
            body: "dup".into(),
        }];
        let DedupeOutcome::Post(out) = dedupe_review_payload(&payload, &prior).unwrap() else {
            panic!("expected Post outcome — body still has out-of-diff findings");
        };
        let value: Value = serde_json::from_str(&out).unwrap();
        assert!(value["comments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn is_a_noop_with_no_prior_comments() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review",
            "comments": [
                {"path": "src/foo.rs", "line": 10, "side": "RIGHT", "body": "x"},
            ],
        })
        .to_string();
        let DedupeOutcome::Post(out) = dedupe_review_payload(&payload, &[]).unwrap() else {
            panic!("expected Post outcome with empty prior");
        };
        // Verbatim passthrough — no JSON re-serialization roundtrip needed.
        assert_eq!(out, payload);
    }
}
