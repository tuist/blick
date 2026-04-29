//! Rendering of completed blick runs into formats suitable for posting back
//! to GitHub: PR reviews, Check Runs, and plain-markdown summaries.
//!
//! The submodule layout mirrors the output formats:
//! - [`github_review`] — single PR review with inline comments
//! - [`check_run`]      — one Check Run per `(scope, review)` pair
//! - [`github_summary`] — plain-markdown rollup
//!
//! Cross-cutting helpers live in [`badges`] (severity → visual indicator),
//! [`details`] (`<details>` block helpers), [`markers`] (last-reviewed SHA
//! markers embedded in review bodies), [`origin`] (origin labels), and
//! [`diff_lines`] (PR-diff line index for filtering inline comments).

pub mod diff_lines;

mod badges;
mod check_run;
mod details;
mod github_review;
mod github_summary;
mod markers;
mod origin;

use std::path::Path;

use clap::ValueEnum;

use crate::error::BlickError;
use crate::run_record::{TaskRecord, list_task_records};

pub use markers::{
    LAST_REVIEWED_MARKER_PREFIX, LAST_REVIEWED_MARKER_SUFFIX, parse_last_reviewed_marker,
};

/// Output format for a rendered run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// JSON for `POST /repos/.../pulls/{n}/reviews` — bundles every
    /// finding in the run into a single review with line comments the PR
    /// author can mark resolved.
    #[value(name = "github-review")]
    GithubReview,
    /// JSON Lines for `POST /repos/.../check-runs` — one Check Run per
    /// `(scope, review)` pair so each shows up as its own check on the PR.
    #[value(name = "check-run")]
    CheckRun,
    /// Plain markdown summary, suitable for `gh pr comment --body-file -`
    /// or any chat/email integration.
    #[value(name = "github-summary")]
    GithubSummary,
}

/// SHA context required by some formats (e.g. `github-review` needs the PR
/// head commit; `check-run` needs the same for the `head_sha` field).
#[derive(Debug, Clone)]
pub struct RenderContext<'a> {
    pub head_sha: Option<&'a str>,
    pub commit_sha: Option<&'a str>,
}

/// Render a completed run directory into the requested format.
pub fn render(
    run_dir: &Path,
    format: Format,
    ctx: RenderContext<'_>,
) -> Result<String, BlickError> {
    let records = list_task_records(run_dir)?;
    match format {
        Format::GithubReview => github_review::render_github_review(&records, ctx),
        Format::CheckRun => check_run::render_check_runs(&records, ctx),
        Format::GithubSummary => Ok(github_summary::render_github_summary(&records)),
    }
}

/// Sum of findings across a slice of persisted task records.
pub(super) fn count_findings(records: &[TaskRecord]) -> usize {
    records.iter().map(|r| r.report.findings.len()).sum()
}

/// Sum of findings across every persisted task in a run.
pub fn total_findings(run_dir: &Path) -> Result<usize, BlickError> {
    Ok(count_findings(&list_task_records(run_dir)?))
}
