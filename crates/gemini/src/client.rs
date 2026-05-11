use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tracing::{debug, info, warn};

use tomo_core::config::GeminiConfig;

use crate::conversation::{Role, Turn};

const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Fallback cooldown when the 429 response carries no usable hint. RPM-style
/// limits reset within a minute; daily quotas are much longer but we don't
/// know which one tripped, so we err short and keep retrying.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

/// Maximum cooldown we'll honour even if Google asks for longer. Prevents a
/// pathological RetryInfo from pinning a model out of rotation for hours.
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct GeminiClient {
    http: Client,
    cfg: GeminiConfig,
    /// `model_id -> when it becomes usable again`. Models absent from the
    /// map (or whose entry is in the past) are eligible. Behind a sync
    /// `Mutex`: the critical section is a couple of map ops, so async
    /// contention is irrelevant.
    cooldowns: Arc<Mutex<HashMap<String, Instant>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    pub text: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    /// Which model in the chain actually answered. Useful when fallbacks
    /// fire so logs and stats reflect reality, not the configured primary.
    pub model_used: String,
}

#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("decode error: {0}")]
    Decode(String),
    /// Returned when **every** model in the chain is currently cooled-down
    /// or hit a 429 on this request. Callers reserve this for the
    /// once-per-day owner alert.
    #[error("gemini quota or rate limit exhausted on all configured models")]
    QuotaExceeded,
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("client build failed: {0}")]
    Build(String),
    #[error("no models configured")]
    NoModels,
}

