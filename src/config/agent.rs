//! Agent (LLM runner) selection and configuration.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Which CLI agent backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[value(name = "claude")]
    Claude,
    #[value(name = "codex")]
    Codex,
    #[value(name = "opencode")]
    Opencode,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }

    /// Default model identifier for each agent. `None` means "let the agent
    /// pick its own default."
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("anthropic/claude-sonnet-4-5"),
            Self::Codex => Some("openai/gpt-5"),
            Self::Opencode => Some("anthropic/claude-sonnet-4-5"),
        }
    }
}

/// `[agent]` block in `blick.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub kind: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_serde() {
        for k in [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode] {
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
    }

    #[test]
    fn every_kind_has_a_default_model() {
        // We rely on this in `ConfigFile::starter` — a starter config is
        // useless without a model. Lock the invariant in.
        assert!(AgentKind::Claude.default_model().is_some());
        assert!(AgentKind::Codex.default_model().is_some());
        assert!(AgentKind::Opencode.default_model().is_some());
    }
}
