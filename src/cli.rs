use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::ProviderKind;

const CLI_VERSION: &str = match option_env!("BLICK_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(
    name = "blick",
    version = CLI_VERSION,
    about = "A configurable code review agent."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Init(InitArgs),
    Review(ReviewArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, value_enum, default_value_t = ProviderKind::OpenAi)]
    pub provider: ProviderKind,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long, default_value = "blick.toml")]
    pub path: PathBuf,

    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ReviewArgs {
    #[arg(long, default_value = "blick.toml")]
    pub config: PathBuf,

    #[arg(long)]
    pub base: Option<String>,

    #[arg(long)]
    pub json: bool,
}
