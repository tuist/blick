use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::AgentKind;

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
    /// Write a starter blick.toml.
    Init(InitArgs),
    /// Run one or all reviews against the current diff.
    Review(ReviewArgs),
    /// Show the resolved configuration and which file each value came from.
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, value_enum, default_value_t = AgentKind::Claude)]
    pub agent: AgentKind,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long, default_value = "blick.toml")]
    pub path: PathBuf,

    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ReviewArgs {
    /// Optional review name; if omitted, runs every review defined in the
    /// scopes that own the changed files.
    pub name: Option<String>,

    /// Override the agent for this run (e.g. `--agent claude`).
    #[arg(long, value_enum)]
    pub agent: Option<AgentKind>,

    /// Override the model (models.dev syntax: `provider/model`).
    #[arg(long)]
    pub model: Option<String>,

    /// Git base revision.
    #[arg(long)]
    pub base: Option<String>,

    /// Emit JSON.
    #[arg(long)]
    pub json: bool,

    /// Print each task's captured stdout/stderr (line-prefixed) on stderr
    /// when it completes. By default only a one-line summary is emitted.
    #[arg(long)]
    pub stream: bool,

    /// Override `defaults.max_concurrency`.
    #[arg(long)]
    pub max_concurrency: Option<usize>,

    /// Repository root (defaults to current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Show provenance for every resolved field.
    #[arg(long)]
    pub explain: bool,

    /// Repository root (defaults to current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}
