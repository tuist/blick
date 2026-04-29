//! Apply a [`LearnProposal`] to the local checkout and push it to a rolling
//! PR on a fixed branch. All file writes go through path-safety checks so
//! a malicious agent response can't escape the repo via symlinks.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::config::LearnConfig;
use crate::error::BlickError;

use super::gh::gh_api;
use super::git::{git, git_check};
use super::proposal::{LearnProposal, ProposedEdit, Theme};

const COMMIT_AUTHOR_NAME: &str = "blick-learn";
const COMMIT_AUTHOR_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";

pub(super) fn apply_and_publish(
    repo_root: &Path,
    gh_repo: &str,
    cfg: &LearnConfig,
    proposal: &LearnProposal,
) -> Result<(), BlickError> {
    git(repo_root, &["fetch", "origin", &cfg.base])?;

    let remote_branch = format!("origin/{}", cfg.branch);
    let branch_exists_remotely = git_check(repo_root, &["rev-parse", "--verify", &remote_branch]);

    if branch_exists_remotely {
        git(repo_root, &["fetch", "origin", &cfg.branch])?;
        ensure_branch_owned(repo_root, &remote_branch, &format!("origin/{}", cfg.base))?;
        git(repo_root, &["checkout", "-B", &cfg.branch, &remote_branch])?;
        git(
            repo_root,
            &["reset", "--hard", &format!("origin/{}", cfg.base)],
        )?;
    } else {
        git(
            repo_root,
            &[
                "checkout",
                "-B",
                &cfg.branch,
                &format!("origin/{}", cfg.base),
            ],
        )?;
    }

    write_proposal_files(repo_root, &proposal.edits)?;

    git(repo_root, &["add", "--", "."])?;

    if !has_staged_changes(repo_root)? {
        eprintln!("  no file changes after applying proposal; nothing to commit");
        return Ok(());
    }

    let commit_msg = build_commit_message(proposal);
    git(
        repo_root,
        &[
            "-c",
            &format!("user.name={COMMIT_AUTHOR_NAME}"),
            "-c",
            &format!("user.email={COMMIT_AUTHOR_EMAIL}"),
            "commit",
            "-m",
            &commit_msg,
        ],
    )?;

    git(
        repo_root,
        &["push", "--force-with-lease", "origin", &cfg.branch],
    )?;

    upsert_pr(gh_repo, cfg, proposal)?;
    Ok(())
}

/// Refuse to overwrite the rolling learn branch when commits ahead of
/// `base` were authored by anyone other than the learn bot identity.
///
/// We compare commit-author email against [`COMMIT_AUTHOR_EMAIL`]. Email is
/// not a strong identity — anyone with push access could set the same
/// `user.email` in their own git config and slip a commit through this
/// guard. That's accepted by design: the learn workflow runs with the
/// repo's `GITHUB_TOKEN`, so the only callers who can push to this branch
/// in practice are workflow runs and repo maintainers, and the guard only
/// exists to prevent the workflow from clobbering its own pushes — not as
/// a hostile-user defense. Branch protection or signed-commit verification
/// would be the right knob if a stronger check were ever needed.
fn ensure_branch_owned(repo_root: &Path, branch: &str, base: &str) -> Result<(), BlickError> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(["log", &format!("{base}..{branch}"), "--format=%ae"])
        .output()
        .map_err(|err| BlickError::Git(format!("git log failed: {err}")))?;
    if !output.status.success() {
        // Branch may not have any commits ahead of base — that's fine.
        return Ok(());
    }
    let emails = String::from_utf8_lossy(&output.stdout);
    for line in emails.lines() {
        let email = line.trim();
        if email.is_empty() {
            continue;
        }
        if email != COMMIT_AUTHOR_EMAIL {
            return Err(BlickError::Git(format!(
                "{branch} has commits authored by {email} (not {COMMIT_AUTHOR_EMAIL}); refusing to overwrite"
            )));
        }
    }
    Ok(())
}

/// Write each proposed edit to disk, refusing any path whose effective
/// destination escapes `repo_root` via a symlink.
///
/// `validate_edit_path` already rejects `..` and absolute paths in the raw
/// string, but a hostile or accidental layout where (e.g.) `.blick/skills`
/// is a symlink pointing at `/home/runner/.ssh/` would let `fs::write`
/// follow that link and clobber files outside the repo. We canonicalize
/// the deepest existing ancestor of the target and require that it stays
/// inside `repo_root.canonicalize()` before writing.
fn write_proposal_files(repo_root: &Path, edits: &[ProposedEdit]) -> Result<(), BlickError> {
    let canonical_root = repo_root.canonicalize().map_err(BlickError::Io)?;

    for edit in edits {
        let target = repo_root.join(&edit.path);

        // Walk up from `target` to find the closest ancestor that already
        // exists on disk; canonicalize that and require it to descend from
        // `canonical_root`. This catches a symlinked intermediate
        // directory regardless of whether the leaf file exists yet.
        let mut probe = target.as_path();
        let anchor = loop {
            if probe.exists() {
                break probe.canonicalize().map_err(BlickError::Io)?;
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                None => {
                    return Err(BlickError::Config(format!(
                        "edit target has no existing ancestor: {}",
                        target.display()
                    )));
                }
            }
        };
        if !anchor.starts_with(&canonical_root) {
            return Err(BlickError::Config(format!(
                "edit target escapes the repository via a symlink: {} → {}",
                target.display(),
                anchor.display()
            )));
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, &edit.contents)?;
    }
    Ok(())
}

