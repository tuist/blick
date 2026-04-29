//! `blick render` — render a stored run to a chosen output format.

use std::path::PathBuf;

use crate::cli::RenderArgs;
use crate::error::BlickError;
use crate::render::{self, RenderContext};
use crate::run_record::resolve_run_dir;

pub fn run(args: RenderArgs) -> Result<(), BlickError> {
    let repo_root = args
        .repo
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .canonicalize()?;
    let run_dir = resolve_run_dir(&repo_root, args.run.as_deref())?;
    let ctx = RenderContext {
        head_sha: args.head_sha.as_deref(),
        commit_sha: args.head_sha.as_deref(),
    };
    let rendered = render::render(&run_dir, args.format, ctx)?;
    println!("{rendered}");
    Ok(())
}
