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
    // `args` is owned, so we can move `args.repo` directly instead of
    // cloning it before defaulting to `.`.
    let repo_root = args
        .repo
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
    let prs = fetch_recent_merged_prs(&gh_repo, cutoff).await?;
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
        "  {} threads (blick resolved={}, blick unresolved={}, blick outdated={}, human-authored={})",
        total,
        buckets.blick_resolved.len(),
        buckets.blick_unresolved.len(),
        buckets.blick_outdated.len(),
        buckets.human_authored.len(),
    );

    if !args.force && (total as u32) < learn_cfg.min_signal {
        eprintln!(
            "  signal below min ({total} < {}); not opening or updating a PR",
            learn_cfg.min_signal
        );
        return Ok(());
    }

    // The allowlist scan walks `.blick/skills/**` and reads every file —
    // bounded but still blocking I/O, so push it onto the blocking pool to
    // keep the async runtime free.
    let allowlist_snapshot = {
        let repo_root = repo_root.clone();
        tokio::task::spawn_blocking(move || collect_allowlist_files(&repo_root))
            .await
            .map_err(|err| BlickError::Api(format!("allowlist scan join failed: {err}")))??
    };
    let proposal = run_agent_pass(
        root_scope,
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
        let rendered = serde_json::to_string_pretty(&proposal)
            .map_err(|err| BlickError::Api(format!("failed to serialize learn proposal: {err}")))?;
        println!("{rendered}");
        return Ok(());
    }

    // `apply_and_publish` chains a series of `git` and `gh` subprocesses
    // and writes files — all blocking. Run it on the blocking pool so the
    // runtime can keep handling other tasks while it waits.
    let repo_root_for_publish = repo_root.clone();
    let gh_repo_for_publish = gh_repo.clone();
    let learn_cfg_for_publish = learn_cfg.clone();
    tokio::task::spawn_blocking(move || {
        apply_and_publish(
            &repo_root_for_publish,
            &gh_repo_for_publish,
            &learn_cfg_for_publish,
            &proposal,
        )
    })
    .await
    .map_err(|err| BlickError::Api(format!("apply_and_publish join failed: {err}")))??;
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
    // Try to read the file directly and fall back to defaults on `NotFound`,
    // saving a stat call vs the previous `exists() && read()` two-step. Any
    // other I/O error (permissions, etc.) is still surfaced to the caller.
    let raw = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LearnConfig::default());
        }
        Err(err) => return Err(BlickError::Io(err)),
    };
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

#[derive(Debug, Clone)]
struct PrSummary {
    number: u64,
}

