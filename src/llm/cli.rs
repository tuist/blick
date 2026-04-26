use std::fs;

use async_trait::async_trait;
use cli_agents::{ClaudeOptions, CliName, ProviderOptions, RunOptions, run};

use crate::config::{LlmConfig, ProviderKind};
use crate::error::BlickError;
use crate::llm::ReviewClient;

pub struct CliReviewClient {
    cli: Option<CliName>,
    model: Option<String>,
}

impl CliReviewClient {
    pub fn new(config: &LlmConfig) -> Self {
        Self {
            cli: provider_cli(config.provider),
            model: config.model.clone(),
        }
    }
}

#[async_trait]
impl ReviewClient for CliReviewClient {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError> {
        let cwd = sandbox_cwd()?;
        let opts = RunOptions {
            cli: self.cli,
            task: user_prompt.to_owned(),
            system_prompt: Some(system_prompt.to_owned()),
            cwd: Some(cwd),
            model: self.model.clone(),
            skip_permissions: true,
            providers: Some(ProviderOptions {
                claude: Some(ClaudeOptions {
                    max_turns: Some(1),
                    ..ClaudeOptions::default()
                }),
                ..ProviderOptions::default()
            }),
            ..RunOptions::default()
        };

        let handle = run(opts, None);
        let result = handle.result.await.map_err(|error| {
            BlickError::Api(format!("local CLI task failed to join: {error}"))
        })??;

        if let Some(text) = result.text {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }

        let stderr = result
            .stderr
            .unwrap_or_else(|| "no stderr captured".to_owned());
        Err(BlickError::Api(format!(
            "local {} review run produced no text output: {}",
            self.cli
                .map(|cli| cli.to_string())
                .unwrap_or_else(|| "cli".to_owned()),
            stderr
        )))
    }
}

fn provider_cli(provider: ProviderKind) -> Option<CliName> {
    match provider {
        ProviderKind::Auto => None,
        ProviderKind::Claude => Some(CliName::Claude),
        ProviderKind::Codex => Some(CliName::Codex),
        ProviderKind::OpenAI | ProviderKind::Anthropic => None,
    }
}

fn sandbox_cwd() -> Result<String, BlickError> {
    let path = std::env::temp_dir().join("blick-review");
    fs::create_dir_all(&path)?;
    Ok(path.to_string_lossy().into_owned())
}