impl GeminiClient {
    pub fn new(cfg: GeminiConfig) -> Result<Self, GenerateError> {
        if cfg.models.is_empty() {
            return Err(GenerateError::NoModels);
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("tomo/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| GenerateError::Build(e.to_string()))?;
        Ok(Self {
            http,
            cfg,
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn config(&self) -> &GeminiConfig {
        &self.cfg
    }

    /// Snapshot of the cooldown map keyed by model id, expressed as seconds
    /// remaining. Models not currently rate-limited are omitted.
    pub fn cooldowns(&self) -> HashMap<String, Duration> {
        let now = Instant::now();
        self.cooldowns
            .lock()
            .map(|map| {
                map.iter()
                    .filter_map(|(k, &t)| {
                        t.checked_duration_since(now).map(|d| (k.clone(), d))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Hit `GET /v1beta/models/<primary>` to validate auth + reachability.
    pub async fn health_check(&self) -> Result<HealthInfo, GenerateError> {
        let model = self.cfg.primary_model().to_string();
        let url = format!("{BASE}/{model}");
        debug!(url = %url, model = %model, "gemini health: GET model");

        let resp = self
            .http
            .get(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
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
                return Err(GenerateError::QuotaExceeded);
            }
            return Err(GenerateError::Api { status: status.as_u16(), body: raw });
        }

        let parsed: ModelInfo = serde_json::from_str(&raw)
            .map_err(|e| GenerateError::Decode(format!("{e} | raw={raw}")))?;

        Ok(HealthInfo {
            name: parsed.name,
            display_name: parsed.display_name.unwrap_or_default(),
            input_token_limit: parsed.input_token_limit.unwrap_or_default(),
            output_token_limit: parsed.output_token_limit.unwrap_or_default(),
        })
    }

    /// Send `turns` to whichever model in the chain is currently available.
    ///
    /// Iterates `cfg.models` in order, skipping any with a live cooldown. On
    /// 429 the active model is parked (using Google's `RetryInfo.retryDelay`
    /// hint when present, otherwise [`DEFAULT_COOLDOWN`]) and the loop tries
    /// the next. Returns [`GenerateError::QuotaExceeded`] only when every
    /// configured model has just refused us; non-quota errors short-circuit
    /// the loop (no point in trying another model for a 401 / 5xx).
    pub async fn generate(
        &self,
        turns: &[Turn],
        extra_context: Option<&str>,
    ) -> Result<GenerateResponse, GenerateError> {
        if self.cfg.models.is_empty() {
            return Err(GenerateError::NoModels);
        }

        let contents: Vec<_> = turns
            .iter()
            .map(|t| {
                json!({
                    "role": role_to_str(t.role),
                    "parts": [{ "text": t.text }],
                })
            })
            .collect();

        let system_text = match extra_context.map(str::trim).filter(|s| !s.is_empty()) {
            Some(ctx) => format!("{}\n\n{ctx}", self.cfg.system_prompt),
            None => self.cfg.system_prompt.clone(),
        };

        let body = json!({
            "contents": contents,
            "systemInstruction": {
                "parts": [{ "text": system_text }],
            },
            "generationConfig": {
                "maxOutputTokens": self.cfg.max_output_tokens,
                "temperature": 0.8,
                "topP": 0.95,
            },
            "safetySettings": [],
        });

        let now = Instant::now();
        let mut all_cooled_down = true;
        let mut last_429: Option<(String, String)> = None;

        for model in &self.cfg.models {
            if let Some(remaining) = self.cooldown_remaining(model, now) {
                debug!(
                    model = %model,
                    remaining_s = remaining.as_secs(),
                    "gemini: skipping cooled-down model"
                );
                continue;
            }
            all_cooled_down = false;

            match self.try_one(model, &body, turns.len()).await {
                Ok(mut resp) => {
                    resp.model_used = model.clone();
                    return Ok(resp);
                }
                Err(GenerateError::QuotaExceeded) => {
                    // try_one already set the cooldown.
                    last_429 = Some((model.clone(), "429".into()));
                    info!(
                        model = %model,
                        next = ?self.cfg.models.iter().skip_while(|m| m != &model).nth(1),
                        "gemini: model rate-limited, rotating to next in chain"
                    );
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        if all_cooled_down {
            debug!("gemini: all models currently cooled-down");
        }
        if let Some((m, _)) = last_429 {
            warn!(last_model = %m, "gemini: every model in chain refused — giving up");
        }
        Err(GenerateError::QuotaExceeded)
    }

    /// Send the request to one specific model. On 429 the model is parked
    /// and `QuotaExceeded` is returned so the outer loop tries the next.
    async fn try_one(
        &self,
        model: &str,
        body: &Value,
        turn_count: usize,
    ) -> Result<GenerateResponse, GenerateError> {
        let url = format!("{BASE}/{model}:generateContent");
        debug!(url = %url, model = %model, turns = turn_count, "gemini generate: POST");

        let resp = self
            .http
            .post(&url)
            .header("x-goog-api-key", &self.cfg.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;

        let status = resp.status();
        let raw = resp
            .text()
            .await
            .map_err(|e| GenerateError::Transport(e.to_string()))?;
        debug!(model = %model, status = %status, raw_len = raw.len(), "gemini generate: response");

        if !status.is_success() {
            if status.as_u16() == 429 || raw.contains("RESOURCE_EXHAUSTED") {
                let delay = parse_retry_delay(&raw).unwrap_or(DEFAULT_COOLDOWN);
                let delay = delay.min(MAX_COOLDOWN);
                self.set_cooldown(model, delay);
                warn!(
                    model = %model,
                    cooldown_s = delay.as_secs(),
                    "gemini: model quota exhausted, parked"
                );
                return Err(GenerateError::QuotaExceeded);
            }
            warn!(model = %model, %status, body = %raw, "gemini error response");
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
            model_used: model.to_string(),
        })
    }

    fn cooldown_remaining(&self, model: &str, now: Instant) -> Option<Duration> {
        let map = self.cooldowns.lock().ok()?;
        map.get(model).and_then(|&t| t.checked_duration_since(now))
    }

    fn set_cooldown(&self, model: &str, delay: Duration) {
        let until = Instant::now() + delay;
        if let Ok(mut map) = self.cooldowns.lock() {
            map.insert(model.to_string(), until);
        }
    }
}

/// Pull `retryDelay` (e.g. `"47s"`, `"1.5s"`) out of Google's 429 body if it
/// includes a `RetryInfo` detail. Returns `None` when the body is something
/// else or the field is missing.
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

/// Parse `"47s"` / `"1.5s"` / `"1m30s"` / `"2m"` style strings into
/// `Duration`. Just enough for what Google's RetryInfo emits.
fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim();
    // Combined `m`/`s` form first — checking the seconds-only suffix first
    // would swallow `"1m30s"` (the trailing `s` matches but `1m30` doesn't
    // parse as a float).
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

/// Lightweight model info returned by `GET /v1beta/models/<model>`.
#[derive(Debug, Clone)]
pub struct HealthInfo {
    pub name: String,
    pub display_name: String,
    pub input_token_limit: u32,
    pub output_token_limit: u32,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "inputTokenLimit")]
    input_token_limit: Option<u32>,
    #[serde(default, rename = "outputTokenLimit")]
    output_token_limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::{parse_duration_str, parse_retry_delay};
    use std::time::Duration;

    #[test]
    fn parses_simple_seconds() {
        assert_eq!(parse_duration_str("47s"), Some(Duration::from_secs(47)));
        assert_eq!(parse_duration_str("0s"), Some(Duration::from_secs(0)));
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
                "code": 429,
                "message": "rate limit",
                "status": "RESOURCE_EXHAUSTED",
                "details": [
                    {"@type": "type.googleapis.com/google.rpc.QuotaFailure"},
                    {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "47s"}
                ]
            }
        }"#;
        assert_eq!(parse_retry_delay(body), Some(Duration::from_secs(47)));
    }

    #[test]
    fn missing_retry_info_returns_none() {
        let body = r#"{"error": {"code": 429, "message": "rate limit"}}"#;
        assert_eq!(parse_retry_delay(body), None);
    }
}
