//! Resolve the inputs `blick learn` needs to run: which scope to use as
//! "root", the `[learn]` config block, and the GitHub `owner/repo` slug.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config::{ConfigFile, LearnConfig, ScopeConfig};
use crate::error::BlickError;

/// Pick the root-of-repo scope, falling back to whatever the first scope is.
pub(super) fn pick_root_scope<'a>(
    scopes: &'a [ScopeConfig],
    repo_root: &Path,
) -> Result<&'a ScopeConfig, BlickError> {
    scopes
        .iter()
        .find(|s| s.root == repo_root)
        .or_else(|| scopes.first())
        .ok_or_else(|| {
            BlickError::Config(format!(
                "no blick.toml found under {} — `blick learn` needs a root config",
                repo_root.display()
            ))
        })
}

/// Read the `[learn]` block from the repo-root `blick.toml`. Missing file
/// is fine — return defaults. Any other I/O error or parse error surfaces.
pub(super) fn load_learn_config(repo_root: &Path) -> Result<LearnConfig, BlickError> {
    let path = repo_root.join("blick.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LearnConfig::default());
        }
        Err(err) => return Err(BlickError::Io(err)),
    };
    let parsed: ConfigFile = toml::from_str(&raw)
        .map_err(|err| BlickError::Config(format!("failed to parse {}: {err}", path.display())))?;
    Ok(parsed.learn.unwrap_or_default())
}

/// Find the GitHub `owner/repo` slug for the current repository: prefer the
/// `GITHUB_REPOSITORY` env var (set by Actions), fall back to `gh repo view`.
pub(super) fn detect_github_repo(repo_root: &Path) -> Result<String, BlickError> {
    if let Ok(value) = std::env::var("GITHUB_REPOSITORY")
        && !value.is_empty()
    {
        return Ok(value);
    }
    let output = Command::new("gh")
        .current_dir(repo_root)
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()
        .map_err(|err| BlickError::Config(format!("failed to run gh: {err}")))?;
    if !output.status.success() {
        // Failure here is a setup problem (no gh, no remote, wrong cwd) —
        // not an upstream API error — so route it through `Config` for
        // consistency with the rest of the configuration-resolution path.
        return Err(BlickError::Config(format!(
            "gh repo view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let slug = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if slug.is_empty() {
        return Err(BlickError::Config(
            "gh repo view returned empty owner/repo".to_owned(),
        ));
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_blick_toml_yields_default_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load_learn_config(tmp.path()).unwrap();
        assert_eq!(cfg.lookback_days, 7);
    }

    #[test]
    fn empty_learn_block_yields_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("blick.toml"), "[[reviews]]\nname = \"x\"\n").unwrap();
        let cfg = load_learn_config(tmp.path()).unwrap();
        assert_eq!(cfg.lookback_days, 7);
        assert_eq!(cfg.branch, "blick/learn");
    }

    #[test]
    fn reads_explicit_learn_block() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("blick.toml"),
            "[learn]\nlookback_days = 14\nmin_signal = 5\n",
        )
        .unwrap();
        let cfg = load_learn_config(tmp.path()).unwrap();
        assert_eq!(cfg.lookback_days, 14);
        assert_eq!(cfg.min_signal, 5);
    }

    #[test]
    fn surfaces_parse_errors_with_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("blick.toml"), "this = is not = toml").unwrap();
        let err = load_learn_config(tmp.path()).unwrap_err();
        assert!(matches!(err, BlickError::Config(_)));
    }
}
