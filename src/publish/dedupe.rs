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
    /// Nothing new to post — every inline finding was already raised on a
    /// prior review, or the run produced no in-diff findings to begin with
    /// (out-of-diff findings are surfaced via check runs instead).
    Skip,
}

/// Drop any inline comment in `payload` whose `(path, line, body)` matches
/// a previously-posted blick comment. Returns `Skip` when no inline
/// comments remain after filtering — the PR review object only carries
/// inline comments now, so a kept-empty payload would be a no-op post.
pub(super) fn dedupe_review_payload(
    payload: &str,
    prior: &[InlineCommentKey],
) -> Result<DedupeOutcome, BlickError> {
    let mut value: Value = serde_json::from_str(payload)
        .map_err(|err| BlickError::Api(format!("review payload is not valid JSON: {err}")))?;

    let original_comments = value
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if original_comments.is_empty() {
        // Nothing to post — every finding was either out-of-diff (which
        // surfaces in the per-record check-run summary) or the run had no
        // findings at all.
        return Ok(DedupeOutcome::Skip);
    }

    if prior.is_empty() {
        // Common first-run case: skip the filter loop and re-serialization
        // and hand back the payload as-is.
        return Ok(DedupeOutcome::Post(payload.to_owned()));
    }

    let prior_set: HashSet<&InlineCommentKey> = prior.iter().collect();
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

    if kept.is_empty() {
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
            "body": "",
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
    fn skips_when_all_comments_match_prior() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "",
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
    fn skips_when_payload_has_no_inline_comments() {
        // E.g. a run where every finding was out-of-diff: there's nothing
        // for the PR review object to carry, so don't post one.
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "",
            "comments": [],
        })
        .to_string();
        assert!(matches!(
            dedupe_review_payload(&payload, &[]).unwrap(),
            DedupeOutcome::Skip
        ));
    }

    #[test]
    fn posts_when_prior_is_empty_and_payload_has_comments() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "",
            "comments": [
                {"path": "src/foo.rs", "line": 10, "side": "RIGHT", "body": "x"},
            ],
        })
        .to_string();
        let DedupeOutcome::Post(out) = dedupe_review_payload(&payload, &[]).unwrap() else {
            panic!("expected Post outcome");
        };
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["comments"].as_array().unwrap().len(), 1);
    }
}
