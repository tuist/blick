use async_trait::async_trait;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatResponseFormat};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ServiceTarget};

use crate::config::{LlmConfig, ProviderKind};
use crate::error::BlickError;
use crate::llm::ReviewClient;

pub struct GenAIReviewClient {
    client: Client,
    model: String,
    options: ChatOptions,
}

impl GenAIReviewClient {
    pub fn new(config: &LlmConfig) -> Result<Self, BlickError> {
        let model = qualified_model(config)?;
        let mut builder = Client::builder();

        if let Some(api_key) = config.api_key()? {
            builder = builder.with_auth_resolver_fn(move |_model| {
                Ok(Some(AuthData::from_single(api_key.clone())))
            });
        }

        if let Some(base_url) = config.base_url() {
            if Some(base_url) != config.provider.default_base_url() {
                let base_url = base_url.to_owned();
                builder =
                    builder.with_service_target_resolver_fn(move |mut target: ServiceTarget| {
                        target.endpoint = Endpoint::from_owned(base_url.clone());
                        Ok(target)
                    });
            }
        }

        Ok(Self {
            client: builder.build(),
            model,
            options: ChatOptions::default()
                .with_max_tokens(config.max_output_tokens)
                .with_response_format(ChatResponseFormat::JsonMode),
        })
    }
}

#[async_trait]
impl ReviewClient for GenAIReviewClient {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError> {
        let request =
            ChatRequest::new(vec![ChatMessage::user(user_prompt)]).with_system(system_prompt);
        let response = self
            .client
            .exec_chat(&self.model, request, Some(&self.options))
            .await?;

        response.into_first_text().ok_or_else(|| {
            BlickError::Api("provider response did not contain any text output".to_owned())
        })
    }
}

fn qualified_model(config: &LlmConfig) -> Result<String, BlickError> {
    let Some(model) = config.model() else {
        return Err(BlickError::Config(format!(
            "provider {} requires a model to be configured",
            config.provider.as_str()
        )));
    };

    let qualified = match config.provider {
        ProviderKind::OpenAI => format!("openai::{model}"),
        ProviderKind::Anthropic => format!("anthropic::{model}"),
        ProviderKind::Auto | ProviderKind::Claude | ProviderKind::Codex => model.to_owned(),
    };

    Ok(qualified)
}