fn has_staged_changes(repo_root: &Path) -> Result<bool, BlickError> {
    let status = Command::new("git")
        .current_dir(repo_root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|err| BlickError::Git(format!("git diff failed: {err}")))?;
    // `--quiet` exits 1 when there are differences, 0 when clean.
    Ok(!status.success())
}

fn build_commit_message(proposal: &LearnProposal) -> String {
    let summary = if proposal.themes.is_empty() {
        "tune review setup from recent feedback".to_owned()
    } else {
        proposal
            .themes
            .iter()
            .map(|t| t.title.as_str())
            .take(3)
            .collect::<Vec<_>>()
            .join("; ")
    };
    let body = proposal
        .themes
        .iter()
        .map(|t| format!("- {}: {}", t.title, t.rationale))
        .collect::<Vec<_>>()
        .join("\n");
    if body.is_empty() {
        format!("chore(learn): {summary}")
    } else {
        format!("chore(learn): {summary}\n\n{body}")
    }
}

fn upsert_pr(gh_repo: &str, cfg: &LearnConfig, proposal: &LearnProposal) -> Result<(), BlickError> {
    let body = build_pr_body(proposal);

    let existing = find_existing_pr(gh_repo, &cfg.branch)?;
    match existing {
        Some(number) => {
            let payload = json!({"body": body}).to_string();
            gh_api(
                &format!("repos/{gh_repo}/pulls/{number}"),
                "PATCH",
                Some(&payload),
            )?;
            eprintln!("✓ updated existing learn PR #{number}");
        }
        None => {
            let payload = json!({
                "title": pr_title(proposal),
                "head": cfg.branch,
                "base": cfg.base,
                "body": body,
                "draft": cfg.draft,
            })
            .to_string();
            let raw = gh_api(&format!("repos/{gh_repo}/pulls"), "POST", Some(&payload))?;
            let value: Value = serde_json::from_str(&raw)
                .map_err(|err| BlickError::Api(format!("PR create response not JSON: {err}")))?;
            let number = value.get("number").and_then(Value::as_u64).ok_or_else(|| {
                BlickError::Api(format!("PR create response missing number: {raw}"))
            })?;
            eprintln!("✓ opened learn PR #{number}");

            if !cfg.reviewers.is_empty() || !cfg.team_reviewers.is_empty() {
                let payload = json!({
                    "reviewers": cfg.reviewers,
                    "team_reviewers": cfg.team_reviewers,
                })
                .to_string();
                if let Err(err) = gh_api(
                    &format!("repos/{gh_repo}/pulls/{number}/requested_reviewers"),
                    "POST",
                    Some(&payload),
                ) {
                    eprintln!("⚠ failed to assign reviewers: {err}");
                }
            }
            if !cfg.labels.is_empty() {
                let payload = json!({"labels": cfg.labels}).to_string();
                if let Err(err) = gh_api(
                    &format!("repos/{gh_repo}/issues/{number}/labels"),
                    "POST",
                    Some(&payload),
                ) {
                    eprintln!("⚠ failed to apply labels: {err}");
                }
            }
        }
    }
    Ok(())
}

fn pr_title(proposal: &LearnProposal) -> String {
    if proposal.themes.is_empty() {
        "chore(learn): tune review setup from recent feedback".to_owned()
    } else {
        let lead = proposal
            .themes
            .iter()
            .map(|t| t.title.as_str())
            .take(2)
            .collect::<Vec<_>>()
            .join("; ");
        format!("chore(learn): {lead}")
    }
}

fn build_pr_body(proposal: &LearnProposal) -> String {
    let mut out = String::from(
        "### Blick learn\n\n\
         Automated proposal generated by `blick learn` from recent PR review activity. \
         Each entry below summarizes a pattern observed across human-resolved or \
         human-ignored Blick line comments and the corresponding tweak to the review setup.\n\n",
    );
    out.push_str("#### Themes\n\n");
    let mut by_title: BTreeMap<&str, &Theme> = BTreeMap::new();
    for theme in &proposal.themes {
        by_title.insert(theme.title.as_str(), theme);
    }
    for theme in by_title.values() {
        out.push_str(&format!("- **{}** — {}\n", theme.title, theme.rationale));
        for url in &theme.evidence {
            out.push_str(&format!("  - {url}\n"));
        }
    }
    out.push_str("\n#### Edits\n\n");
    for edit in &proposal.edits {
        out.push_str(&format!("- `{}`\n", edit.path));
    }
    out.push_str(
        "\n---\n\n_This PR is updated in place by the `blick-learn` workflow. Force pushes to this branch are expected; commit manually elsewhere._\n",
    );
    out
}

