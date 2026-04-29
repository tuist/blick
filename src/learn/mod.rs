//! `blick learn` — periodic self-improvement pass.
//!
//! Reads merged-PR review activity from the past N days, buckets each blick
//! line-comment thread by whether the human author resolved it, asks the
//! agent to pattern-match recurring failures and successes, and proposes
//! edits to the review setup itself (skills, prompts, agent instructions).
//! Output is a single rolling draft PR on a fixed branch (default
//! `blick/learn`); subsequent runs force-push fresh proposals onto the same
//! branch instead of opening a second PR.
//!
//! Submodules:
//! - [`setup`]      — pick the root scope, load `[learn]`, find `owner/repo`
//! - [`prs`]        — search merged PRs in the lookback window
//! - [`threads`]    — fetch and parse review threads via GraphQL
//! - [`buckets`]    — categorize threads (blick / human, resolved / not)
//! - [`proposal`]   — agent-output schema and lenient JSON parsing
//! - [`allowlist`]  — files learn may read/write + path safety
//! - [`agent_pass`] — system + user prompts and the agent invocation
//! - [`publish`]    — apply edits, commit, push, and upsert the rolling PR
//! - [`git`] / [`gh`] — subprocess wrappers

mod agent_pass;
mod allowlist;
mod buckets;
mod gh;
mod git;
mod proposal;
mod prs;
mod publish;
mod setup;
mod threads;

use std::path::PathBuf;

use crate::error::BlickError;
use crate::scope::load_scopes;

use agent_pass::run_agent_pass;
use allowlist::{collect_allowlist_files, validate_edits};
use buckets::{ThreadBuckets, bucket_for};
use prs::fetch_recent_merged_prs;
use publish::apply_and_publish;
use setup::{detect_github_repo, load_learn_config, pick_root_scope};
use threads::fetch_review_threads;

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
