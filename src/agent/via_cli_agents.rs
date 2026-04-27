use std::fs;

use async_trait::async_trait;
use cli_agents::{ClaudeOptions, CliName, ProviderOptions, RunOptions, run};

use crate::agent::{AgentRunner, RunOutput, strip_provider_prefix};
use crate::config::{AgentConfig, AgentKind};
use crate::error::BlickError;

pub struct CliAgentsRunner {
    cli: CliName,
    model: Option<String>,
}

impl CliAgentsRunner {
    pub fn new(config: &AgentConfig) -> Result<Self, BlickError> {
        let cli = match config.kind {
            AgentKind::Claude => CliName::Claude,
            AgentKind::Codex => CliName::Codex,
            AgentKind::Opencode => {
                return Err(BlickError::Config(
                    "opencode is not handled by the cli-agents adapter".to_owned(),
                ));
            }
        };
        Ok(Self {
            cli,
            model: config
                .model
                .as_deref()
                .map(strip_provider_prefix)
                .map(str::to_owned),
        })
    }
}

#[async_trait]
impl AgentRunner for CliAgentsRunner {
    async fn run(&self, system_prompt: &str, user_prompt: &str) -> Result<RunOutput, BlickError> {
        let cwd = sandbox_cwd()?;
        let opts = RunOptions {
            cli: Some(self.cli),
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
        let result = handle
            .result
            .await
            .map_err(|error| BlickError::Api(format!("agent task failed to join: {error}")))??;

        let stderr = result.stderr.clone().unwrap_or_default();

        if let Some(text) = result.text
            && !text.trim().is_empty()
        {
            return Ok(RunOutput {
                stdout: text.clone(),
                text,
                stderr,
            });
        }

        Err(BlickError::Api(format!(
            "{} produced no text output: {}",
            self.cli,
            if stderr.trim().is_empty() {
                "no stderr captured"
            } else {
                stderr.trim()
            }
        )))
    }
}

fn sandbox_cwd() -> Result<String, BlickError> {
    let path = std::env::temp_dir().join("blick-review");
    fs::create_dir_all(&path)?;
    Ok(path.to_string_lossy().into_owned())
}
