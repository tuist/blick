use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::BlickError;
use crate::llm::ReviewClient;

pub struct OpenAiClient {
    base_url: String,
    api_key: String,
    model: String,
    max_output_tokens: u32,
    http: reqwest::Client,
}

impl OpenAiClient {
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
impl ReviewClient for OpenAiClient {
    async fn review(&self, system_prompt: &str, user_prompt: &str) -> Result<String, BlickError> {
        let request = OpenAiRequest {
            model: &self.model,
            max_output_tokens: self.max_output_tokens,
            input: vec![
                OpenAiMessage {
                    role: "developer",
                    content: system_prompt,
                },
                OpenAiMessage {
                    role: "user",
                    content: user_prompt,
                },
            ],
        };

        let response = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;

        parse_openai_response(response).await
    }
}

async fn parse_openai_response(response: reqwest::Response) -> Result<String, BlickError> {
    let status = response.status();

    if status == StatusCode::OK {
        let response: OpenAiResponse = response.json().await?;
        return response.extract_text().ok_or_else(|| {
            BlickError::Api("OpenAI response did not contain any text output".to_owned())
        });
    }

    let error = response.text().await?;
    Err(BlickError::Api(format!(
        "OpenAI request failed with status {}: {}",
        status.as_u16(),
        error.trim()
    )))
}

#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    max_output_tokens: u32,
    input: Vec<OpenAiMessage<'a>>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<OpenAiOutput>,
}

impl OpenAiResponse {
    fn extract_text(self) -> Option<String> {
        if let Some(output_text) = self.output_text {
            if !output_text.trim().is_empty() {
                return Some(output_text);
            }
        }

        let text = self
            .output
            .into_iter()
            .flat_map(|item| item.content)
            .filter_map(|content| content.text)
            .collect::<Vec<_>>()
            .join("");

        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiOutput {
    #[serde(default)]
    content: Vec<OpenAiOutputContent>,
}

#[derive(Debug, Deserialize)]
struct OpenAiOutputContent {
    #[serde(default)]
    text: Option<String>,
}
