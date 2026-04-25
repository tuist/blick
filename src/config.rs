use std::env;
use std::fs;
use std::path::Path;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::error::BlickError;

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
                model: model.unwrap_or_else(|| provider.default_model().to_owned()),
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
    #[serde(rename = "openai")]
    #[value(name = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    #[value(name = "anthropic")]
    Anthropic,
}

impl ProviderKind {
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-5",
            Self::Anthropic => "claude-sonnet-4-5",
        }
    }

    pub fn default_api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub provider: ProviderKind,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl LlmConfig {
    pub fn api_key_env(&self) -> &str {
        self.api_key_env
            .as_deref()
            .unwrap_or_else(|| self.provider.default_api_key_env())
    }

    pub fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or_else(|| self.provider.default_base_url())
    }

    pub fn api_key(&self) -> Result<String, BlickError> {
        env::var(self.api_key_env()).map_err(|_| {
            BlickError::MissingApiKey(format!(
                "expected {} to be set for provider {}",
                self.api_key_env(),
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
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            base: default_base(),
            max_diff_bytes: default_max_diff_bytes(),
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
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

fn default_model() -> String {
    ProviderKind::default().default_model().to_owned()
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
        assert_eq!(config.llm.model, "claude-sonnet-4-5");
    }
}
