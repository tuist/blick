//! Thin GitHub helpers built on top of the `gh` CLI.
//!
//! Today this is just enough to look up the SHA blick last reviewed on a
//! pull request — encoded as an HTML-comment marker in the review body — so
//! subsequent runs can review only what changed since.

use std::process::Command;

use serde_json::Value;

use crate::error::BlickError;
use crate::render::parse_last_reviewed_marker;

/// Fetch the most recent `blick:last-reviewed=<sha>` SHA from any review on
/// the given PR. Returns `None` when no prior blick review exists.
pub fn fetch_last_reviewed_sha(repo: &str, pr: u64) -> Result<Option<String>, BlickError> {
    let raw = gh_api_get(&format!("repos/{repo}/pulls/{pr}/reviews?per_page=100"))?;
    let reviews: Vec<Value> = serde_json::from_str(&raw)
        .map_err(|err| BlickError::Api(format!("failed to parse PR reviews JSON: {err}")))?;

    // GitHub returns reviews in chronological order; walk newest-first so the
    // first marker we hit is the most recent reviewed SHA.
    Ok(reviews.iter().rev().find_map(|review| {
        let body = review.get("body").and_then(|v| v.as_str())?;
        parse_last_reviewed_marker(body)
    }))
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
