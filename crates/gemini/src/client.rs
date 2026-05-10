use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tracing::warn;

use tomo_core::config::GeminiConfig;

use crate::conversation::{Role, Turn};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

#[derive(Clone)]
pub struct GeminiClient {
    http: Client,
    cfg: GeminiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    pub text: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
}

/// Errors returned from [`GeminiClient::generate`]. Callers care about
/// [`GenerateError::QuotaExceeded`] specifically — that one fires the
/// once-per-day owner alert in `tomo-discord`.
#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("decode error: {0}")]
    Decode(String),
    /// HTTP 429 or `RESOURCE_EXHAUSTED` from the model API.
    #[error("gemini quota or rate limit exhausted")]
    QuotaExceeded,
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("client build failed: {0}")]
    Build(String),
}

impl GeminiClient {
    pub fn new(cfg: GeminiConfig) -> Result<Self, GenerateError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("tomo/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| GenerateError::Build(e.to_string()))?;
        Ok(Self { http, cfg })
    }

    pub fn config(&self) -> &GeminiConfig {
        &self.cfg
    }

    /// Send `turns` to the model and return the text + token counts. The system
    /// instruction from config is attached automatically.
    pub async fn generate(&self, turns: &[Turn]) -> Result<GenerateResponse, GenerateError> {
        let url = format!("{BASE}/{model}:generateContent", model = self.cfg.model);

        let contents: Vec<_> = turns
            .iter()
            .map(|t| {
                json!({
                    "role": role_to_str(t.role),
                    "parts": [{ "text": t.text }],
                })
            })
            .collect();

        let body = json!({
            "contents": contents,
            "systemInstruction": {
                "parts": [{ "text": self.cfg.system_prompt }],
            },
            "generationConfig": {
                "maxOutputTokens": self.cfg.max_output_tokens,
                "temperature": 0.8,
                "topP": 0.95,
            },
            "safetySettings": [],
        });

        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;

        let status = resp.status();
        let raw = resp
            .text()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;

        if !status.is_success() {
            // 429 ("Too Many Requests") and Google's `RESOURCE_EXHAUSTED` both
            // mean the same thing here: quota / rate limit hit. Surface it as
            // a distinct error so the caller can pick the right reaction.
            if status.as_u16() == 429 || raw.contains("RESOURCE_EXHAUSTED") {
                warn!(%status, "gemini quota exhausted");
                return Err(GenerateError::QuotaExceeded);
            }
            warn!(%status, body = %raw, "gemini error response");
            return Err(GenerateError::Api { status: status.as_u16(), body: raw });
        }

        let parsed: ApiResponse = serde_json::from_str(&raw)
            .map_err(|e| GenerateError::Decode(format!("{e} | raw={raw}")))?;

        let text = parsed
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content)
            .map(|c| {
                c.parts
                    .into_iter()
                    .filter_map(|p| p.text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let usage = parsed.usage_metadata.unwrap_or_default();
        Ok(GenerateResponse {
            text,
            prompt_tokens: usage.prompt_token_count,
            output_tokens: usage.candidates_token_count,
        })
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Model => "model",
    }
}

// ---------- Wire format ----------

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Option<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct Part {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: u32,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: u32,
}
