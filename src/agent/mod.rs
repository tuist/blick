mod opencode;
mod via_cli_agents;

use async_trait::async_trait;

use crate::config::{AgentConfig, AgentKind};
use crate::error::BlickError;

/// Captured output from a single agent invocation.
#[derive(Debug, Default, Clone)]
pub struct RunOutput {
    /// The text payload the agent emitted (the JSON we'll parse into a report).
    pub text: String,
    /// Full captured stdout (may overlap with `text` when the agent prints
    /// only the result; useful for debugging when it doesn't).
    pub stdout: String,
    /// Full captured stderr.
    pub stderr: String,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(&self, system_prompt: &str, user_prompt: &str) -> Result<RunOutput, BlickError>;
}

pub fn runner_for(config: &AgentConfig) -> Result<Box<dyn AgentRunner>, BlickError> {
    match config.kind {
        AgentKind::Claude | AgentKind::Codex => {
            Ok(Box::new(via_cli_agents::CliAgentsRunner::new(config)?))
        }
        AgentKind::Opencode => Ok(Box::new(opencode::OpencodeRunner::new(config))),
    }
}

/// Strip the `provider/` prefix from a models.dev-style model id.
pub fn strip_provider_prefix(model: &str) -> &str {
    model.split_once('/').map(|(_, m)| m).unwrap_or(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_models_dev_prefix() {
        assert_eq!(
            strip_provider_prefix("anthropic/claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
        assert_eq!(strip_provider_prefix("openai/gpt-5"), "gpt-5");
        assert_eq!(strip_provider_prefix("gpt-5"), "gpt-5");
    }
}
