//! `blick learn` — periodic self-improvement pass.

use crate::cli::LearnArgs as CliLearnArgs;
use crate::error::BlickError;
use crate::learn::{self, LearnArgs};

pub async fn run(args: CliLearnArgs) -> Result<(), BlickError> {
    learn::learn(LearnArgs {
        repo: args.repo,
        dry_run: args.dry_run,
        force: args.force,
        lookback_days: args.lookback_days,
        min_signal: args.min_signal,
    })
    .await
}
