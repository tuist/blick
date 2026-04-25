use std::process::Command;

use crate::error::BlickError;

#[derive(Debug, Clone)]
pub struct DiffBundle {
    pub files: Vec<String>,
    pub diff: String,
    pub truncated: bool,
}

pub fn collect_diff(base: &str, max_diff_bytes: usize) -> Result<DiffBundle, BlickError> {
    ensure_git_repo()?;

    let has_base = has_revision(base)?;
    let tracked_files = if has_base {
        run_git(&["diff", "--name-only", "--find-renames", base])?
    } else {
        String::new()
    };
    let untracked_files = run_git(&["ls-files", "--others", "--exclude-standard"])?;

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

    let mut diff = if has_base {
        run_git(&["diff", "--find-renames", "--no-ext-diff", base])?
    } else {
        String::new()
    };

    for file in untracked_files
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        let addition =
            run_git_allow_exit_codes(&["diff", "--no-index", "--", "/dev/null", file], &[0, 1])?;
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

fn ensure_git_repo() -> Result<(), BlickError> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    Err(BlickError::Git(
        "blick review must run inside a Git working tree".to_owned(),
    ))
}

fn run_git(args: &[&str]) -> Result<String, BlickError> {
    run_git_allow_exit_codes(args, &[0])
}

fn run_git_allow_exit_codes(args: &[&str], ok_codes: &[i32]) -> Result<String, BlickError> {
    let output = Command::new("git").args(args).output()?;

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

fn has_revision(revision: &str) -> Result<bool, BlickError> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", revision])
        .output()?;

    Ok(output.status.success())
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

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn leaves_small_diff_untouched() {
        let (diff, truncated) = truncate("hello".to_owned(), 20);
        assert_eq!(diff, "hello");
        assert!(!truncated);
    }

    #[test]
    fn truncates_large_diff() {
        let (diff, truncated) = truncate("abcdef".repeat(10), 12);
        assert!(truncated);
        assert!(diff.contains("[diff truncated by blick]"));
    }
}
