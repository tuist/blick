//! Thin GitHub helpers built on top of the `gh` CLI.
//!
//! Looks up the SHA blick last reviewed on a PR (so subsequent runs can
//! focus on changes since), and the bot's prior inline comments (so we
//! can dedupe findings). The "last-reviewed" lookup walks the PR's
//! commits newest-first and returns the SHA of the first one with a
//! blick-authored check run, falling back to the legacy
//! `<!-- blick:last-reviewed=<sha> -->` marker on review bodies for PRs
//! reviewed before the marker moved out of the body.

use std::process::Command;

use serde_json::Value;

use crate::error::BlickError;
use crate::render::parse_last_reviewed_marker;

/// Header every legacy blick PR review body started with — used as a soft
/// signature for the back-compat lookup path so arbitrary reviewers can't
/// poison the incremental base by pasting a marker into their own comment.
const BLICK_REVIEW_HEADER: &str = "### Blick review";

/// Prefix on every blick check-run name (see `render::check_run`). Used as
/// a soft signature when scanning a commit's check runs for our own.
const BLICK_CHECK_RUN_NAME_PREFIX: &str = "blick / ";

/// Fetch the most recent SHA blick reviewed on the given PR. Returns
/// `None` when there is no prior blick review.
///
/// Walks the PR's commits newest-first looking for a blick-authored check
/// run on each. If none exist (e.g. an older PR reviewed before the
/// marker moved to check runs), falls back to parsing the legacy
/// `<!-- blick:last-reviewed=<sha> -->` marker out of prior review bodies.
pub fn fetch_last_reviewed_sha(repo: &str, pr: u64) -> Result<Option<String>, BlickError> {
    if let Some(sha) = fetch_last_reviewed_sha_from_check_runs(repo, pr)? {
        return Ok(Some(sha));
    }

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

/// Walk the PR's commits newest-first; return the first commit's SHA that
/// has a check run whose name starts with `blick / `. The check run's own
/// `head_sha` equals the commit SHA, so we don't need a separate marker
/// field — the commit-level association is the marker.
///
/// Worst case this is one `check-runs` API call per PR commit, so we cap
/// the probe depth: blick is virtually always found on or near the head
/// commit, and a deep probe on a force-pushed PR with no prior blick run
/// would just be wasted budget. If we exhaust the cap without a hit, the
/// caller falls back to the legacy review-body marker before giving up.
fn fetch_last_reviewed_sha_from_check_runs(
    repo: &str,
    pr: u64,
) -> Result<Option<String>, BlickError> {
    const MAX_COMMITS_TO_PROBE: usize = 25;
    let commits = fetch_pr_commit_shas(repo, pr)?;
    for sha in commits.iter().rev().take(MAX_COMMITS_TO_PROBE) {
        let runs = fetch_check_runs_for_ref(repo, sha)?;
        if runs.iter().any(is_blick_check_run) {
            return Ok(Some(sha.clone()));
        }
    }
    Ok(None)
}

fn fetch_pr_commit_shas(repo: &str, pr: u64) -> Result<Vec<String>, BlickError> {
    // GitHub caps PR commits at 250 (`/pulls/{n}/commits` truncates beyond
    // that), so 3 pages × 100 covers the entire API response. We don't
    // need to walk every commit anyway — the caller probes newest-first
    // up to its own depth cap — but fetching the full list keeps
    // pagination logic local rather than threading the cap into the
    // request loop.
    const MAX_PAGES: u32 = 3;
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let raw = gh_api_get(&format!(
            "repos/{repo}/pulls/{pr}/commits?per_page=100&page={page}"
        ))?;
        let batch: Vec<Value> = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse PR commits JSON: {err}")))?;
        let len = batch.len();
        for c in &batch {
            if let Some(sha) = c.get("sha").and_then(Value::as_str) {
                all.push(sha.to_owned());
            }
        }
        if len < 100 || page >= MAX_PAGES {
            break;
        }
        page += 1;
    }
    Ok(all)
}

fn fetch_check_runs_for_ref(repo: &str, sha: &str) -> Result<Vec<Value>, BlickError> {
    let raw = gh_api_get(&format!(
        "repos/{repo}/commits/{sha}/check-runs?per_page=100"
    ))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|err| BlickError::Api(format!("failed to parse check-runs JSON: {err}")))?;
    Ok(value
        .get("check_runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn is_blick_check_run(run: &Value) -> bool {
    run.get("name")
        .and_then(Value::as_str)
        .map(|n| n.starts_with(BLICK_CHECK_RUN_NAME_PREFIX))
        .unwrap_or(false)
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
    // Sanity cap on pagination. 20 pages × 100 per page = 2000 comments,
    // which is far more than any real review thread; a runaway loop here
    // would burn the Actions runner's GH API budget and stall the publish
    // step. If a PR ever does exceed this, missing the tail just means a
    // few duplicate findings on the next push — strictly preferable to a
    // hang.
    const MAX_PAGES: u32 = 20;
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
        if page >= MAX_PAGES {
            eprintln!(
                "⚠ stopped fetching PR comments at page {MAX_PAGES} (>{} comments); dedupe may miss the tail",
                MAX_PAGES * 100
            );
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
    fn recognizes_blick_check_run_by_name_prefix() {
        let run = json!({"name": "blick / src/web · security"});
        assert!(is_blick_check_run(&run));
    }

    #[test]
    fn ignores_non_blick_check_runs() {
        let run = json!({"name": "ci / build"});
        assert!(!is_blick_check_run(&run));
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
