//! Thin wrapper around the `gh` CLI for posting to the GitHub API.
//!
//! We shell out to `gh` rather than calling the API directly so we inherit
//! the user's existing authentication (env var, keychain, or GH_TOKEN in CI)
//! without re-implementing it.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::BlickError;

/// `POST` to `api_path` (e.g. `repos/owner/repo/check-runs`) with `body` as
/// the JSON payload.
pub(super) fn gh_api_post(api_path: &str, body: &str) -> Result<(), BlickError> {
    let mut child = Command::new("gh")
        .args([
            "api",
            api_path,
            "-X",
            "POST",
            "-H",
            "Accept: application/vnd.github+json",
            "--input",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            BlickError::Api(format!(
                "failed to invoke gh (is it installed and on PATH?): {err}"
            ))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .map_err(|err| BlickError::Api(format!("writing to gh stdin failed: {err}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| BlickError::Api(format!("waiting on gh failed: {err}")))?;
    if !output.status.success() {
        // `gh` writes the API response body to stdout (even on 4xx) and a
        // short status line to stderr. Surface both so 422s tell us *why*.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(BlickError::Api(format!(
            "gh api {api_path} failed: {} {}",
            stderr.trim(),
            stdout.trim(),
        )));
    }
    Ok(())
}
