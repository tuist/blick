use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::AgentKind;
use crate::render::Format;

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
    /// Render a previous run for a downstream destination (PR review, check
    /// runs, markdown summary).
    Render(RenderArgs),
    /// Publish the latest run to GitHub: posts per-review check runs and a
    /// resolvable PR review. Auto-detects PR context from GitHub Actions.
    Publish(PublishArgs),
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

#[derive(Debug, Args)]
pub struct RenderArgs {
    /// Output format. `github-review` and `check-run` produce JSON for the
    /// matching GitHub API endpoints; `github-summary` produces markdown.
    #[arg(long, value_enum, default_value_t = Format::GithubSummary)]
    pub format: Format,

    /// Run id (e.g. `20260427T123456Z`), `latest`, or a directory path under
    /// `.blick/runs/`. Defaults to `latest`.
    #[arg(long)]
    pub run: Option<String>,

    /// Commit SHA the report applies to. Required for `github-review` and
    /// `check-run`. Pass the PR head SHA in CI.
    #[arg(long)]
    pub head_sha: Option<String>,

    /// Repository root (defaults to current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PublishArgs {
    /// Run id, `latest`, or a directory path. Defaults to `latest`.
    #[arg(long)]
    pub run: Option<String>,

    /// Commit SHA the report applies to. Auto-detected from
    /// `GITHUB_EVENT_PATH` when running in GitHub Actions.
    #[arg(long)]
    pub head_sha: Option<String>,

    /// `owner/repo` slug. Auto-detected from `GITHUB_REPOSITORY`.
    #[arg(long, value_name = "OWNER/REPO")]
    pub gh_repo: Option<String>,

    /// PR number. Auto-detected from `GITHUB_EVENT_PATH`. When omitted and
    /// no PR context is detected, only check runs are posted.
    #[arg(long)]
    pub pr: Option<u64>,

    /// Repository root on disk (defaults to current directory).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}
