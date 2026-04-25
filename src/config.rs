use std::env;
use std::fs;
use std::path::Path;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::error::BlickError;
use crate::workflow::ReviewWorkflow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub review: ReviewConfig,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, BlickError> {
        let raw = fs::read_to_string(path)?;
        toml::from_str(&raw).map_err(|error| {
            BlickError::Config(format!("failed to parse {}: {error}", path.display()))
        })
    }

    pub fn to_toml(&self) -> Result<String, BlickError> {
        toml::to_string_pretty(self)
            .map_err(|error| BlickError::Config(format!("failed to serialize config: {error}")))
    }

    pub fn for_provider(provider: ProviderKind, model: Option<String>) -> Self {
        Self {
            llm: LlmConfig {
                model: model.or_else(|| provider.default_model().map(ToOwned::to_owned)),
                provider,
                ..LlmConfig::default()
            },
            review: ReviewConfig::default(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            review: ReviewConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum ProviderKind {
    #[serde(rename = "auto")]
    #[value(name = "auto")]
    Auto,
    #[serde(rename = "openai")]
    #[value(name = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    #[value(name = "anthropic")]
    Anthropic,
    #[serde(rename = "claude")]
    #[value(name = "claude")]
    Claude,
    #[serde(rename = "codex")]
    #[value(name = "codex")]
    Codex,
}

impl ProviderKind {
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::OpenAi => Some("gpt-5"),
            Self::Anthropic => Some("claude-sonnet-4-5"),
            Self::Claude => None,
            Self::Codex => None,
        }
    }

    pub fn default_api_key_env(self) -> Option<&'static str> {
        match self {
            Self::Auto | Self::Claude | Self::Codex => None,
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Auto | Self::Claude | Self::Codex => None,
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub provider: ProviderKind,
    #[serde(default = "default_model")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl LlmConfig {
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn api_key_env(&self) -> Option<&str> {
        self.api_key_env
            .as_deref()
            .or_else(|| self.provider.default_api_key_env())
    }

    pub fn base_url(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .or_else(|| self.provider.default_base_url())
    }

    pub fn api_key(&self) -> Result<Option<String>, BlickError> {
        let Some(env_name) = self.api_key_env() else {
            return Ok(None);
        };

        env::var(env_name).map(Some).map_err(|_| {
            BlickError::MissingApiKey(format!(
                "expected {} to be set for provider {}",
                env_name,
                self.provider.as_str()
            ))
        })
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::default(),
            model: default_model(),
            api_key_env: None,
            base_url: None,
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_max_diff_bytes")]
    pub max_diff_bytes: usize,
    #[serde(default, skip_serializing_if = "ReviewWorkflow::is_default")]
    pub workflow: ReviewWorkflow,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            base: default_base(),
            max_diff_bytes: default_max_diff_bytes(),
            workflow: ReviewWorkflow::default(),
        }
    }
}

impl Default for ProviderKind {
    fn default() -> Self {
        Self::OpenAi
    }
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

fn default_model() -> Option<String> {
    ProviderKind::default()
        .default_model()
        .map(ToOwned::to_owned)
}

fn default_base() -> String {
    "HEAD".to_owned()
}

fn default_max_diff_bytes() -> usize {
    120_000
}

fn default_max_output_tokens() -> u32 {
    2_048
}

#[cfg(test)]
mod tests {
    use super::{Config, ProviderKind};

    #[test]
    fn serializes_default_provider_config() {
        let config = Config::default();
        let rendered = config.to_toml().expect("config should serialize");

        assert!(rendered.contains("provider = \"openai\""));
        assert!(rendered.contains("model = \"gpt-5\""));
    }

    #[test]
    fn picks_provider_specific_default_model() {
        let config = Config::for_provider(ProviderKind::Anthropic, None);
        assert_eq!(config.llm.model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn local_provider_can_omit_model() {
        let config = Config::for_provider(ProviderKind::Claude, None);
        assert_eq!(config.llm.model, None);
    }

    #[test]
    fn omits_default_workflow_from_serialized_config() {
        let config = Config::default();
        let rendered = config.to_toml().expect("config should serialize");

        assert!(!rendered.contains("[[review.workflow.steps]]"));
    }
}
