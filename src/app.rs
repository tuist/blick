//! Top-level CLI entry point: parse arguments and dispatch to the
//! corresponding command module under [`crate::commands`].

use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::commands::{init, learn_cmd, publish_cmd, render_cmd, review, show_config};
use crate::error::BlickError;

pub async fn run() -> Result<(), BlickError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init(args) => init::run(args),
        Commands::Review(args) => review::run(args).await,
        Commands::Config(args) => show_config::run(args),
        Commands::Render(args) => render_cmd::run(args),
        Commands::Publish(args) => publish_cmd::run(args),
        Commands::Learn(args) => learn_cmd::run(args).await,
    }
}
