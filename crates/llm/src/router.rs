//! `LlmRouter` — walks a flat list of (provider, model) entries, parks
//! ones that 429, and propagates non-quota errors immediately.
//!
//! The chain is intentionally flat (not nested per-provider) because that
//! matches how the user thinks about it: "try these in order, skip
//! whatever's cooled down". Per-model cooldowns let one Gemini-2.5-pro
//! 429 only park *that* model, not the rest of Gemini.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use tracing::{debug, info, warn};

use tomo_core::config::LlmConfig;

use crate::conversation::Turn;
use crate::error::GenerateError;
use crate::provider::Provider;
use crate::providers::{GeminiProvider, OpenAiCompatKind, OpenAiCompatProvider};

/// Hard upper bound on any cooldown — prevents a runaway `retryDelay` of
/// "30 days" from pinning a provider/model out of rotation indefinitely.
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct RouteEntry {
    pub provider: Arc<dyn Provider>,
    pub model: String,
}

impl RouteEntry {
    fn key(&self) -> (String, String) {
        (self.provider.name().to_string(), self.model.clone())
    }
}

#[derive(Clone)]
pub struct LlmRouter {
    entries: Vec<RouteEntry>,
    cooldowns: Arc<Mutex<HashMap<(String, String), Instant>>>,
    pub system_prompt: String,
    pub max_output_tokens: u32,
    pub context_messages: usize,
}

/// What the router returns. Inherits everything from [`ProviderResponse`]
/// plus the provider/model that actually answered — handy for logs, stats,
/// and the admin embed.
#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub text: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub provider_used: String,
    pub model_used: String,
}

impl LlmRouter {
    pub fn new(
        entries: Vec<RouteEntry>,
        system_prompt: String,
        max_output_tokens: u32,
        context_messages: usize,
    ) -> Self {
        Self {
            entries,
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
            system_prompt,
            max_output_tokens,
            context_messages,
        }
    }

