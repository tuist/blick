//! `blick publish` — turn a completed run directory into GitHub side-effects:
//! Check Runs, a PR review, or (if the run dir is missing) a one-line
//! "didn't run" notice.
//!
//! Submodules:
//! - [`context`] resolves repo / head SHA / PR from CLI args + env
//! - [`gh`]      shells out to the `gh` CLI for API calls
//! - [`notice`]  builds the "didn't run" notice body
//! - [`payload`] massages review payloads when GitHub rejects parts of them

mod context;
mod gh;
mod notice;
mod payload;

use std::path::Path;

use crate::error::BlickError;
use crate::render::{self, Format, RenderContext};

use self::context::{PublishContext, resolve_context, workflow_run_url};
use self::gh::gh_api_post;
use self::notice::build_review_failed_notice;
use self::payload::strip_inline_comments;

#[derive(Debug, Default)]
pub struct PublishArgs {
    pub run: Option<String>,
    pub head_sha: Option<String>,
    pub repo: Option<String>,
    pub pr: Option<u64>,
}

pub fn publish(repo_root: &Path, args: PublishArgs) -> Result<(), BlickError> {
    let ctx = resolve_context(args.head_sha.as_deref(), args.repo.as_deref(), args.pr)?;

    let runs_root = repo_root.join(".blick").join("runs");
    let probe = match args.run.as_deref() {
        None | Some("latest") => runs_root.join("latest"),
        Some(other) => runs_root.join(other),
    };
    if !probe.exists() {
        post_review_failed_notice(&ctx, &probe);
        return Ok(());
    }

    let run_dir = crate::run_record::resolve_run_dir(repo_root, args.run.as_deref())?;
    post_check_runs(&ctx, &run_dir)?;
    post_pr_review(&ctx, &run_dir)?;
    Ok(())
}

/// `blick review` failed before writing a manifest. In CI (`if: always()`
/// publish step) we still want the PR author to see *something* explaining
/// the failure rather than a red workflow box and silence on the PR. Post a
/// one-line notice and exit cleanly so the publish step doesn't double-fail
/// the workflow.
fn post_review_failed_notice(ctx: &PublishContext, probe: &Path) {
    eprintln!(
        "ℹ no run directory at {} — `blick review` likely failed before writing a manifest",
        probe.display()
    );
    let Some(pr) = ctx.pr else {
        return;
    };
    let body = build_review_failed_notice(ctx, workflow_run_url().as_deref());
    let payload = serde_json::json!({ "body": body }).to_string();
    let endpoint = format!("repos/{}/issues/{pr}/comments", ctx.repo);
    match gh_api_post(&endpoint, &payload) {
        Ok(()) => eprintln!("✓ posted review-failed notice on #{pr}"),
        Err(err) => eprintln!("⚠ failed to post review-failed notice: {err}"),
    }
}

fn post_check_runs(ctx: &PublishContext, run_dir: &Path) -> Result<(), BlickError> {
    let render_ctx = RenderContext {
        head_sha: Some(&ctx.head_sha),
        commit_sha: Some(&ctx.head_sha),
    };

    let check_runs = render::render(run_dir, Format::CheckRun, render_ctx)?;
    let mut posted = 0usize;
    for line in check_runs.lines() {
        if line.trim().is_empty() {
            continue;
        }
        gh_api_post(&format!("repos/{}/check-runs", ctx.repo), line)?;
        posted += 1;
    }
    eprintln!(
        "✓ posted {posted} check run{}",
        if posted == 1 { "" } else { "s" }
    );
    Ok(())
}

fn post_pr_review(ctx: &PublishContext, run_dir: &Path) -> Result<(), BlickError> {
    let Some(pr) = ctx.pr else {
        eprintln!("ℹ no PR context detected; skipped PR review post");
        return Ok(());
    };

    // Don't fail the whole publish step on a counting error — the check
    // runs are already up. Surface the warning and skip just the review
    // post so the user still gets the per-review status.
    match render::total_findings(run_dir) {
        Ok(0) => {
            eprintln!(
                "ℹ no findings; skipping PR review post (check runs convey the pass status)"
            );
            Ok(())
        }
        Ok(_) => post_review_with_inline_fallback(ctx, run_dir, pr),
        Err(err) => {
            eprintln!(
                "⚠ could not count findings ({err}); skipping PR review post but check runs are already up"
            );
            Ok(())
        }
    }
}

fn post_review_with_inline_fallback(
    ctx: &PublishContext,
    run_dir: &Path,
    pr: u64,
) -> Result<(), BlickError> {
    let render_ctx = RenderContext {
        head_sha: Some(&ctx.head_sha),
        commit_sha: Some(&ctx.head_sha),
    };
    let review = render::render(run_dir, Format::GithubReview, render_ctx)?;
    let endpoint = format!("repos/{}/pulls/{pr}/reviews", ctx.repo);
    match gh_api_post(&endpoint, &review) {
        Ok(()) => {
            eprintln!("✓ posted PR review on #{pr}");
            Ok(())
        }
        Err(BlickError::Api(msg)) if msg.contains("Line could not be resolved") => {
            // GitHub rejected at least one inline comment because its line
            // isn't in the PR diff as GitHub computes it. Retry without
            // inline comments — those findings are folded into the body —
            // so we still post *something* rather than dropping the review.
            eprintln!("⚠ inline comments rejected ({msg}); retrying without them");
            let fallback = strip_inline_comments(&review)?;
            gh_api_post(&endpoint, &fallback)?;
            eprintln!("✓ posted PR review on #{pr} (without inline comments)");
            Ok(())
        }
        Err(err) => Err(err),
    }
}
