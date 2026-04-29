//! Thin GitHub helpers built on top of the `gh` CLI.
//!
//! Today this is just enough to look up the SHA blick last reviewed on a
//! pull request — encoded as an HTML-comment marker in the review body — so
//! subsequent runs can review only what changed since.

use std::process::Command;

use serde_json::Value;

use crate::error::BlickError;
use crate::render::parse_last_reviewed_marker;

/// Header every blick PR review starts with — used as a soft signature so we
/// only trust markers from reviews blick itself authored, not arbitrary
/// reviewers who might paste a marker into their own comment to manipulate
/// the incremental base.
const BLICK_REVIEW_HEADER: &str = "### Blick review";

/// Fetch the most recent `blick:last-reviewed=<sha>` SHA from a prior
/// blick-authored review on the given PR. Returns `None` when no such
/// review exists. Reviews from other authors are ignored even if their
/// body happens to contain a marker.
pub fn fetch_last_reviewed_sha(repo: &str, pr: u64) -> Result<Option<String>, BlickError> {
    let reviews = fetch_all_reviews(repo, pr)?;

    // GitHub returns reviews in chronological order; walk newest-first so the
    // first marker we hit is the most recent reviewed SHA.
    Ok(reviews.iter().rev().find_map(|review| {
        let body = review.get("body").and_then(|v| v.as_str())?;
        if !is_blick_authored(review, body) {
            return None;
        }
        parse_last_reviewed_marker(body)
    }))
}

fn is_blick_authored(review: &Value, body: &str) -> bool {
    // Soft signature: the canonical blick header at the top of the body.
    // Combined with the GITHUB_TOKEN-only write path in CI this is enough
    // to keep arbitrary reviewers from poisoning the incremental base
    // without us also gating on a specific bot identity (which would break
    // self-hosted GitHub Apps that post blick reviews under different names).
    if !body.trim_start().starts_with(BLICK_REVIEW_HEADER) {
        return false;
    }
    // And the author must be a bot — humans don't post reviews with this
    // exact header by accident, and requiring the bot type narrows the
    // attack surface to other automations rather than any reviewer.
    review
        .get("user")
        .and_then(|u| u.get("type"))
        .and_then(|t| t.as_str())
        .map(|t| t == "Bot")
        .unwrap_or(false)
}

/// Identifying tuple of a previously-posted inline review comment, used to
/// dedupe future runs against findings the bot has already raised on this
/// PR. We key on the *literal body* because every blick comment embeds the
/// finding title and body verbatim — same finding → identical body string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InlineCommentKey {
    pub path: String,
    pub line: u64,
    pub body: String,
}

/// Fetch every blick-authored inline review comment on a PR. Returns an
/// empty list (not an error) when there are no prior comments. Comments
/// without a resolvable line, or authored by non-bots / non-blick reviews,
/// are dropped — only blick's own prior findings count toward dedupe.
pub fn fetch_blick_inline_comments(
    repo: &str,
    pr: u64,
) -> Result<Vec<InlineCommentKey>, BlickError> {
    let mut all: Vec<Value> = Vec::new();
    let mut page = 1u32;
    loop {
        let raw = gh_api_get(&format!(
            "repos/{repo}/pulls/{pr}/comments?per_page=100&page={page}"
        ))?;
        let batch: Vec<Value> = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse PR comments JSON: {err}")))?;
        let len = batch.len();
        all.extend(batch);
        if len < 100 {
            break;
        }
        page += 1;
    }

    let keys = all
        .iter()
        .filter_map(extract_blick_comment_key)
        .collect::<Vec<_>>();
    Ok(keys)
}

fn extract_blick_comment_key(comment: &Value) -> Option<InlineCommentKey> {
    let body = comment.get("body").and_then(Value::as_str)?;
    if !is_blick_authored_comment(comment, body) {
        return None;
    }
    // `line` is the latest line in the head; `original_line` is where the
    // comment was first anchored. Prefer `line` so a comment that has
    // followed the diff still keys against its current location, matching
    // what we'd post for the same finding now.
    let line = comment
        .get("line")
        .and_then(Value::as_u64)
        .or_else(|| comment.get("original_line").and_then(Value::as_u64))?;
    let path = comment.get("path").and_then(Value::as_str)?.to_owned();
    Some(InlineCommentKey {
        path,
        line,
        body: body.to_owned(),
    })
}