    /// Build a router straight from a parsed [`LlmConfig`]. Owns the
    /// shared `reqwest::Client` and the per-provider instances so the
    /// caller (the discord crate) doesn't need to depend on reqwest
    /// directly.
    ///
    /// Returns an error only when the HTTP client itself fails to build —
    /// otherwise produces a router whose chain has had unconfigured
    /// providers filtered out (and logged at `warn!`).
    pub fn from_config(cfg: LlmConfig) -> Result<Self, GenerateError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("tomo/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| GenerateError::Build(e.to_string()))?;

        // One provider instance per name in api_keys (shared across chain
        // entries that reuse the same provider for different models).
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        for (name, key) in &cfg.api_keys {
            let provider: Arc<dyn Provider> = match name.as_str() {
                "gemini" => Arc::new(GeminiProvider::new(http.clone(), key.clone())),
                "groq" => Arc::new(OpenAiCompatProvider::new(
                    http.clone(),
                    OpenAiCompatKind::Groq,
                    key.clone(),
                )),
                "cerebras" => Arc::new(OpenAiCompatProvider::new(
                    http.clone(),
                    OpenAiCompatKind::Cerebras,
                    key.clone(),
                )),
                "openrouter" => Arc::new(OpenAiCompatProvider::new(
                    http.clone(),
                    OpenAiCompatKind::OpenRouter,
                    key.clone(),
                )),
                "mistral" => Arc::new(OpenAiCompatProvider::new(
                    http.clone(),
                    OpenAiCompatKind::Mistral,
                    key.clone(),
                )),
                other => {
                    warn!(provider = %other, "llm: unknown provider in api_keys — skipping");
                    continue;
                }
            };
            providers.insert(name.clone(), provider);
        }

        // Flatten the chain to RouteEntry, dropping any that reference a
        // provider with no instance (e.g., unknown provider name or
        // missing key).
        let mut entries = Vec::with_capacity(cfg.chain.len());
        for raw in &cfg.chain {
            let Some(provider) = providers.get(&raw.provider) else {
                warn!(
                    provider = %raw.provider,
                    model = %raw.model,
                    "llm: chain entry references unconfigured provider — skipping"
                );
                continue;
            };
            entries.push(RouteEntry {
                provider: Arc::clone(provider),
                model: raw.model.clone(),
            });
        }

        Ok(Self::new(
            entries,
            cfg.system_prompt,
            cfg.max_output_tokens,
            cfg.context_messages.max(2),
        ))
    }

    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Snapshot of remaining cooldown per (provider, model). Entries whose
    /// cooldown has elapsed are omitted.
    pub fn cooldowns(&self) -> HashMap<(String, String), Duration> {
        let now = Instant::now();
        self.cooldowns
            .lock()
            .map(|map| {
                map.iter()
                    .filter_map(|(k, &t)| t.checked_duration_since(now).map(|d| (k.clone(), d)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Send `turns` to whichever (provider, model) entry is currently
    /// available. Walks the chain in order, skipping entries with live
    /// cooldowns. On **any** error the entry is parked for
    /// [`GenerateError::cooldown_hint`] (capped at [`MAX_COOLDOWN`]) and
    /// the next entry is tried; the last error is held back and only
    /// surfaced once every entry refuses.
    pub async fn generate(
        &self,
        turns: &[Turn],
        extra_context: Option<&str>,
    ) -> Result<GenerateResponse, GenerateError> {
        if self.entries.is_empty() {
            return Err(GenerateError::NoProviders);
        }

        let system = match extra_context.map(str::trim).filter(|s| !s.is_empty()) {
            Some(ctx) => format!("{}\n\n{ctx}", self.system_prompt),
            None => self.system_prompt.clone(),
        };

        let now = Instant::now();
        let mut last_err: Option<GenerateError> = None;

        for entry in &self.entries {
            let key = entry.key();
            if let Some(remaining) = self.cooldown_remaining(&key, now) {
                debug!(
                    provider = %entry.provider.name(),
                    model = %entry.model,
                    remaining_s = remaining.as_secs(),
                    "router: skipping cooled-down entry"
                );
                continue;
            }

            match entry
                .provider
                .generate(&entry.model, turns, &system, self.max_output_tokens)
                .await
            {
                Ok(r) => {
                    return Ok(GenerateResponse {
                        text: r.text,
                        prompt_tokens: r.prompt_tokens,
                        output_tokens: r.output_tokens,
                        provider_used: entry.provider.name().to_string(),
                        model_used: entry.model.clone(),
                    });
                }
                Err(e) => {
                    let park = e.cooldown_hint().min(MAX_COOLDOWN);
                    self.park(&key, park);
                    let kind = error_kind(&e);
                    // 429 is the expected rotation case → info; everything
                    // else is worth a louder warn so operators notice
                    // when a model is misconfigured or a provider is down.
                    if matches!(e, GenerateError::QuotaExceeded { .. }) {
                        info!(
                            provider = %entry.provider.name(),
                            model = %entry.model,
                            cooldown_s = park.as_secs(),
                            "router: entry rate-limited, rotating to next in chain"
                        );
                    } else {
                        warn!(
                            provider = %entry.provider.name(),
                            model = %entry.model,
                            cooldown_s = park.as_secs(),
                            kind = kind,
                            error = %e,
                            "router: entry failed, rotating to next in chain"
                        );
                    }
                    last_err = Some(e);
                    continue;
                }
            }
        }

        match last_err {
            Some(e) => {
                warn!(error = %e, "router: every entry refused — giving up");
                Err(e)
            }
            None => {
                debug!("router: every entry was already cooled-down at request time");
                Err(GenerateError::QuotaExceeded {
                    provider: "router".into(),
                    model: "*".into(),
                    retry_after: Duration::from_secs(60),
                })
            }
        }
    }

    fn cooldown_remaining(
        &self,
        key: &(String, String),
        now: Instant,
    ) -> Option<Duration> {
        let map = self.cooldowns.lock().ok()?;
        map.get(key).and_then(|&t| t.checked_duration_since(now))
    }

    fn park(&self, key: &(String, String), delay: Duration) {
        let until = Instant::now() + delay;
        if let Ok(mut map) = self.cooldowns.lock() {
            map.insert(key.clone(), until);
        }
    }
}

/// Short tag describing the failure class — purely for log scannability.
/// Keeps the `warn!` line a tidy single key=value pair instead of a long
/// `Debug`-formatted enum.
fn error_kind(e: &GenerateError) -> &'static str {
    match e {
        GenerateError::Transport(_) => "transport",
        GenerateError::Decode(_) => "decode",
        GenerateError::QuotaExceeded { .. } => "quota",
        GenerateError::Api { status, .. } if *status == 404 => "model_not_found",
        GenerateError::Api { status, .. } if (400..500).contains(status) => "client_error",
        GenerateError::Api { .. } => "server_error",
        GenerateError::Build(_) => "build",
        GenerateError::NoProviders => "no_providers",
    }
}
