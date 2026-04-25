use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::BlickError;
use crate::llm::ReviewClient;

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicClient {
    base_url: String,
    api_key: String,
    model: String,
    max_output_tokens: u32,
    http: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(base_url: &str, api_key: String, model: &str, max_output_tokens: u32) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            model: model.to_owned(),
            max_output_tokens,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ReviewClient for AnthropicClient {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError> {
        let request = AnthropicRequest {
            model: &self.model,
            max_tokens: self.max_output_tokens,
            system: system_prompt,
            messages: vec![AnthropicMessage {
                role: "user",
                content: user_prompt,
            }],
        };

        let response = self
            .http
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .await?;

        parse_anthropic_response(response).await
    }
}

async fn parse_anthropic_response(response: reqwest::Response) -> Result<String, BlickError> {
    let status = response.status();

    if status == StatusCode::OK {
        let response: AnthropicResponse = response.json().await?;
        let text = response
            .content
            .into_iter()
            .filter(|item| item.kind == "text")
            .map(|item| item.text)
            .collect::<Vec<_>>()
            .join("");

        if text.trim().is_empty() {
            return Err(BlickError::Api(
                "Anthropic response did not contain any text output".to_owned(),
            ));
        }

        return Ok(text);
    }

    let error = response.text().await?;
    Err(BlickError::Api(format!(
        "Anthropic request failed with status {}: {}",
        status.as_u16(),
        error.trim()
    )))
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}
