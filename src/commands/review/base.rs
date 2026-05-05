//! Choosing the diff base — explicit `--base` wins, otherwise pick the
//! scope's configured default. In CI on a PR, also resolve a *focus base*
//! (the SHA of our last review) which is sent to the agent as a hint to
//! limit findings to lines changed since — independent of `--base`, which
//! continues to pick the full-PR diff for context.

use std::env;
use std::fs;
use std::path::Path;

use crate::config::ScopeConfig;
use crate::github::fetch_last_reviewed_sha;

/// `--base` from the CLI wins; otherwise fall back to the first scope's
/// configured default (`HEAD` if even that is missing).
pub(super) fn resolve_base(scopes: &[ScopeConfig], cli_base: Option<&str>) -> String {
    if let Some(base) = cli_base {
        return base.to_owned();
    }
    scopes
        .first()
        .map(|s| s.defaults.base.clone())
        .unwrap_or_else(|| "HEAD".to_owned())
}

/// Look up the SHA of the previous blick review on this PR (recovered
/// from blick-authored check runs on prior commits, with a back-compat
/// fallback to the legacy review-body marker) so we can ask the agent to
/// focus on changes since. Independent of `--base`: the full PR diff is
/// still sent as context. Returns `None` when there's no PR context, no
/// prior review, or the SHA isn't reachable in the local clone.
pub(super) fn resolve_focus_base(repo_root: &Path) -> Option<String> {
    let (repo, pr) = detect_pr_context()?;

    let sha = match fetch_last_reviewed_sha(&repo, pr) {
        Ok(Some(sha)) => sha,
        Ok(None) => return None,
        Err(err) => {
            eprintln!("ℹ could not fetch prior blick reviews ({err}); reviewing full PR");
            return None;
        }
    };

    if !revision_exists(repo_root, &sha) {
        eprintln!("ℹ last-reviewed SHA {sha} is not reachable locally; reviewing full PR");
        return None;
    }

    eprintln!("▶ focus mode: previous blick review at {sha}");
    Some(sha)
}

fn detect_pr_context() -> Option<(String, u64)> {
    let path = env::var("GITHUB_EVENT_PATH").ok()?;
    let raw = fs::read_to_string(path).ok()?;
    let event: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let pr = event.get("pull_request")?.get("number")?.as_u64()?;
    let repo = env::var("GITHUB_REPOSITORY").ok()?;
    Some((repo, pr))
}

fn revision_exists(repo_root: &Path, revision: &str) -> bool {
    std::process::Command::new("git")
        .current_dir(repo_root)
        .args(["rev-parse", "--verify", &format!("{revision}^{{commit}}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, AgentKind, EffectiveDefaults};
    use std::path::PathBuf;

    fn scope_with_base(base: &str) -> ScopeConfig {
        ScopeConfig {
            root: PathBuf::from("/repo"),
            agent: AgentConfig {
                kind: AgentKind::Claude,
                model: None,
                binary: None,
                args: Vec::new(),
            },
            skills: Default::default(),
            reviews: Vec::new(),
            defaults: EffectiveDefaults {
                base: base.to_owned(),
                ..Default::default()
            },
            provenance: Vec::new(),
        }
    }

    #[test]
    fn cli_base_overrides_scope_default() {
        let scopes = vec![scope_with_base("origin/main")];
        assert_eq!(resolve_base(&scopes, Some("HEAD~1")), "HEAD~1");
    }

    #[test]
    fn falls_back_to_first_scope_default() {
        let scopes = vec![scope_with_base("origin/main")];
        assert_eq!(resolve_base(&scopes, None), "origin/main");
    }

    #[test]
    fn falls_back_to_head_when_no_scopes() {
        assert_eq!(resolve_base(&[], None), "HEAD");
    }
}
