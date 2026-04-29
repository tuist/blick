//! Tiny wrappers around `git` subprocess invocations used by the learn
//! workflow. Kept separate so they're easy to swap for a fake in tests if
//! we ever want to exercise the orchestration end-to-end.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::BlickError;

/// Run `git <args>` in `repo_root`, propagating non-zero exit codes as
/// [`BlickError::Git`]. Stdout/stderr are inherited (we want learn's
/// progress visible in CI logs).
pub(super) fn git(repo_root: &Path, args: &[&str]) -> Result<(), BlickError> {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .status()
        .map_err(|err| BlickError::Git(format!("git {}: {err}", args.join(" "))))?;
    if !status.success() {
        return Err(BlickError::Git(format!(
            "git {} exited with {}",
            args.join(" "),
            status
        )));
    }
    Ok(())
}

/// Run `git <args>` and return whether it exited cleanly. Output is
/// suppressed — useful for probes like `rev-parse --verify`.
pub(super) fn git_check(repo_root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
