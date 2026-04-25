use std::path::Path;
use std::process::Command;

use crate::error::BlickError;

#[derive(Debug, Clone)]
pub struct DiffBundle {
    pub files: Vec<String>,
    pub diff: String,
    pub truncated: bool,
}

pub fn collect_diff(base: &str, max_diff_bytes: usize) -> Result<DiffBundle, BlickError> {
    collect_diff_in(Path::new("."), base, max_diff_bytes)
}

pub fn collect_diff_in(
    repo_root: &Path,
    base: &str,
    max_diff_bytes: usize,
) -> Result<DiffBundle, BlickError> {
    ensure_git_repo(repo_root)?;

    let head_exists = has_revision(repo_root, "HEAD")?;
    let base_exists = has_revision(repo_root, base)?;
    if !base_exists && (head_exists || base != "HEAD") {
        return Err(BlickError::Git(format!(
            "git revision {base} was not found"
        )));
    }

    let tracked_files = if base_exists {
        run_git(repo_root, &["diff", "--name-only", "--find-renames", base])?
    } else {
        String::new()
    };
    let untracked_files = run_git(repo_root, &["ls-files", "--others", "--exclude-standard"])?;

    let mut files = tracked_files
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    files.extend(
        untracked_files
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToOwned::to_owned),
    );

    let mut diff = if base_exists {
        run_git(
            repo_root,
            &["diff", "--find-renames", "--no-ext-diff", base],
        )?
    } else {
        String::new()
    };

    for file in untracked_files
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let addition = run_git_allow_exit_codes(
            repo_root,
            &["diff", "--no-index", "--", "/dev/null", file],
            &[0, 1],
        )?;
        diff.push_str(&addition);
        if !addition.ends_with('\n') {
            diff.push('\n');
        }
    }

    let (diff, truncated) = truncate(diff, max_diff_bytes);

    Ok(DiffBundle {
        files,
        diff,
        truncated,
    })
}

fn ensure_git_repo(repo_root: &Path) -> Result<(), BlickError> {
    let output = git_command(repo_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    Err(BlickError::Git(
        "blick review must run inside a git working tree".to_owned(),
    ))
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<String, BlickError> {
    run_git_allow_exit_codes(repo_root, args, &[0])
}

fn run_git_allow_exit_codes(
    repo_root: &Path,
    args: &[&str],
    ok_codes: &[i32],
) -> Result<String, BlickError> {
    let output = git_command(repo_root).args(args).output()?;

    if ok_codes.contains(&output.status.code().unwrap_or_default()) {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(BlickError::Git(format!(
        "git {} failed: {}",
        args.join(" "),
        stderr.trim()
    )))
}

fn has_revision(repo_root: &Path, revision: &str) -> Result<bool, BlickError> {
    let output = git_command(repo_root)
        .args(["rev-parse", "--verify", revision])
        .output()?;

    Ok(output.status.success())
}

fn git_command(repo_root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(repo_root);
    command
}

fn truncate(diff: String, max_diff_bytes: usize) -> (String, bool) {
    if diff.len() <= max_diff_bytes {
        return (diff, false);
    }

    let mut end = max_diff_bytes;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }

    let mut truncated = diff[..end].to_owned();
    truncated.push_str("\n\n... [diff truncated by blick]\n");
    (truncated, true)
}