/// Fetch numbers of every PR merged in `gh_repo` since `cutoff`, using the
/// search API.
///
/// We deliberately use `search/issues` with `is:merged merged:>=DATE` rather
/// than `repos/.../pulls?state=closed&sort=updated`: the pulls endpoint
/// orders by `updated_at` (no `merged_at` sort exists), which means a PR
/// merged before the cutoff but commented on yesterday appears before a
/// recently-merged PR with no fresh activity, so any "stop when all results
/// on this page are older" optimization can stop too early. The search API
/// filters server-side by merge date so we can paginate without that
/// heuristic.
async fn fetch_recent_merged_prs(
    gh_repo: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<PrSummary>, BlickError> {
    let cutoff_date = cutoff.format("%Y-%m-%d").to_string();
    let query = format!("repo:{gh_repo} is:pr is:merged merged:>={cutoff_date}");
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        let path = format!(
            "search/issues?q={}&per_page=100&page={page}",
            percent_encode_query(&query),
        );
        let raw = gh_api_get_with_retry(&path).await?;
        let response: Value = serde_json::from_str(&raw)
            .map_err(|err| BlickError::Api(format!("failed to parse search response: {err}")))?;
        let items = response
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let len = items.len();
        for item in items {
            if let Some(number) = item.get("number").and_then(Value::as_u64) {
                out.push(PrSummary { number });
            }
        }
        // The search API caps at 1000 results (10 pages of 100). Stop early
        // when we drain a partial page or hit the cap.
        if len < 100 || page >= 10 {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// Percent-encode an entire query string for the `q=` parameter of the
/// search API. Only alphanumerics and a small set of unreserved characters
/// pass through untouched; everything else (including spaces and `:`) is
/// `%XX`-encoded so `gh api` doesn't have to interpret it.
fn percent_encode_query(input: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            // `write!` against a `String` is infallible — the `_ =` is a
            // visible nudge that we're intentionally not propagating an
            // error here. Avoids the per-byte `format!` allocation that an
            // earlier version of this function did for every escaped char.
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
struct ReviewThread {
    pr_number: u64,
    is_resolved: bool,
    is_outdated: bool,
    path: Option<String>,
    line: Option<u64>,
    url: Option<String>,
    /// All comments on the thread in chronological order. The first entry is
    /// the comment that opened the thread; later entries are replies.
    comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, Serialize)]
struct ThreadComment {
    author: String,
    /// `"Bot"` or `"User"`, as reported by the GraphQL API. Useful when we
    /// can't pin down the bot login but still want to distinguish the two.
    author_type: String,
    body: String,
}

impl ReviewThread {
    fn first_body(&self) -> &str {
        self.comments.first().map(|c| c.body.as_str()).unwrap_or("")
    }

    fn first_author_type(&self) -> &str {
        self.comments
            .first()
            .map(|c| c.author_type.as_str())
            .unwrap_or("")
    }
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
          comments(first: 20) {
            pageInfo { hasNextPage }
            nodes { url body author { login __typename } }
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
                let comment_nodes = node
                    .pointer("/comments/nodes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if comment_nodes.is_empty() {
                    continue;
                }
                if node
                    .pointer("/comments/pageInfo/hasNextPage")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    eprintln!(
                        "  ⚠ PR #{pr} thread has more than {} comments; truncating (later replies are not in the agent's input)",
                        comment_nodes.len()
                    );
                }
                let mut comments: Vec<ThreadComment> = Vec::with_capacity(comment_nodes.len());
                for c in &comment_nodes {
                    comments.push(ThreadComment {
                        author: c
                            .pointer("/author/login")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        author_type: c
                            .pointer("/author/__typename")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        body: c
                            .get("body")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    });
                }
                let url = comment_nodes
                    .first()
                    .and_then(|c| c.get("url"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
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
                    url,
                    comments,
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
    /// Thread opened by blick that a human resolved (suggestion accepted).
    BlickResolved,
    /// Thread opened by blick that was left unresolved on a merged PR
    /// (suggestion likely ignored).
    BlickUnresolved,
    /// Thread opened by blick that GitHub flagged as outdated (ambiguous).
    BlickOutdated,
    /// Thread opened by a human reviewer — strong signal of either
    /// something blick missed, or a class of feedback humans care about
    /// that the current skills don't cover.
    HumanAuthored,
}

fn bucket_for(thread: &ReviewThread) -> Option<Bucket> {
    let first = thread.first_body();
    if first.contains(BLICK_COMMENT_SIGNATURE) {
        if thread.is_outdated {
            return Some(Bucket::BlickOutdated);
        }
        return Some(if thread.is_resolved {
            Bucket::BlickResolved
        } else {
            Bucket::BlickUnresolved
        });
    }
    // Treat anything authored by a real user as human feedback. Bots that
    // aren't blick (other automations) are ignored — their patterns aren't
    // human review signal and would just add noise.
    if thread.first_author_type() == "User" {
        return Some(Bucket::HumanAuthored);
    }
    None
}

#[derive(Debug, Default, Serialize)]
struct ThreadBuckets {
    blick_resolved: Vec<ReviewThread>,
    blick_unresolved: Vec<ReviewThread>,
    blick_outdated: Vec<ReviewThread>,
    human_authored: Vec<ReviewThread>,
}

impl ThreadBuckets {
    fn push(&mut self, bucket: Bucket, thread: ReviewThread) {
        match bucket {
            Bucket::BlickResolved => self.blick_resolved.push(thread),
            Bucket::BlickUnresolved => self.blick_unresolved.push(thread),
            Bucket::BlickOutdated => self.blick_outdated.push(thread),
            Bucket::HumanAuthored => self.human_authored.push(thread),
        }
    }
    fn total(&self) -> usize {
        self.blick_resolved.len()
            + self.blick_unresolved.len()
            + self.blick_outdated.len()
            + self.human_authored.len()
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
- A bucketed list of review threads from recent merged PRs. Each thread carries the full sequence of comments (opening comment first, then replies), with each comment's author and whether it was authored by a Bot or User:
  - blick_resolved: thread opened by Blick, then resolved by a human (suggestion accepted/addressed). Replies on these threads can carry nuance — a "this is wrong because…" reply means the human resolved it dismissively, not approvingly.
  - blick_unresolved: thread opened by Blick, still open on a merged PR (suggestion likely ignored or rejected). Replies often contain the human's reason.
  - blick_outdated: thread opened by Blick that GitHub flagged as out of date (ambiguous; weight low).
  - human_authored: thread opened by a human reviewer. This is the strongest signal of what Blick MISSED — categories of feedback humans care about that the current skills don't surface.
- A snapshot of every file you are allowed to edit (blick.toml, AGENTS.md / CLAUDE.md, and anything under .blick/skills/).

What to look for:
- Recurring blick suggestion types that are repeatedly *ignored* (blick_unresolved, or blick_resolved with a dismissive human reply) — relax or remove the corresponding rule.
- Recurring blick suggestion types that are repeatedly *accepted* (blick_resolved without a pushback reply) but phrased poorly (verbose, repetitive, off-tone) — tighten the relevant skill.
- Recurring themes in human_authored threads that Blick is missing entirely — propose a new local skill under .blick/skills/<name>/SKILL.md and reference it from a review in blick.toml.
- Disagreements voiced in replies — a clear "no, X is fine because Y" from a human is strong evidence to weaken or scope down the rule that triggered it.

Skill design constraints (very important):
- Any new or edited skill must be GENERIC and reusable across PRs, not a fix for one specific incident. If you can't articulate the rule without naming a particular file, function, or PR, the pattern is not yet generalizable — drop it instead of encoding a one-off.
- New skills must be COHESIVE with the existing setup. Read the snapshot of the current skills and review prompts before proposing anything. Prefer extending an existing skill over adding a new one; only create a new skill when the topic is genuinely orthogonal to what's already there. Match the tone, structure, and granularity of the skills already in place.
- A theme needs at least two independent supporting threads (across different PRs) before it justifies a skill change. One-off feedback is not a pattern.

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
        "blick_resolved": buckets.blick_resolved,
        "blick_unresolved": buckets.blick_unresolved,
        "blick_outdated": buckets.blick_outdated,
        "human_authored": buckets.human_authored,
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
    // Same approach as `review::extract_json_block`: try every `{` as a
    // start, with a depth walker that ignores braces inside JSON string
    // literals, and return the first balanced object that round-trips
    // through `serde_json` as a `LearnProposal`. Robust to prose prefixes
    // that contain stray braces.
    let bytes = raw.as_bytes();
    for (start, _) in raw.match_indices('{') {
        let Some(end) = balanced_object_end(&bytes[start..]) else {
            continue;
        };
        let Some(slice) = raw.get(start..start + end) else {
            continue;
        };
        if serde_json::from_str::<LearnProposal>(slice).is_ok() {
            return Some(slice.to_owned());
        }
    }
    None
}

fn balanced_object_end(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn validate_edits(edits: &[ProposedEdit]) -> Result<(), BlickError> {
    for edit in edits {
        validate_edit_path(&edit.path)?;
    }
    Ok(())
}

/// Reject anything other than a relative path containing only `Normal`
/// components, expressed in git-canonical form (forward slashes only).
///
/// Catches absolute paths (`/etc/passwd`), Windows-drive roots (`C:\foo`),
/// parent traversal (`../foo`), bare `.` segments, embedded NUL bytes, and
/// backslash separators — the last one is critical because the allowlist
/// is matched on string prefixes with `/` and `Path::components()` on
/// Windows happily splits `\` segments, so without this rejection a value
/// like `.blick\skills\..\..\secret` could slip past the allowlist on
/// Windows even though it would be rejected here.
fn validate_edit_path(raw: &str) -> Result<(), BlickError> {
    if raw.is_empty() {
        return Err(BlickError::Config("edit path is empty".to_owned()));
    }
    if raw.as_bytes().contains(&0) {
        return Err(BlickError::Config(format!(
            "edit path contains a NUL byte: {raw:?}"
        )));
    }
    if raw.contains('\\') {
        return Err(BlickError::Config(format!(
            "edit path must use `/` separators, not `\\`: {raw}"
        )));
    }

    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(BlickError::Config(format!(
            "edit path must be relative: {raw}"
        )));
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            std::path::Component::CurDir
            | std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(BlickError::Config(format!(
                    "edit path has a disallowed component ({component:?}): {raw}"
                )));
            }
        }
    }

    if !path_in_allowlist(raw) {
        return Err(BlickError::Config(format!(
            "agent proposed edit outside the allowlist: {raw}"
        )));
    }

    Ok(())
}

/// Match `path` against the static [`EDIT_ALLOWLIST`].
///
/// Callers MUST run [`validate_edit_path`] first; we rely on that step to
/// reject `\` separators, traversal segments, and absolute paths. Given
/// those preconditions, plain string matching against `/`-terminated
/// prefixes is sufficient — and refusing to do it any other way means the
/// allowlist contract stays trivial to read.
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
    let canonical_root = repo_root
        .canonicalize()
        .map_err(|err| BlickError::Io(err))?;

    for edit in edits {
        let target = repo_root.join(&edit.path);

        // Walk up from `target` to find the closest ancestor that already
        // exists on disk; canonicalize that and require it to descend from
        // `canonical_root`. This catches a symlinked intermediate
        // directory regardless of whether the leaf file exists yet.
        let mut probe = target.as_path();
        let anchor = loop {
            if probe.exists() {
                break probe.canonicalize().map_err(|err| BlickError::Io(err))?;
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

/// Like [`gh_api_get`] but retries once on a transient rate-limit / abuse
/// failure, with a short backoff. Search and listing endpoints are the only
/// ones with secondary rate limits this code path realistically hits, so
/// retry is scoped to the explicit callers rather than smeared across every
/// `gh_api` invocation.
async fn gh_api_get_with_retry(api_path: &str) -> Result<String, BlickError> {
    match gh_api_get(api_path) {
        Ok(value) => Ok(value),
        Err(err) => {
            let msg = err.to_string().to_lowercase();
            let transient = msg.contains("rate limit")
                || msg.contains("secondary rate")
                || msg.contains("abuse detection")
                || msg.contains("429");
            if !transient {
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

    fn comment(author_type: &str, body: &str) -> ThreadComment {
        ThreadComment {
            author: "alice".into(),
            author_type: author_type.into(),
            body: body.into(),
        }
    }

    fn thread(comments: Vec<ThreadComment>, resolved: bool, outdated: bool) -> ReviewThread {
        ReviewThread {
            pr_number: 1,
            is_resolved: resolved,
            is_outdated: outdated,
            path: Some("src/lib.rs".into()),
            line: Some(1),
            url: Some("https://example/x".into()),
            comments,
        }
    }

    #[test]
    fn bucket_blick_resolved_and_unresolved() {
        let blick_body = "**foo** body — [Blick](https://github.com/tuist/blick) · default review";
        let resolved = thread(vec![comment("Bot", blick_body)], true, false);
        assert!(matches!(bucket_for(&resolved), Some(Bucket::BlickResolved)));

        let unresolved = thread(vec![comment("Bot", blick_body)], false, false);
        assert!(matches!(
            bucket_for(&unresolved),
            Some(Bucket::BlickUnresolved)
        ));
    }

    #[test]
    fn bucket_human_authored_thread() {
        let human = thread(
            vec![comment(
                "User",
                "We never validate this input — looks like a bug.",
            )],
            false,
            false,
        );
        assert!(matches!(bucket_for(&human), Some(Bucket::HumanAuthored)));
    }

    #[test]
    fn other_bot_threads_are_skipped() {
        let dependabot = thread(
            vec![comment("Bot", "Bumps foo from 1.0 to 1.1")],
            false,
            false,
        );
        assert!(bucket_for(&dependabot).is_none());
    }

    #[test]
    fn outdated_blick_thread_buckets_as_outdated_regardless_of_resolved_state() {
        let blick_body = "x [Blick](https://github.com/tuist/blick) y";
        let mut t = thread(vec![comment("Bot", blick_body)], true, true);
        assert!(matches!(bucket_for(&t), Some(Bucket::BlickOutdated)));
        t.is_outdated = false;
        t.is_resolved = false;
        assert!(matches!(bucket_for(&t), Some(Bucket::BlickUnresolved)));
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
    fn validate_edit_path_rejects_backslash_separators() {
        // Windows-style separators are rejected outright so `..\..` style
        // traversal can't sneak past `path_in_allowlist`'s `/`-anchored
        // string match on platforms where `Path::components()` recognises
        // `\`. This matches blick's git-canonical edit path contract.
        assert!(validate_edit_path(".blick\\skills\\foo\\SKILL.md").is_err());
        assert!(validate_edit_path(".blick/skills\\..\\..\\secret").is_err());
    }

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
    fn validate_edit_path_rejects_absolute_and_special_components() {
        // Absolute path → rejected even though the suffix would be in the
        // allowlist if it were relative.
        assert!(validate_edit_path("/etc/passwd").is_err());
        // Parent-traversal segments are rejected ahead of the allowlist
        // check so they can't smuggle a path out of `.blick/skills/` past
        // `path_in_allowlist`.
        assert!(validate_edit_path(".blick/skills/foo/../../bar").is_err());
        // Leading `./` produces a `CurDir` component, which we reject so
        // the path stays in a canonical form.
        assert!(validate_edit_path("./blick.toml").is_err());
        // Empty path and NUL bytes are not file paths.
        assert!(validate_edit_path("").is_err());
        assert!(validate_edit_path(".blick/skills/foo/SKILL.md\0evil").is_err());
        // Sanity check that a clean relative path still passes. Mid-path
        // `./` is normalized away by `Path::components()` and is fine.
        assert!(validate_edit_path(".blick/skills/foo/SKILL.md").is_ok());
        assert!(validate_edit_path(".blick/skills/./foo/SKILL.md").is_ok());
        assert!(validate_edit_path("blick.toml").is_ok());
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
    fn percent_encode_query_escapes_separators_and_keeps_unreserved() {
        // Spaces, colons, and `>` (used by `merged:>=DATE`) must be encoded
        // so `gh api` doesn't have to interpret them.
        assert_eq!(
            percent_encode_query("repo:tuist/blick is:pr is:merged merged:>=2026-04-22"),
            "repo%3Atuist%2Fblick%20is%3Apr%20is%3Amerged%20merged%3A%3E%3D2026-04-22"
        );
        // Unreserved characters per RFC 3986 pass through untouched.
        assert_eq!(percent_encode_query("a-Z_0.9~"), "a-Z_0.9~");
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
    fn extract_json_block_handles_prose_and_braces_in_strings() {
        // Plain text wrapping a single JSON object — pull out just the object.
        assert_eq!(
            extract_json_block("noise before {\"a\":1} noise after").as_deref(),
            Some("{\"a\":1}")
        );
        // Nested braces are tracked by depth so the first balanced object wins.
        assert_eq!(
            extract_json_block("{\"a\":{\"b\":2},\"c\":3}").as_deref(),
            Some("{\"a\":{\"b\":2},\"c\":3}")
        );
        // No object at all → None.
        assert!(extract_json_block("plain text, no json").is_none());
    }

    #[test]
    fn parse_proposal_rejects_non_json() {
        assert!(parse_proposal("definitely not json").is_err());
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
