mod anthropic;
mod openai;

use async_trait::async_trait;

use crate::config::{Config, ProviderKind};
use crate::error::BlickError;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;

#[async_trait]
pub trait ReviewClient: Send + Sync {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError>;
}

pub fn client_from_config(config: &Config) -> Result<Box<dyn ReviewClient>, BlickError> {
    let api_key = config.llm.api_key()?;

    match config.llm.provider {
        ProviderKind::OpenAi => Ok(Box::new(OpenAiClient::new(
            config.llm.base_url(),
            api_key,
            &config.llm.model,
            config.llm.max_output_tokens,
        ))),
        ProviderKind::Anthropic => Ok(Box::new(AnthropicClient::new(
            config.llm.base_url(),
            api_key,
            &config.llm.model,
            config.llm.max_output_tokens,
        ))),
    }
}
