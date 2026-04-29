//! `blick learn` — periodic self-improvement pass.
//!
//! Reads merged-PR review activity from the past N days, buckets each blick
//! line-comment thread by whether the human author resolved it, asks the
//! agent to pattern-match recurring failures and successes, and proposes
//! edits to the review setup itself (skills, prompts, agent instructions).
//! Output is a single rolling draft PR on a fixed branch (default
//! `blick/learn`); subsequent runs force-push fresh proposals onto the same
//! branch instead of opening a second PR.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::runner_for;
use crate::config::{LearnConfig, ScopeConfig};
use crate::error::BlickError;
use crate::scope::load_scopes;

/// Substring that uniquely identifies blick-authored review-thread comments.
/// Every line comment blick posts (see [`crate::render`]) ends with a footer
/// linking back to the project, which makes for a more reliable filter than
/// a per-deployment bot login.
const BLICK_COMMENT_SIGNATURE: &str = "[Blick](https://github.com/tuist/blick)";

/// Files learn is allowed to write. Anything outside this list is rejected
/// before the commit stage so the agent can't modify source code.
const EDIT_ALLOWLIST: &[&str] = &["blick.toml", "AGENTS.md", "CLAUDE.md", ".blick/skills/"];

const COMMIT_AUTHOR_NAME: &str = "blick-learn";
const COMMIT_AUTHOR_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";

#[derive(Debug, Default)]
pub struct LearnArgs {
    pub repo: Option<PathBuf>,
    pub dry_run: bool,
    pub force: bool,
    /// Override for the configured lookback window.
    pub lookback_days: Option<u32>,
    /// Override for the configured minimum signal.
    pub min_signal: Option<u32>,
}

pub async fn learn(args: LearnArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;

    let scopes = load_scopes(&repo_root)?;
    let root_scope = pick_root_scope(&scopes, &repo_root)?;
    let mut learn_cfg = load_learn_config(&repo_root)?;
    if let Some(days) = args.lookback_days {
        learn_cfg.lookback_days = days;
    }
    if let Some(min) = args.min_signal {
        learn_cfg.min_signal = min;
    }

    let gh_repo = detect_github_repo(&repo_root)?;
    eprintln!(
        "▶ blick learn against {gh_repo} (lookback {} days, min signal {})",
        learn_cfg.lookback_days, learn_cfg.min_signal
    );

    let cutoff = chrono::Utc::now() - chrono::Duration::days(learn_cfg.lookback_days as i64);
    let prs = fetch_recent_merged_prs(&gh_repo, cutoff)?;
    eprintln!(
        "  found {} merged PR{} since {}",
        prs.len(),
        if prs.len() == 1 { "" } else { "s" },
        cutoff.format("%Y-%m-%d")
    );

    let mut buckets = ThreadBuckets::default();
    for pr in &prs {
        let threads = match fetch_review_threads(&gh_repo, pr.number) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("  ⚠ skipping PR #{}: {err}", pr.number);
                continue;
            }
        };
        for thread in threads {
            let Some(bucket) = bucket_for(&thread) else {
                continue;
            };
            buckets.push(bucket, thread);
        }
    }

    let total = buckets.total();
    eprintln!(
        "  {} blick threads (resolved={}, unresolved={}, outdated={})",
        total,
        buckets.resolved.len(),
        buckets.unresolved.len(),
        buckets.outdated.len(),
    );

    if !args.force && (total as u32) < learn_cfg.min_signal {
        eprintln!(
            "  signal below min ({total} < {}); not opening or updating a PR",
            learn_cfg.min_signal
        );
        return Ok(());
    }

    let allowlist_snapshot = collect_allowlist_files(&repo_root)?;
    let proposal = run_agent_pass(
        root_scope,
        &repo_root,
        &gh_repo,
        &buckets,
        &allowlist_snapshot,
        learn_cfg.lookback_days,
    )
    .await?;

    if proposal.themes.is_empty() {
        eprintln!("  agent returned no themes; nothing to propose");
        return Ok(());
    }
    if proposal.edits.is_empty() {
        eprintln!("  agent returned themes but no edits; skipping PR");
        return Ok(());
    }

    validate_edits(&proposal.edits)?;

    if args.dry_run {
        eprintln!(
            "--- dry run: would propose {} edit(s) ---",
            proposal.edits.len()
        );
        for edit in &proposal.edits {
            eprintln!("  • {} ({} bytes)", edit.path, edit.contents.len());
        }
        println!("{}", serde_json::to_string_pretty(&proposal).unwrap());
        return Ok(());
    }

    apply_and_publish(&repo_root, &gh_repo, &learn_cfg, &proposal)?;
    Ok(())
}

