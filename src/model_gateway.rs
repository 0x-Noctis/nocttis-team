use std::{env, time::Duration};

use serde::Serialize;

#[path = "model/mod.rs"]
pub mod model;
#[path = "model/openai/mod.rs"]
pub mod openai;

use model::{Message, MessageRole, ModelLimits, ModelRequest, Usage};
use openai::OpenAiChatClient;

#[derive(Clone)]
pub struct OpenAiClient {
    chat: OpenAiChatClient,
    model: String,
}

#[derive(Serialize)]
pub struct ProbeResult {
    pub status: &'static str,
    pub model: String,
    pub latency_ms: u128,
    pub content: String,
    pub usage: Usage,
}

impl OpenAiClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = env::var("PRIMARY_BASE_URL")?;
        let api_key = env::var("PRIMARY_API_KEY")?;
        let model = env::var("PRIMARY_MODEL")?;
        let chat = OpenAiChatClient::new(&base_url, api_key, &model, Duration::from_secs(180))
            .map_err(anyhow::Error::new)?;
        Ok(Self { chat, model })
    }

    pub async fn probe(&self) -> Result<ProbeResult, model::ModelError> {
        let response = self
            .chat
            .complete(&ModelRequest {
                project_id: "probe".to_owned(),
                task_id: "probe".to_owned(),
                agent_run_id: "probe".to_owned(),
                model_class: "probe".to_owned(),
                messages: vec![Message {
                    role: MessageRole::User,
                    content: "Reply with exactly: OK".to_owned(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                }],
                tools: Vec::new(),
                limits: ModelLimits {
                    max_input_tokens: 0,
                    max_output_tokens: 8,
                },
            })
            .await?;

        Ok(ProbeResult {
            status: "ok",
            model: self.model.clone(),
            latency_ms: response.latency_ms,
            content: response.content.unwrap_or_default(),
            usage: response.usage,
        })
    }
}
