use std::fs;
use std::path::Path;

use crate::cli::{Cli, Commands, InitArgs, ReviewArgs};
use crate::config::Config;
use crate::error::BlickError;
use crate::git::collect_diff;
use crate::llm::client_from_config;
use crate::review::{ReviewReport, build_prompt, parse_report, render_report};
use clap::Parser;

pub async fn run() -> Result<(), BlickError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init(args) => init(args),
        Commands::Review(args) => review(args).await,
    }
}

fn init(args: InitArgs) -> Result<(), BlickError> {
    if args.path.exists() && !args.force {
        return Err(BlickError::Config(format!(
            "{} already exists. Re-run with --force to overwrite it.",
            args.path.display()
        )));
    }

    let config = Config::for_provider(args.provider, args.model);
    let rendered = config.to_toml()?;
    fs::write(&args.path, rendered)?;

    println!("Created {}", args.path.display());
    Ok(())
}

async fn review(args: ReviewArgs) -> Result<(), BlickError> {
    let mut config = load_config(&args.config)?;
    if let Some(base) = args.base {
        config.review.base = base;
    }

    let diff = collect_diff(&config.review.base, config.review.max_diff_bytes)?;
    if diff.diff.is_empty() {
        let report = ReviewReport::empty(format!(
            "No tracked changes found relative to {}.",
            config.review.base
        ));
        println!("{}", render_report(&report, args.json)?);
        return Ok(());
    }

    let prompt = build_prompt(&config.review.base, &diff);
    let client = client_from_config(&config)?;
    let raw = client.review(prompt.system, &prompt.user).await?;
    let report = parse_report(&raw)?;

    println!("{}", render_report(&report, args.json)?);
    Ok(())
}

fn load_config(path: &Path) -> Result<Config, BlickError> {
    if path.exists() {
        return Config::load(path);
    }

    Ok(Config::default())
}