fn pick_root_scope<'a>(
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

fn load_learn_config(repo_root: &Path) -> Result<LearnConfig, BlickError> {
    let path = repo_root.join("blick.toml");
    if !path.exists() {
        return Ok(LearnConfig::default());
    }
    let raw = fs::read_to_string(&path)?;
    let parsed: crate::config::ConfigFile = toml::from_str(&raw)
        .map_err(|err| BlickError::Config(format!("failed to parse {}: {err}", path.display())))?;
    Ok(parsed.learn.unwrap_or_default())
}

fn detect_github_repo(repo_root: &Path) -> Result<String, BlickError> {
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
        .map_err(|err| BlickError::Api(format!("failed to run gh: {err}")))?;
    if !output.status.success() {
        return Err(BlickError::Api(format!(
            "gh repo view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let slug = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if slug.is_empty() {
        return Err(BlickError::Api(
            "gh repo view returned empty owner/repo".to_owned(),
        ));
    }
    Ok(slug)
}

#[derive(Debug, Clone)]
struct PrSummary {
    number: u64,
    #[allow(dead_code)]
    title: String,
    #[allow(dead_code)]
    merged_at: String,
}

fn fetch_recent_merged_prs(
    gh_repo: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<PrSummary>, BlickError> {
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let path = format!(
            "repos/{gh_repo}/pulls?state=closed&sort=updated&direction=desc&per_page=100&page={page}"
        );
        let raw = gh_api_get(&path)?;
        let batch: Vec<Value> = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse PR list: {err}")))?;
        let len = batch.len();
        let mut all_older = !batch.is_empty();
        for pr in batch {
            let Some(merged_at) = pr.get("merged_at").and_then(Value::as_str) else {
                continue;
            };
            let parsed = match chrono::DateTime::parse_from_rfc3339(merged_at) {
                Ok(dt) => dt.with_timezone(&chrono::Utc),
                Err(_) => continue,
            };
            if parsed < cutoff {
                continue;
            }
            all_older = false;
            out.push(PrSummary {
                number: pr.get("number").and_then(Value::as_u64).unwrap_or(0),
                title: pr
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                merged_at: merged_at.to_owned(),
            });
        }
        if len < 100 || all_older {
            break;
        }
        page += 1;
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
struct ReviewThread {
    pr_number: u64,
    is_resolved: bool,
    is_outdated: bool,
    path: Option<String>,
    line: Option<u64>,
    url: Option<String>,
    body: String,
}

fn fetch_review_threads(gh_repo: &str, pr: u64) -> Result<Vec<ReviewThread>, BlickError> {
    let (owner, name) = gh_repo
        .split_once('/')
        .ok_or_else(|| BlickError::Config(format!("expected owner/repo, got {gh_repo}")))?;

    let query = r#"
query($owner: String!, $name: String!, $pr: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $pr) {
      reviewThreads(first: 50, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          isResolved
          isOutdated
          path
          line
          comments(first: 1) {
            nodes { url body author { login } }
          }
        }
      }
    }
  }
}
"#;

    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec![
            "api".to_owned(),
            "graphql".to_owned(),
            "-f".to_owned(),
            format!("query={query}"),
            "-F".to_owned(),
            format!("owner={owner}"),
            "-F".to_owned(),
            format!("name={name}"),
            "-F".to_owned(),
            format!("pr={pr}"),
        ];
        if let Some(c) = &cursor {
            args.push("-F".to_owned());
            args.push(format!("cursor={c}"));
        }
        let output = Command::new("gh")
            .args(&args)
            .output()
            .map_err(|err| BlickError::Api(format!("failed to run gh: {err}")))?;
        if !output.status.success() {
            return Err(BlickError::Api(format!(
                "gh api graphql failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let value: Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
            .map_err(|err| BlickError::Api(format!("graphql response not JSON: {err}")))?;
        let root = value
            .pointer("/data/repository/pullRequest/reviewThreads")
            .ok_or_else(|| {
                BlickError::Api(format!("graphql response missing reviewThreads: {}", value))
            })?;

        if let Some(nodes) = root.get("nodes").and_then(Value::as_array) {
            for node in nodes {
                let Some(comment) = node.pointer("/comments/nodes/0").filter(|v| !v.is_null())
                else {
                    continue;
                };
                let body = comment
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                threads.push(ReviewThread {
                    pr_number: pr,
                    is_resolved: node
                        .get("isResolved")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    is_outdated: node
                        .get("isOutdated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    path: node
                        .get("path")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    line: node.get("line").and_then(Value::as_u64),
                    url: comment
                        .get("url")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    body,
                });
            }
        }

        let page_info = root.get("pageInfo");
        let has_next = page_info
            .and_then(|p| p.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !has_next {
            break;
        }
        cursor = page_info
            .and_then(|p| p.get("endCursor"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(threads)
}

#[derive(Debug, Clone, Copy, Serialize)]
enum Bucket {
    Resolved,
    Unresolved,
    Outdated,
}

fn bucket_for(thread: &ReviewThread) -> Option<Bucket> {
    if !thread.body.contains(BLICK_COMMENT_SIGNATURE) {
        return None;
    }
    if thread.is_outdated {
        return Some(Bucket::Outdated);
    }
    if thread.is_resolved {
        Some(Bucket::Resolved)
    } else {
        Some(Bucket::Unresolved)
    }
}

#[derive(Debug, Default, Serialize)]
struct ThreadBuckets {
    resolved: Vec<ReviewThread>,
    unresolved: Vec<ReviewThread>,
    outdated: Vec<ReviewThread>,
}

impl ThreadBuckets {
    fn push(&mut self, bucket: Bucket, thread: ReviewThread) {
        match bucket {
            Bucket::Resolved => self.resolved.push(thread),
            Bucket::Unresolved => self.unresolved.push(thread),
            Bucket::Outdated => self.outdated.push(thread),
        }
    }
    fn total(&self) -> usize {
        self.resolved.len() + self.unresolved.len() + self.outdated.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Theme {
    title: String,
    rationale: String,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposedEdit {
    path: String,
    contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LearnProposal {
    #[serde(default)]
    themes: Vec<Theme>,
    #[serde(default)]
    edits: Vec<ProposedEdit>,
}

#[derive(Debug, Clone)]
struct AllowlistFile {
    path: String,
    contents: String,
}

fn collect_allowlist_files(repo_root: &Path) -> Result<Vec<AllowlistFile>, BlickError> {
    let mut out = Vec::new();
    for explicit in ["blick.toml", "AGENTS.md", "CLAUDE.md"] {
        let p = repo_root.join(explicit);
        if p.exists()
            && p.is_file()
            && let Ok(body) = fs::read_to_string(&p)
        {
            out.push(AllowlistFile {
                path: explicit.to_owned(),
                contents: body,
            });
        }
    }
    let skills_root = repo_root.join(".blick/skills");
    if skills_root.is_dir() {
        for entry in walkdir::WalkDir::new(&skills_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(repo_root) else {
                continue;
            };
            if let Ok(body) = fs::read_to_string(entry.path()) {
                out.push(AllowlistFile {
                    path: rel.to_string_lossy().into_owned(),
                    contents: body,
                });
            }
        }
    }
    Ok(out)
}

async fn run_agent_pass(
    scope: &ScopeConfig,
    _repo_root: &Path,
    gh_repo: &str,
    buckets: &ThreadBuckets,
    allowlist: &[AllowlistFile],
    lookback_days: u32,
) -> Result<LearnProposal, BlickError> {
    let runner = runner_for(&scope.agent)?;
    let system_prompt = LEARN_SYSTEM_PROMPT.to_owned();
    let user_prompt = build_learn_user_prompt(gh_repo, buckets, allowlist, lookback_days);
    let run = runner.run(&system_prompt, &user_prompt).await?;
    parse_proposal(&run.text)
}

const LEARN_SYSTEM_PROMPT: &str = r#"You are Blick's reflection pass. Your job is to inspect how Blick's past code reviews landed with human reviewers and propose improvements to Blick's own review setup (skills, prompts, agent instructions).

You will be given:
- A bucketed list of Blick's line-comment threads from recent merged PRs:
  - resolved: a human marked the thread resolved (signal that the suggestion was accepted/addressed)
  - unresolved: still open on a merged PR (signal that the suggestion was likely ignored or rejected)
  - outdated: GitHub flagged the thread as out of date (ambiguous; weight low)
- A snapshot of every file you are allowed to edit (blick.toml, AGENTS.md / CLAUDE.md, and anything under .blick/skills/).

What to look for:
- Recurring suggestion types that are repeatedly *ignored* — relax or remove the corresponding rule.
- Recurring suggestion types that are repeatedly *accepted* but phrased poorly (verbose, repetitive, off-tone) — tighten the relevant skill.
- Themes the data hints Blick is missing entirely — propose a new local skill under .blick/skills/<name>/SKILL.md and reference it from a review in blick.toml.

What NOT to do:
- Do not propose edits to source code, tests, CI, or anything outside the allowlist.
- Do not propose speculative edits without clear thread evidence — if a theme has fewer than two supporting threads, drop it.
- Do not rewrite files end-to-end when a small, surgical change is sufficient. Always return the FULL final contents of any file you change, but keep the actual diff minimal.
- Do not invent thread URLs. Only cite URLs that appear in the input.

Output ONLY a JSON object of this shape, no markdown fences:
{
  "themes": [
    {
      "title": "short label",
      "rationale": "1-3 sentences explaining the pattern and what change you propose",
      "evidence": ["https://github.com/.../pull/.../#discussion_r123", "..."]
    }
  ],
  "edits": [
    {
      "path": "blick.toml",
      "contents": "<full new file contents>"
    },
    {
      "path": ".blick/skills/<name>/SKILL.md",
      "contents": "<full file contents>"
    }
  ]
}

If there is not enough signal to justify any change, return {"themes":[],"edits":[]}.
"#;

fn build_learn_user_prompt(
    gh_repo: &str,
    buckets: &ThreadBuckets,
    allowlist: &[AllowlistFile],
    lookback_days: u32,
) -> String {
    let bucketed = json!({
        "repo": gh_repo,
        "lookback_days": lookback_days,
        "resolved": buckets.resolved,
        "unresolved": buckets.unresolved,
        "outdated": buckets.outdated,
    });
    let mut out = String::new();
    out.push_str("Past blick review threads (bucketed):\n\n");
    out.push_str(&serde_json::to_string_pretty(&bucketed).unwrap_or_default());
    out.push_str("\n\nCurrent allowlist files:\n\n");
    for file in allowlist {
        out.push_str(&format!("--- path: {} ---\n", file.path));
        out.push_str(&file.contents);
        if !file.contents.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("--- end ---\n\n");
    }
    out
}

fn parse_proposal(raw: &str) -> Result<LearnProposal, BlickError> {
    if let Ok(p) = serde_json::from_str::<LearnProposal>(raw) {
        return Ok(p);
    }
    if let Some(json) = extract_json_block(raw)
        && let Ok(p) = serde_json::from_str::<LearnProposal>(&json)
    {
        return Ok(p);
    }
    Err(BlickError::Api(format!(
        "agent response was not a valid learn proposal: {}",
        raw.trim()
    )))
}

fn extract_json_block(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    for (offset, ch) in raw[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + ch.len_utf8();
                    return Some(raw[start..end].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

fn validate_edits(edits: &[ProposedEdit]) -> Result<(), BlickError> {
    for edit in edits {
        if !path_in_allowlist(&edit.path) {
            return Err(BlickError::Config(format!(
                "agent proposed edit outside the allowlist: {}",
                edit.path
            )));
        }
        if edit.path.contains("..") {
            return Err(BlickError::Config(format!(
                "edit path contains traversal: {}",
                edit.path
            )));
        }
    }
    Ok(())
}

fn path_in_allowlist(path: &str) -> bool {
    EDIT_ALLOWLIST.iter().any(|allowed| {
        if allowed.ends_with('/') {
            path.starts_with(allowed)
        } else {
            path == *allowed
        }
    })
}

fn apply_and_publish(
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
    git_with_env(
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

fn write_proposal_files(repo_root: &Path, edits: &[ProposedEdit]) -> Result<(), BlickError> {
    for edit in edits {
        let target = repo_root.join(&edit.path);
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
            out.push_str(&format!("  - {}\n", url));
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

fn find_existing_pr(gh_repo: &str, branch: &str) -> Result<Option<u64>, BlickError> {
    let owner = gh_repo.split_once('/').map(|(o, _)| o).unwrap_or("");
    let head = format!("{owner}:{branch}");
    let path = format!("repos/{gh_repo}/pulls?state=open&head={}", urlencode(&head));
    let raw = gh_api_get(&path)?;
    let prs: Vec<Value> = serde_json::from_str(&raw)
        .map_err(|err| BlickError::Api(format!("failed to parse PR list: {err}")))?;
    Ok(prs
        .first()
        .and_then(|pr| pr.get("number"))
        .and_then(Value::as_u64))
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

fn git(repo_root: &Path, args: &[&str]) -> Result<(), BlickError> {
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

fn git_with_env(repo_root: &Path, args: &[&str]) -> Result<(), BlickError> {
    git(repo_root, args)
}

fn git_check(repo_root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn gh_api_get(api_path: &str) -> Result<String, BlickError> {
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

fn gh_api(api_path: &str, method: &str, body: Option<&str>) -> Result<String, BlickError> {
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

    fn thread(body: &str, resolved: bool, outdated: bool) -> ReviewThread {
        ReviewThread {
            pr_number: 1,
            is_resolved: resolved,
            is_outdated: outdated,
            path: Some("src/lib.rs".into()),
            line: Some(1),
            url: Some("https://example/x".into()),
            body: body.into(),
        }
    }

    #[test]
    fn bucket_only_blick_authored_threads() {
        let blick = thread(
            "**foo** body — [Blick](https://github.com/tuist/blick) · default review",
            true,
            false,
        );
        assert!(matches!(bucket_for(&blick), Some(Bucket::Resolved)));

        let other = thread("just a human comment", false, false);
        assert!(bucket_for(&other).is_none());
    }

    #[test]
    fn outdated_overrides_resolved_state() {
        let mut t = thread("x [Blick](https://github.com/tuist/blick) y", true, true);
        assert!(matches!(bucket_for(&t), Some(Bucket::Outdated)));
        t.is_outdated = false;
        t.is_resolved = false;
        assert!(matches!(bucket_for(&t), Some(Bucket::Unresolved)));
    }

    #[test]
    fn allowlist_accepts_skill_subpaths_and_blick_toml() {
        assert!(path_in_allowlist("blick.toml"));
        assert!(path_in_allowlist("AGENTS.md"));
        assert!(path_in_allowlist(".blick/skills/foo/SKILL.md"));
        assert!(!path_in_allowlist("src/lib.rs"));
        assert!(!path_in_allowlist("blick.toml.bak"));
        assert!(!path_in_allowlist("blickXtoml"));
    }

    #[test]
    fn validate_edits_rejects_traversal_and_outside_paths() {
        let bad = vec![ProposedEdit {
            path: "src/main.rs".into(),
            contents: String::new(),
        }];
        assert!(validate_edits(&bad).is_err());
        let traversal = vec![ProposedEdit {
            path: ".blick/skills/../../etc/passwd".into(),
            contents: String::new(),
        }];
        assert!(validate_edits(&traversal).is_err());
        let ok = vec![ProposedEdit {
            path: ".blick/skills/foo/SKILL.md".into(),
            contents: "# foo\n".into(),
        }];
        assert!(validate_edits(&ok).is_ok());
    }

    #[test]
    fn parse_proposal_accepts_plain_and_fenced_json() {
        let plain = r#"{"themes":[{"title":"t","rationale":"r","evidence":[]}],"edits":[]}"#;
        let p = parse_proposal(plain).unwrap();
        assert_eq!(p.themes.len(), 1);

        let noisy = "Sure, here is the proposal:\n```json\n{\"themes\":[],\"edits\":[]}\n```\n";
        let p = parse_proposal(noisy).unwrap();
        assert!(p.themes.is_empty());
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
