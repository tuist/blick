//! `gh` CLI wrappers used by the learn workflow.

use std::process::{Command, Stdio};

use crate::error::BlickError;

/// Like [`gh_api_get`] but retries once on a transient rate-limit / abuse
/// failure, with a short backoff. Search and listing endpoints are the only
/// ones with secondary rate limits this code path realistically hits, so
/// retry is scoped to the explicit callers rather than smeared across every
/// `gh_api` invocation.
pub(super) async fn gh_api_get_with_retry(api_path: &str) -> Result<String, BlickError> {
    match gh_api_get(api_path) {
        Ok(value) => Ok(value),
        Err(err) => {
            if !is_transient(&err) {
                return Err(err);
            }
            eprintln!("  ⚠ gh api hit a transient limit; retrying once after 5s ({err})");
            // tokio sleep so we don't block the executor — this function
            // runs from inside an async task on the same runtime as the
            // agent invocation.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            gh_api_get(api_path)
        }
    }
}

fn is_transient(err: &BlickError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("rate limit")
        || msg.contains("secondary rate")
        || msg.contains("abuse detection")
        || msg.contains("429")
}

pub(super) fn gh_api_get(api_path: &str) -> Result<String, BlickError> {
    let output = Command::new("gh")
        .args(["api", api_path, "-H", "Accept: application/vnd.github+json"])
        .output()
        .map_err(|err| BlickError::Api(format!("failed to invoke gh: {err}")))?;
    if !output.status.success() {
        return Err(BlickError::Api(format!(
            "gh api {api_path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn gh_api(
    api_path: &str,
    method: &str,
    body: Option<&str>,
) -> Result<String, BlickError> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "api",
        api_path,
        "-X",
        method,
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    if body.is_some() {
        cmd.args(["--input", "-"]);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| BlickError::Api(format!("failed to invoke gh: {err}")))?;
    if let (Some(body), Some(mut stdin)) = (body, child.stdin.take()) {
        use std::io::Write;
        stdin
            .write_all(body.as_bytes())
            .map_err(|err| BlickError::Api(format!("writing to gh stdin failed: {err}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| BlickError::Api(format!("waiting on gh failed: {err}")))?;
    if !output.status.success() {
        return Err(BlickError::Api(format!(
            "gh api {method} {api_path} failed: {} {}",
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_transient_matches_known_rate_limit_phrasings() {
        assert!(is_transient(&BlickError::Api(
            "API rate limit exceeded".into()
        )));
        assert!(is_transient(&BlickError::Api(
            "secondary rate limit triggered".into()
        )));
        assert!(is_transient(&BlickError::Api(
            "abuse detection mechanism".into()
        )));
        assert!(is_transient(&BlickError::Api("HTTP 429".into())));
    }

    #[test]
    fn is_transient_rejects_unrelated_failures() {
        assert!(!is_transient(&BlickError::Api("not found".into())));
        assert!(!is_transient(&BlickError::Api("permission denied".into())));
    }
}
