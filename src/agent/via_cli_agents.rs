use std::fs;

use async_trait::async_trait;
use cli_agents::{ClaudeOptions, CliName, ProviderOptions, RunOptions, run};

use crate::agent::{AgentRunner, RunOutput, strip_provider_prefix};
use crate::config::{AgentConfig, AgentKind};
use crate::error::BlickError;

pub struct CliAgentsRunner {
    cli: CliName,
    model: Option<String>,
    binary: Option<String>,
}

impl CliAgentsRunner {
    pub fn new(config: &AgentConfig) -> Result<Self, BlickError> {
        let cli = match config.kind {
            AgentKind::Claude => CliName::Claude,
            AgentKind::Codex => CliName::Codex,
            AgentKind::Opencode | AgentKind::Gemini => {
                return Err(BlickError::Config(format!(
                    "{} is not handled by the cli-agents adapter",
                    config.kind.as_str()
                )));
            }
        };
        if !config.args.is_empty() {
            return Err(BlickError::Config(format!(
                "`agent.args` is not yet supported for `{}`: \
                 the upstream `cli-agents` crate does not expose an \
                 `extra_args` field on its Claude/Codex options (verified \
                 against 0.2.10, 0.2.11, and main). Use `kind = \"opencode\"` \
                 or `kind = \"gemini\"` if you need to pass custom CLI flags. \
                 Track: https://github.com/skoppisetty/cli-agents-rs",
                config.kind.as_str()
            )));
        }
        Ok(Self {
            cli,
            model: config
                .model
                .as_deref()
                .map(strip_provider_prefix)
                .map(str::to_owned),
            binary: config.binary.clone(),
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
            executable_path: self.binary.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_binary_override() {
        let runner = CliAgentsRunner::new(&AgentConfig {
            kind: AgentKind::Claude,
            model: None,
            binary: Some("/opt/homebrew/bin/claude".to_owned()),
            args: Vec::new(),
        })
        .expect("claude config builds");
        assert_eq!(runner.binary.as_deref(), Some("/opt/homebrew/bin/claude"));
    }

    #[test]
    fn defaults_binary_to_none_for_cli_discovery() {
        let runner = CliAgentsRunner::new(&AgentConfig {
            kind: AgentKind::Claude,
            model: None,
            binary: None,
            args: Vec::new(),
        })
        .expect("claude config builds");
        // None tells cli-agents to discover the CLI on PATH, matching prior behavior.
        assert!(runner.binary.is_none());
    }

    #[test]
    fn rejects_args_until_upstream_support_lands() {
        // The upstream `cli-agents` crate does not yet expose an `extra_args`
        // hook for Claude/Codex. Surfacing this as a config error is friendlier
        // than silently ignoring a documented option.
        let result = CliAgentsRunner::new(&AgentConfig {
            kind: AgentKind::Claude,
            model: None,
            binary: None,
            args: vec!["--json-schema".to_owned(), "{}".to_owned()],
        });
        match result {
            Ok(_) => panic!("args should be rejected for claude until cli-agents adds extra_args"),
            Err(BlickError::Config(message)) => assert!(
                message.contains("agent.args"),
                "error should mention the offending field, got: {message}"
            ),
            Err(other) => panic!("expected BlickError::Config, got: {other:?}"),
        }
    }
}
