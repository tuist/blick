//! `blick review` — run a single `(scope, review)` pair against a diff and
//! parse the agent's response into a [`ReviewReport`].
//!
//! Submodules:
//! - [`types`]  — `ReviewReport`, `Finding`, `ReviewOutcome`
//! - [`parse`]  — agent text → `ReviewReport`
//! - [`prompt`] — assembles system + user prompts
//! - [`render`] — human-facing rendering for terminal output

mod parse;
mod prompt;
mod render;
mod types;

use crate::agent::AgentRunner;
use crate::config::{ReviewEntry, ScopeConfig};
use crate::error::BlickError;
use crate::git::DiffBundle;

pub use parse::parse_report;
pub use render::render_report;
pub use types::{Finding, FocusDiff, ReviewOutcome, ReviewReport};

use prompt::{build_system_prompt, build_user_prompt, collect_prompt_addendum, collect_skills};

/// Run a single review (named bundle of skills) for one scope.
pub async fn run_review(
    runner: &dyn AgentRunner,
    scope: &ScopeConfig,
    review: &ReviewEntry,
    base: &str,
    diff: &DiffBundle,
    focus: Option<&FocusDiff>,
) -> Result<ReviewOutcome, BlickError> {
    let skills = collect_skills(scope, review)?;
    let prompt_addendum = collect_prompt_addendum(scope, review)?;
    let system_prompt = build_system_prompt(&skills, prompt_addendum.as_deref(), focus.is_some());
    let user_prompt = build_user_prompt(base, diff, focus);

    let run = runner.run(&system_prompt, &user_prompt).await?;
    let report = parse_report(&run.text)?;
    Ok(ReviewOutcome {
        report,
        run,
        system_prompt,
    })
}
