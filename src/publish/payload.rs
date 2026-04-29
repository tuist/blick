//! Payload massaging used by `publish` when GitHub rejects part of a
//! `github-review` body — currently just folding inline comments into the
//! review body so we still post *something* when GitHub says
//! "Line could not be resolved".

use serde_json::Value;

use crate::error::BlickError;

/// Rewrite a `github-review` payload with its inline `comments` array
/// emptied and the rejected findings appended to the review `body`.
///
/// GitHub rejects inline comments whose lines aren't in its computed view
/// of the PR diff (edge of context, `\ No newline` markers, lines added
/// then removed in later commits, etc.). When that happens we still want
/// the review to land — falling back to a body-only review preserves every
/// finding rather than dropping the whole post.
pub(super) fn strip_inline_comments(payload: &str) -> Result<String, BlickError> {
    let mut value: Value = serde_json::from_str(payload)
        .map_err(|err| BlickError::Api(format!("review payload is not valid JSON: {err}")))?;

    let comments = value
        .get("comments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if !comments.is_empty() {
        let body = value
            .get("body")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_default();

        let mut appended = body;
        appended.push_str("\n\n#### Inline comments not posted\n");
        appended
            .push_str("_GitHub rejected one or more line comments; reproducing them here._\n\n");
        for comment in &comments {
            let path = comment.get("path").and_then(Value::as_str).unwrap_or("?");
            let line = comment.get("line").and_then(Value::as_u64);
            let body = comment.get("body").and_then(Value::as_str).unwrap_or("");
            match line {
                Some(line) => appended.push_str(&format!("**`{path}:{line}`**\n\n{body}\n\n")),
                None => appended.push_str(&format!("**`{path}`**\n\n{body}\n\n")),
            }
        }
        value["body"] = Value::String(appended);
    }

    value["comments"] = Value::Array(Vec::new());
    Ok(serde_json::to_string(&value).expect("serializable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folds_comments_into_body() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\n1 finding.",
            "comments": [
                {
                    "path": "src/foo.rs",
                    "line": 42,
                    "side": "RIGHT",
                    "body": "**[low]** Title\n\nDetails."
                }
            ]
        })
        .to_string();

        let stripped = strip_inline_comments(&payload).unwrap();
        let value: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(value["comments"].as_array().unwrap().len(), 0);
        let body = value["body"].as_str().unwrap();
        assert!(body.contains("Inline comments not posted"));
        assert!(body.contains("`src/foo.rs:42`"));
        assert!(body.contains("Details."));
    }

    #[test]
    fn is_a_noop_when_there_are_no_comments() {
        let payload = json!({
            "commit_id": "deadbeef",
            "event": "COMMENT",
            "body": "### Blick review\n\nNo findings.",
            "comments": []
        })
        .to_string();

        let stripped = strip_inline_comments(&payload).unwrap();
        let value: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(value["body"], "### Blick review\n\nNo findings.");
        assert_eq!(value["comments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn handles_comment_without_line_number() {
        let payload = json!({
            "body": "header",
            "comments": [
                { "path": "src/foo.rs", "body": "no line" }
            ]
        })
        .to_string();
        let stripped = strip_inline_comments(&payload).unwrap();
        let value: Value = serde_json::from_str(&stripped).unwrap();
        let body = value["body"].as_str().unwrap();
        assert!(body.contains("**`src/foo.rs`**"));
        assert!(!body.contains(":"));
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(strip_inline_comments("not json").is_err());
    }
}