fn is_blick_authored_comment(comment: &Value, body: &str) -> bool {
    // Blick inline comment bodies always carry the canonical Blick footer
    // link. Combined with the bot-only author check this is enough to keep
    // arbitrary humans/bots from poisoning the dedupe set.
    if !body.contains("https://github.com/tuist/blick") {
        return false;
    }
    comment
        .get("user")
        .and_then(|u| u.get("type"))
        .and_then(|t| t.as_str())
        .map(|t| t == "Bot")
        .unwrap_or(false)
}

fn fetch_all_reviews(repo: &str, pr: u64) -> Result<Vec<Value>, BlickError> {
    let mut all: Vec<Value> = Vec::new();
    let mut page = 1u32;
    loop {
        let raw = gh_api_get(&format!(
            "repos/{repo}/pulls/{pr}/reviews?per_page=100&page={page}"
        ))?;
        let batch: Vec<Value> = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse PR reviews JSON: {err}")))?;
        let len = batch.len();
        all.extend(batch);
        if len < 100 {
            break;
        }
        page += 1;
    }
    Ok(all)
}

fn gh_api_get(api_path: &str) -> Result<String, BlickError> {
    let output = Command::new("gh")
        .args(["api", api_path, "-H", "Accept: application/vnd.github+json"])
        .output()
        .map_err(|err| {
            BlickError::Api(format!(
                "failed to invoke gh (is it installed and on PATH?): {err}"
            ))
        })?;

    if !output.status.success() {
        return Err(BlickError::Api(format!(
            "gh api {api_path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_blick_authored_bot_review() {
        let review = json!({
            "user": {"type": "Bot", "login": "github-actions[bot]"},
            "body": "### Blick review\n\nNo findings.\n\n<!-- blick:last-reviewed=cafef00d -->"
        });
        let body = review["body"].as_str().unwrap();
        assert!(is_blick_authored(&review, body));
    }

    #[test]
    fn rejects_marker_from_human_reviewer() {
        let review = json!({
            "user": {"type": "User", "login": "rando"},
            "body": "### Blick review\n\nLGTM\n\n<!-- blick:last-reviewed=spoofedsha -->"
        });
        let body = review["body"].as_str().unwrap();
        assert!(!is_blick_authored(&review, body));
    }

    #[test]
    fn extracts_blick_inline_comment_key() {
        let comment = json!({
            "path": "src/foo.rs",
            "line": 42,
            "user": {"type": "Bot", "login": "github-actions[bot]"},
            "body": "**title**\n\nbody\n\n— [Blick](https://github.com/tuist/blick) · `web` review",
        });
        let key = extract_blick_comment_key(&comment).expect("blick comment should extract");
        assert_eq!(key.path, "src/foo.rs");
        assert_eq!(key.line, 42);
    }

    #[test]
    fn ignores_non_blick_inline_comment() {
        let comment = json!({
            "path": "src/foo.rs",
            "line": 42,
            "user": {"type": "Bot", "login": "other-bot"},
            "body": "some other automation comment without the blick footer",
        });
        assert!(extract_blick_comment_key(&comment).is_none());
    }

    #[test]
    fn ignores_human_inline_comment_even_with_link() {
        let comment = json!({
            "path": "src/foo.rs",
            "line": 42,
            "user": {"type": "User", "login": "rando"},
            "body": "fyi https://github.com/tuist/blick",
        });
        assert!(extract_blick_comment_key(&comment).is_none());
    }

    #[test]
    fn falls_back_to_original_line_when_line_is_null() {
        let comment = json!({
            "path": "src/foo.rs",
            "line": null,
            "original_line": 10,
            "user": {"type": "Bot", "login": "github-actions[bot]"},
            "body": "see [Blick](https://github.com/tuist/blick) review",
        });
        let key = extract_blick_comment_key(&comment).expect("should extract via original_line");
        assert_eq!(key.line, 10);
    }

    #[test]
    fn rejects_bot_review_without_blick_header() {
        let review = json!({
            "user": {"type": "Bot", "login": "other-bot"},
            "body": "Some other automation report.\n\n<!-- blick:last-reviewed=spoofedsha -->"
        });
        let body = review["body"].as_str().unwrap();
        assert!(!is_blick_authored(&review, body));
    }
}
