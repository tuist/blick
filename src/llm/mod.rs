mod cli;
mod genai_client;

use async_trait::async_trait;

use crate::config::{Config, ProviderKind};
use crate::error::BlickError;

use self::cli::CliReviewClient;
use self::genai_client::GenAiReviewClient;

#[async_trait]
pub trait ReviewClient: Send + Sync {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError>;
}

pub fn client_from_config(config: &Config) -> Result<Box<dyn ReviewClient>, BlickError> {
    match config.llm.provider {
        ProviderKind::OpenAi | ProviderKind::Anthropic => {
            Ok(Box::new(GenAiReviewClient::new(&config.llm)?))
        }
        ProviderKind::Auto | ProviderKind::Claude | ProviderKind::Codex => {
            Ok(Box::new(CliReviewClient::new(&config.llm)))
        }
    }
}