/// Look up the open PR (if any) whose head branch is `branch` in `gh_repo`,
/// using `gh pr list`. Delegating to `gh` keeps us out of the
/// `?head=owner:branch` URL-encoding business and works the same way against
/// forked PRs and same-repo PRs.
fn find_existing_pr(gh_repo: &str, branch: &str) -> Result<Option<u64>, BlickError> {
    let output = Command::new("gh")
        .args([
            "pr", "list", "--repo", gh_repo, "--head", branch, "--state", "open", "--json",
            "number", "--limit", "1",
        ])
        .output()
        .map_err(|err| BlickError::Api(format!("failed to run gh pr list: {err}")))?;
    if !output.status.success() {
        return Err(BlickError::Api(format!(
            "gh pr list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let prs: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|err| BlickError::Api(format!("failed to parse gh pr list output: {err}")))?;
    Ok(prs
        .first()
        .and_then(|pr| pr.get("number"))
        .and_then(Value::as_u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_proposal_files_rejects_symlinked_directories_outside_repo() {
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        // `<repo>/.blick/skills` is a symlink pointing into another
        // tempdir. The agent's edit path `.blick/skills/foo/SKILL.md` is
        // in the allowlist, but writing through the symlink would land
        // outside the repository. The hardened writer must refuse.
        std::fs::create_dir_all(repo.path().join(".blick")).unwrap();
        symlink(outside.path(), repo.path().join(".blick/skills")).unwrap();

        let edits = vec![ProposedEdit {
            path: ".blick/skills/foo/SKILL.md".into(),
            contents: "# foo\n".into(),
        }];
        let err =
            write_proposal_files(repo.path(), &edits).expect_err("symlink escape must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("escapes the repository"),
            "expected escape error, got: {msg}"
        );

        // The escape was caught before any write, so nothing landed in
        // the outside dir.
        assert!(
            !outside.path().join("foo/SKILL.md").exists(),
            "symlinked write should not have produced a file"
        );
    }

    #[test]
    fn write_proposal_files_writes_into_a_clean_repo_layout() {
        let repo = tempfile::tempdir().unwrap();
        let edits = vec![ProposedEdit {
            path: ".blick/skills/foo/SKILL.md".into(),
            contents: "# foo\n".into(),
        }];
        write_proposal_files(repo.path(), &edits).expect("clean write should succeed");
        let written =
            std::fs::read_to_string(repo.path().join(".blick/skills/foo/SKILL.md")).unwrap();
        assert_eq!(written, "# foo\n");
    }

    #[test]
    fn pr_title_is_conventional_commit_with_lead_themes() {
        let with_themes = LearnProposal {
            themes: vec![
                Theme {
                    title: "noisy async warning".into(),
                    rationale: "x".into(),
                    evidence: vec![],
                },
                Theme {
                    title: "missing tokio-channels skill".into(),
                    rationale: "y".into(),
                    evidence: vec![],
                },
                Theme {
                    title: "third theme".into(),
                    rationale: "z".into(),
                    evidence: vec![],
                },
            ],
            edits: vec![],
        };
        let title = pr_title(&with_themes);
        assert!(title.starts_with("chore(learn): "));
        assert!(title.contains("noisy async warning"));
        assert!(title.contains("missing tokio-channels skill"));
        // Only the first two themes are folded into the title — the third
        // would push the subject past readable length.
        assert!(!title.contains("third theme"));

        let empty = LearnProposal {
            themes: vec![],
            edits: vec![],
        };
        assert_eq!(
            pr_title(&empty),
            "chore(learn): tune review setup from recent feedback"
        );
    }

    #[test]
    fn build_commit_message_includes_themes_in_body() {
        let proposal = LearnProposal {
            themes: vec![Theme {
                title: "noisy async warning".into(),
                rationale: "Ignored 4/5 times.".into(),
                evidence: vec![],
            }],
            edits: vec![],
        };
        let msg = build_commit_message(&proposal);
        let mut lines = msg.lines();
        assert_eq!(
            lines.next(),
            Some("chore(learn): noisy async warning"),
            "subject must use conventional-commit prefix"
        );
        assert!(msg.contains("Ignored 4/5 times."));
    }

    #[test]
    fn build_commit_message_uses_default_summary_when_no_themes() {
        let proposal = LearnProposal {
            themes: vec![],
            edits: vec![],
        };
        let msg = build_commit_message(&proposal);
        assert_eq!(msg, "chore(learn): tune review setup from recent feedback");
    }

    #[test]
    fn pr_body_includes_themes_and_edits() {
        let proposal = LearnProposal {
            themes: vec![Theme {
                title: "noisy async warning".into(),
                rationale: "Ignored on 4 of 5 PRs.".into(),
                evidence: vec!["https://example/r/1".into()],
            }],
            edits: vec![ProposedEdit {
                path: ".blick/skills/foo/SKILL.md".into(),
                contents: "x".into(),
            }],
        };
        let body = build_pr_body(&proposal);
        assert!(body.contains("noisy async warning"));
        assert!(body.contains(".blick/skills/foo/SKILL.md"));
        assert!(body.contains("Ignored on 4 of 5 PRs."));
    }
}
