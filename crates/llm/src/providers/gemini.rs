//! Provider implementation for Google's Gemini REST API.
//!
//! Endpoint: `POST https://generativelanguage.googleapis.com/v1beta/models/<model>:generateContent`.
//! Auth: `x-goog-api-key` header.
//!
//! Gemini uses a bespoke request shape (no OpenAI compat) so it gets its
//! own provider. Quota errors come back as HTTP 429 *or* status 200 with
//! `RESOURCE_EXHAUSTED` in the body; we parse the optional
//! `error.details[].retryDelay` to honour Google's hint when present.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::conversation::{Role, Turn};
use crate::error::GenerateError;
use crate::provider::{Provider, ProviderResponse};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Fallback cooldown when the 429 body has no parseable `retryDelay`.
const DEFAULT_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct GeminiProvider {
    http: Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(http: Client, api_key: String) -> Self {
        Self { http, api_key }
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn generate(
        &self,
        model: &str,
        turns: &[Turn],
        system: &str,
        max_output_tokens: u32,
    ) -> Result<ProviderResponse, GenerateError> {
        let url = format!("{BASE}/{model}:generateContent");
        debug!(provider = "gemini", url = %url, model = %model, turns = turns.len(), "POST");

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
            "systemInstruction": { "parts": [{ "text": system }] },
            "generationConfig": {
                "maxOutputTokens": max_output_tokens,
                "temperature": 0.8,
                "topP": 0.95,
            },
            "safetySettings": [],
        });

        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
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
            if status.as_u16() == 429 || raw.contains("RESOURCE_EXHAUSTED") {
                let retry = parse_retry_delay(&raw).unwrap_or(DEFAULT_RETRY);
                warn!(provider = "gemini", model = %model, retry_s = retry.as_secs(), "quota exhausted");
                return Err(GenerateError::QuotaExceeded {
                    provider: "gemini".into(),
                    model: model.into(),
                    retry_after: retry,
                });
            }
            warn!(provider = "gemini", model = %model, %status, body = %raw, "api error");
            return Err(GenerateError::Api {
                provider: "gemini".into(),
                status: status.as_u16(),
                body: raw,
            });
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
        Ok(ProviderResponse {
            text,
            prompt_tokens: usage.prompt_token_count,
            output_tokens: usage.candidates_token_count,
        })
    }

    async fn health_check(&self) -> Result<(), GenerateError> {
        // We don't have a free no-token endpoint we can hit for *every*
        // configured model, so health-check is a no-op here. The router's
        // first real `generate` call will surface any auth / network issue.
        Ok(())
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Model => "model",
    }
}

/// Pull `retryDelay` (e.g. `"47s"`, `"1.5s"`, `"1m30s"`) from Google's 429
/// body when it carries a `RetryInfo` detail.
fn parse_retry_delay(body: &str) -> Option<Duration> {
    let v: Value = serde_json::from_str(body).ok()?;
    let details = v.pointer("/error/details")?.as_array()?;
    for d in details {
        if d.get("@type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.ends_with("RetryInfo"))
        {
            if let Some(s) = d.get("retryDelay").and_then(Value::as_str) {
                return parse_duration_str(s);
            }
        }
    }
    None
}

pub(crate) fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim();
    // `1m30s` / `2m` first — checking the `s` suffix earlier would swallow
    // them (the trailing `s` matches but `1m30` doesn't parse as a float).
    if let Some((m_part, s_part)) = s.split_once('m') {
        let mins: u64 = m_part.parse().ok()?;
        let secs: u64 = match s_part.trim_end_matches('s') {
            "" => 0,
            n => n.parse().ok()?,
        };
        return Some(Duration::from_secs(mins * 60 + secs));
    }
    if let Some(num) = s.strip_suffix('s') {
        let secs: f64 = num.parse().ok()?;
        return Some(Duration::from_secs_f64(secs));
    }
    None
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

#[cfg(test)]
mod tests {
    use super::{parse_duration_str, parse_retry_delay};
    use std::time::Duration;

    #[test]
    fn parses_simple_seconds() {
        assert_eq!(parse_duration_str("47s"), Some(Duration::from_secs(47)));
        assert_eq!(
            parse_duration_str("1.5s"),
            Some(Duration::from_secs_f64(1.5))
        );
    }

    #[test]
    fn parses_minutes_seconds() {
        assert_eq!(parse_duration_str("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration_str("2m"), Some(Duration::from_secs(120)));
    }

    #[test]
    fn parses_retry_info_block() {
        let body = r#"{
            "error": {
                "details": [
                    {"@type": "...RetryInfo", "retryDelay": "47s"}
                ]
            }
        }"#;
        assert_eq!(parse_retry_delay(body), Some(Duration::from_secs(47)));
    }

    #[test]
    fn missing_retry_info_returns_none() {
        assert_eq!(parse_retry_delay(r#"{"error":{}}"#), None);
    }
}
