//! Resolve the repo / head SHA / PR number that `blick publish` operates on,
//! with explicit args overriding GitHub Actions environment data.

use std::env;
use std::fs;

use serde_json::Value;

use crate::error::BlickError;

/// Everything `publish` needs to know about *where* to post.
pub struct PublishContext {
    pub repo: String,
    pub head_sha: String,
    pub pr: Option<u64>,
}

/// Build a [`PublishContext`] from the optional CLI overrides plus the
/// GitHub Actions environment (`GITHUB_REPOSITORY`, `GITHUB_SHA`,
/// `GITHUB_EVENT_PATH`).
pub fn resolve_context(
    head_sha: Option<&str>,
    repo: Option<&str>,
    pr: Option<u64>,
) -> Result<PublishContext, BlickError> {
    let event = read_event_payload();

    let repo = repo
        .map(ToOwned::to_owned)
        .or_else(|| env::var("GITHUB_REPOSITORY").ok())
        .ok_or_else(|| {
            BlickError::Config(
                "could not determine repo (set GITHUB_REPOSITORY or pass --repo owner/repo)"
                    .to_owned(),
            )
        })?;

    let head_sha = head_sha
        .map(ToOwned::to_owned)
        .or_else(|| pr_head_sha_from_event(event.as_ref()))
        .or_else(|| env::var("GITHUB_SHA").ok())
        .ok_or_else(|| {
            BlickError::Config(
                "could not determine head SHA (pass --head-sha or run inside GitHub Actions)"
                    .to_owned(),
            )
        })?;

    let pr = pr.or_else(|| pr_number_from_event(event.as_ref()));

    Ok(PublishContext { repo, head_sha, pr })
}

/// URL of the GitHub Actions workflow run, if we're running inside one.
pub fn workflow_run_url() -> Option<String> {
    let server = env::var("GITHUB_SERVER_URL").ok()?;
    let repo = env::var("GITHUB_REPOSITORY").ok()?;
    let run_id = env::var("GITHUB_RUN_ID").ok()?;
    Some(format!("{server}/{repo}/actions/runs/{run_id}"))
}

fn read_event_payload() -> Option<Value> {
    let path = env::var("GITHUB_EVENT_PATH").ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn pr_head_sha_from_event(event: Option<&Value>) -> Option<String> {
    event?
        .get("pull_request")?
        .get("head")?
        .get("sha")?
        .as_str()
        .map(ToOwned::to_owned)
}

fn pr_number_from_event(event: Option<&Value>) -> Option<u64> {
    event?.get("pull_request")?.get("number")?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_pr_head_sha_from_event_payload() {
        let event = json!({ "pull_request": { "head": { "sha": "abc123" }, "number": 7 } });
        assert_eq!(
            pr_head_sha_from_event(Some(&event)).as_deref(),
            Some("abc123")
        );
        assert_eq!(pr_number_from_event(Some(&event)), Some(7));
    }

    #[test]
    fn returns_none_for_non_pull_request_events() {
        let event = json!({ "push": { "ref": "refs/heads/main" } });
        assert!(pr_head_sha_from_event(Some(&event)).is_none());
        assert!(pr_number_from_event(Some(&event)).is_none());
    }

    #[test]
    fn returns_none_when_event_is_absent() {
        assert!(pr_head_sha_from_event(None).is_none());
        assert!(pr_number_from_event(None).is_none());
    }
}
