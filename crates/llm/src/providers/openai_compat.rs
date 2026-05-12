//! Provider for any service that speaks OpenAI's `/v1/chat/completions`.
//!
//! Verified compatible (free tier, May 2026):
//! * **Groq** — `https://api.groq.com/openai/v1` — 30 RPM × 14.4K RPD
//! * **Cerebras** — `https://api.cerebras.ai/v1` — ~1700 RPD
//! * **OpenRouter** — `https://openrouter.ai/api/v1` — 20 RPM × 200 RPD on `:free` models
//! * **Mistral** — `https://api.mistral.ai/v1` — 1B tok/month, 2 RPM
//!
//! Anything else that ships the same shape (Together, Cloudflare Workers AI,
//! local llama.cpp servers, …) drops in via `OpenAiCompatKind::Custom`.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::conversation::{Role, Turn};
use crate::error::GenerateError;
use crate::provider::{Provider, ProviderResponse};
use crate::providers::gemini::parse_duration_str;

/// Fallback cooldown when the 429 carries no `Retry-After` / `retry_after` hint.
const DEFAULT_RETRY: Duration = Duration::from_secs(60);

/// Which upstream this provider points at — picks a default base URL and a
/// short name for logs / cooldown keys.
#[derive(Debug, Clone)]
pub enum OpenAiCompatKind {
    Groq,
    Cerebras,
    OpenRouter,
    Mistral,
    /// Arbitrary OpenAI-compatible endpoint. `name` is what logs/the admin
    /// embed call it; `base_url` must end without `/chat/completions` —
    /// the provider appends that itself.
    Custom { name: String, base_url: String },
}

impl OpenAiCompatKind {
    pub fn name(&self) -> &str {
        match self {
            Self::Groq => "groq",
            Self::Cerebras => "cerebras",
            Self::OpenRouter => "openrouter",
            Self::Mistral => "mistral",
            Self::Custom { name, .. } => name,
        }
    }

    pub fn base_url(&self) -> &str {
        match self {
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::Cerebras => "https://api.cerebras.ai/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Mistral => "https://api.mistral.ai/v1",
            Self::Custom { base_url, .. } => base_url,
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatProvider {
    http: Client,
    kind: OpenAiCompatKind,
    api_key: String,
    name: String,      // cached `kind.name()` so `Provider::name(&self) -> &str` is straightforward
    base_url: String,  // cached `kind.base_url()`
}

impl OpenAiCompatProvider {
    pub fn new(http: Client, kind: OpenAiCompatKind, api_key: String) -> Self {
        let name = kind.name().to_string();
        let base_url = kind.base_url().to_string();
        Self { http, kind, api_key, name, base_url }
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn generate(
        &self,
        model: &str,
        turns: &[Turn],
        system: &str,
        max_output_tokens: u32,
    ) -> Result<ProviderResponse, GenerateError> {
        let url = format!("{}/chat/completions", self.base_url);
        debug!(provider = %self.name, url = %url, model = %model, turns = turns.len(), "POST");

        // OpenAI shape: messages = system + turn list.
        let mut messages = Vec::with_capacity(turns.len() + 1);
        if !system.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": system }));
        }
        for t in turns {
            messages.push(json!({
                "role": role_to_str(t.role),
                "content": t.text,
            }));
        }

        let body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_output_tokens,
            "temperature": 0.8,
            "top_p": 0.95,
        });

        let mut req = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body);

        // OpenRouter recommends `HTTP-Referer` + `X-Title` so they can
        // attribute traffic — costs nothing for us and helps if we ever
        // hit support.
        if matches!(self.kind, OpenAiCompatKind::OpenRouter) {
            req = req
                .header("HTTP-Referer", "https://github.com/tmokenc/tomo")
                .header("X-Title", "Tomo");
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;

        let status = resp.status();
        // Pull `Retry-After` before consuming the body — it's a header, not
        // a JSON field on OpenAI-shaped providers.
        let retry_after_header = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.trim()
                    .parse::<u64>()
                    .ok()
                    .map(Duration::from_secs)
                    .or_else(|| parse_duration_str(s))
            });

        let raw = resp
            .text()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;

        if !status.is_success() {
            if status.as_u16() == 429 {
                let retry = retry_after_header
                    .or_else(|| parse_retry_from_body(&raw))
                    .unwrap_or(DEFAULT_RETRY);
                warn!(provider = %self.name, model = %model, retry_s = retry.as_secs(), "quota exhausted");
                return Err(GenerateError::QuotaExceeded {
                    provider: self.name.clone(),
                    model: model.into(),
                    retry_after: retry,
                });
            }
            warn!(provider = %self.name, model = %model, %status, body = %raw, "api error");
            return Err(GenerateError::Api {
                provider: self.name.clone(),
                status: status.as_u16(),
                body: raw,
            });
        }

        let parsed: ChatResponse = serde_json::from_str(&raw)
            .map_err(|e| GenerateError::Decode(format!("{e} | raw={raw}")))?;

        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.unwrap_or_default())
            .unwrap_or_default();

        let usage = parsed.usage.unwrap_or_default();
        Ok(ProviderResponse {
            text,
            prompt_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
        })
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        // OpenAI uses "assistant" for what Gemini calls "model".
        Role::Model => "assistant",
    }
}

/// Best-effort scrape of a retry hint from the JSON body. OpenAI / Groq
/// surface it under `error.message` (free-form), so we look for an integer
/// followed by `"s"` / `"ms"` / `"seconds"`. Plenty of false-negatives are
/// OK — the caller falls back to `DEFAULT_RETRY`.
fn parse_retry_from_body(body: &str) -> Option<Duration> {
    // First try a few standard JSON shapes.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        for path in [
            "/error/retry_after",
            "/error/details/0/retryDelay",
            "/retry_after",
        ] {
            if let Some(s) = v.pointer(path).and_then(serde_json::Value::as_str) {
                if let Some(d) = parse_duration_str(s) {
                    return Some(d);
                }
            }
            if let Some(n) = v.pointer(path).and_then(serde_json::Value::as_u64) {
                return Some(Duration::from_secs(n));
            }
        }
    }
    None
}

// ---------- OpenAI wire format ----------

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::{parse_retry_from_body, OpenAiCompatKind};
    use std::time::Duration;

    #[test]
    fn kind_defaults() {
        assert_eq!(OpenAiCompatKind::Groq.name(), "groq");
        assert_eq!(OpenAiCompatKind::Groq.base_url(), "https://api.groq.com/openai/v1");
        assert_eq!(OpenAiCompatKind::Cerebras.name(), "cerebras");
        assert_eq!(OpenAiCompatKind::OpenRouter.name(), "openrouter");
        assert_eq!(OpenAiCompatKind::Mistral.name(), "mistral");
    }

    #[test]
    fn parses_numeric_retry_after() {
        // OpenRouter style.
        let body = r#"{"error":{"retry_after":42}}"#;
        assert_eq!(parse_retry_from_body(body), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parses_string_retry_after() {
        let body = r#"{"retry_after":"30s"}"#;
        assert_eq!(parse_retry_from_body(body), Some(Duration::from_secs(30)));
    }
}
