//! `blick publish` — post a stored run to GitHub (Check Runs + PR review).

use std::path::PathBuf;

use crate::cli::PublishArgs as CliPublishArgs;
use crate::error::BlickError;
use crate::publish::{self, PublishArgs};

pub fn run(args: CliPublishArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    publish::publish(
        &repo_root,
        PublishArgs {
            run: args.run,
            head_sha: args.head_sha,
            repo: args.gh_repo,
            pr: args.pr,
        },
    )
}
