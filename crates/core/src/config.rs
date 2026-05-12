use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use twilight_model::id::Id;
use twilight_model::id::marker::{GuildMarker, UserMarker};

use crate::error::{Error, Result};

/// Top level config, populated from environment variables (typically loaded
/// from a `.env` file). Anything optional has a sensible default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub discord: DiscordConfig,
    pub llm: Option<LlmConfig>,
    pub ocr: Option<OcrConfig>,
    /// Directory the database backend lives in. Created on first run.
    pub data_dir: PathBuf,
    pub script_dir: PathBuf,
    pub enable_hot_reload: bool,
}

/// PaddleOCR (PP-OCRv5) model file paths. Each slot is optional — operators
/// can configure one or both to cover the languages they care about:
///
/// - **`latin`** — English, Czech, Vietnamese, and most European scripts via
///   the `latin_PP-OCRv5_mobile_rec` model.
/// - **`cjk`** — Simplified/Traditional Chinese, Japanese, English via the
///   default `ch_PP-OCRv5_*` models.
///
/// Configuring both engines lets the `ocr` command read documents that mix
/// scripts: it runs each engine in turn and merges the output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrConfig {
    pub latin: Option<OcrEngineFiles>,
    pub cjk: Option<OcrEngineFiles>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrEngineFiles {
    pub det: PathBuf,
    pub rec: PathBuf,
    pub keys: PathBuf,
}

impl OcrConfig {
    pub fn is_some(&self) -> bool {
        self.latin.is_some() || self.cjk.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub token: String,
    pub prefix: String,
    pub master_prefix: String,
    pub owners: Vec<Id<UserMarker>>,
    pub activity: Option<String>,
    pub enable_prefix: bool,
    pub enable_slash: bool,
    pub enable_llm: bool,
    pub enable_auto_triggers: bool,
    pub register_global: bool,
    pub dev_guild: Option<Id<GuildMarker>>,
    /// How long the gallery-lookup auto-triggers wait for a user to click the
    /// reaction before giving up and removing it. Seconds.
    pub gallery_lookup_wait_secs: u64,
}

/// Top-level LLM config — a flat ordered chain of (provider, model) entries
/// the router walks. Each entry knows which API key to use via the
/// provider-specific key in [`LlmConfig::api_keys`].
///
/// Configured from env via two complementary controls:
///   * the simple path — `LLM_PROVIDERS` (provider priority) × per-provider
///     `*_MODEL` lists (model priority within a provider);
///   * the power-user path — a flat `LLM_CHAIN=provider:model,...` that lets
///     you interleave providers (e.g., `groq:fast,gemini:flash,groq:slow`).
///
/// If both are set, `LLM_CHAIN` wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub chain: Vec<LlmChainEntry>,
    /// `provider_name -> api_key`. Entries in `chain` referencing a missing
    /// key are filtered out at router build time.
    pub api_keys: std::collections::HashMap<String, String>,
    pub context_messages: usize,
    pub max_output_tokens: u32,
    pub rate_limit_per_minute: u32,
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmChainEntry {
    /// Lower-case provider id: `gemini`, `groq`, `cerebras`, `openrouter`, `mistral`.
    pub provider: String,
    pub model: String,
}

impl LlmConfig {
    /// The model the router will try first — useful for the startup health-log
    /// line that confirms which model gets the "primary" slot.
    pub fn primary(&self) -> Option<&LlmChainEntry> {
        self.chain.first()
    }
}

impl Config {
    /// Parse environment variables. The caller is expected to have already
    /// loaded `.env` (so `RUST_LOG` is visible to the tracing subscriber
    /// before it initialises) — see `tomo/src/main.rs`.
    pub fn from_env() -> Result<Self> {
        let token = require("DISCORD_TOKEN")?;
        let prefix = optional("TOMO_PREFIX").unwrap_or_else(|| "tomo>".into());
        let master_prefix = optional("TOMO_MASTER_PREFIX").unwrap_or_else(|| "%".into());
        let activity = optional("TOMO_ACTIVITY");

        let owners = optional("TOMO_OWNERS")
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Id::from_str(s).map_err(|_| Error::config(format!("invalid owner id `{s}`"))))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        let dev_guild = optional("TOMO_DEV_GUILD")
            .map(|raw| Id::from_str(&raw).map_err(|_| Error::config("invalid TOMO_DEV_GUILD")))
            .transpose()?;

        let discord = DiscordConfig {
            token,
            prefix,
            master_prefix,
            owners,
            activity,
            enable_prefix: bool_env("TOMO_ENABLE_PREFIX", true),
            enable_slash: bool_env("TOMO_ENABLE_SLASH", true),
            enable_llm: bool_env("TOMO_ENABLE_LLM", true),
            enable_auto_triggers: bool_env("TOMO_ENABLE_AUTO_TRIGGERS", true),
            register_global: bool_env("TOMO_REGISTER_GLOBAL", false),
            dev_guild,
            gallery_lookup_wait_secs: parse_env("TOMO_GALLERY_LOOKUP_WAIT_SECS", 30u64),
        };

        let data_dir = optional("TOMO_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"));
        let script_dir = optional("TOMO_SCRIPT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./scripts"));
        let enable_hot_reload = bool_env("TOMO_ENABLE_HOT_RELOAD", true);

        let llm = if discord.enable_llm {
            build_llm_config()
        } else {
            tracing::info!("TOMO_ENABLE_LLM is false — LLM responses disabled by config");
            None
        };

        let ocr = {
            let latin = ocr_files("TOMO_OCR_LATIN_DET", "TOMO_OCR_LATIN_REC", "TOMO_OCR_LATIN_KEYS");
            let cjk = ocr_files("TOMO_OCR_CJK_DET", "TOMO_OCR_CJK_REC", "TOMO_OCR_CJK_KEYS");
            let cfg = OcrConfig { latin, cjk };
            cfg.is_some().then_some(cfg)
        };

        Ok(Self {
            discord,
            llm,
            ocr,
            data_dir,
            script_dir,
            enable_hot_reload,
        })
    }
}

// ---------- LLM config helpers ----------

/// Known providers and which env var holds their API key. Order here is
/// also the *default* priority order when neither `LLM_CHAIN` nor
/// `LLM_PROVIDERS` is set — Gemini first because it's the historical
/// default; high-quota providers (Groq, Cerebras) right behind it for
/// rotation; OpenRouter / Mistral last because their free quotas are
/// tighter.
const KNOWN_PROVIDERS: &[(&str, &str, &str)] = &[
    // (provider name, api key env, default-model env)
    ("gemini", "GEMINI_API_KEY", "GEMINI_MODEL"),
    ("groq", "GROQ_API_KEY", "GROQ_MODEL"),
    ("cerebras", "CEREBRAS_API_KEY", "CEREBRAS_MODEL"),
    ("openrouter", "OPENROUTER_API_KEY", "OPENROUTER_MODEL"),
    ("mistral", "MISTRAL_API_KEY", "MISTRAL_MODEL"),
];

/// Default models per provider when the user hasn't set `<PROVIDER>_MODEL`.
/// All are free-tier as of 2026-05.
fn default_models_for(provider: &str) -> Vec<String> {
    match provider {
        "gemini" => vec![
            // ai.google.dev/gemini-api/docs/pricing — free tier:
            // flash (10 RPM × 250 RPD) → 3-flash-preview → flash-lite
            // (15 RPM × 1000 RPD, highest quota) → pro (5 RPM × 100 RPD).
            "gemini-2.5-flash".into(),
            "gemini-3-flash-preview".into(),
            "gemini-2.5-flash-lite".into(),
            "gemini-2.5-pro".into(),
        ],
        "groq" => vec![
            // groq.com — free tier: 30 RPM × 14.4K RPD.
            "llama-3.3-70b-versatile".into(),
            "llama-3.1-8b-instant".into(),
        ],
        "cerebras" => vec![
            // cerebras.ai — free tier: ~1.7K RPD, 60K tok/min.
            "llama-3.3-70b".into(),
            "llama3.1-8b".into(),
        ],
        "openrouter" => vec![
            // openrouter.ai — free :free models: 20 RPM × 200 RPD.
            // Picks verified live from /api/v1/models on 2026-05-12; ordered
            // fast→big. The previous `google/gemini-2.0-flash-exp:free`
            // default was removed by Google before this build.
            "meta-llama/llama-3.3-70b-instruct:free".into(),
            "google/gemma-4-31b-it:free".into(),
            "qwen/qwen3-next-80b-a3b-instruct:free".into(),
            "openai/gpt-oss-120b:free".into(),
        ],
        "mistral" => vec![
            // mistral.ai — 1B tok/month, 2 RPM.
            "mistral-small-latest".into(),
            "open-mistral-nemo".into(),
        ],
        _ => Vec::new(),
    }
}

fn build_llm_config() -> Option<LlmConfig> {
    // Collect the API key for every known provider whose env var is set.
    let mut api_keys = std::collections::HashMap::new();
    let mut available_providers = Vec::new();
    for (name, key_env, _) in KNOWN_PROVIDERS {
        if let Some(key) = optional(key_env) {
            api_keys.insert((*name).to_string(), key);
            available_providers.push(*name);
        }
    }
    if api_keys.is_empty() {
        // Stay back-compat with the old `GEMINI_API_KEY missing` warning so
        // existing users see exactly the same diagnostic.
        tracing::warn!(
            "no LLM provider API keys found in environment — LLM responses disabled. \
             Looked for: GEMINI_API_KEY, GROQ_API_KEY, CEREBRAS_API_KEY, \
             OPENROUTER_API_KEY, MISTRAL_API_KEY."
        );
        return None;
    }

    let chain = build_chain(&available_providers, &api_keys);
    if chain.is_empty() {
        tracing::warn!(
            "LLM provider API keys are set but the resolved chain is empty. \
             Check `LLM_CHAIN` / `LLM_PROVIDERS` / `*_MODEL` env vars."
        );
        return None;
    }

    Some(LlmConfig {
        chain,
        api_keys,
        context_messages: parse_env("LLM_CONTEXT_MESSAGES", 10),
        max_output_tokens: parse_env("LLM_MAX_OUTPUT_TOKENS", 1024),
        rate_limit_per_minute: parse_env("LLM_RATE_LIMIT", 6),
        system_prompt: optional("LLM_SYSTEM_PROMPT").unwrap_or_else(|| {
            "You are Tomo, a helpful Discord bot. Reply concisely.".into()
        }),
    })
}

/// Build the flat (provider, model) chain from env. `LLM_CHAIN` wins when
/// set; otherwise fall back to `LLM_PROVIDERS` × per-provider `*_MODEL`
/// lists. Entries referencing a provider without an API key are dropped
/// with a warning so misconfigurations are auditable from the startup log.
fn build_chain(
    available_providers: &[&str],
    api_keys: &std::collections::HashMap<String, String>,
) -> Vec<LlmChainEntry> {
    // Power-user override: explicit flat chain.
    if let Some(raw) = optional("LLM_CHAIN") {
        let mut chain = Vec::new();
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            // Split on the *first* colon only — model names may contain
            // colons themselves (OpenRouter's `:free` suffix).
            let Some((provider, model)) = token.split_once(':') else {
                tracing::warn!(
                    entry = %token,
                    "LLM_CHAIN entry has no `provider:model` separator — skipping"
                );
                continue;
            };
            let provider = provider.trim().to_ascii_lowercase();
            let model = model.trim().to_string();
            if !api_keys.contains_key(&provider) {
                tracing::warn!(
                    provider = %provider,
                    model = %model,
                    "LLM_CHAIN references a provider without an API key — skipping"
                );
                continue;
            }
            chain.push(LlmChainEntry { provider, model });
        }
        return chain;
    }

    // Simple path: provider order × per-provider model list.
    let provider_order: Vec<String> = optional("LLM_PROVIDERS")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_else(|| available_providers.iter().map(|s| (*s).to_string()).collect());

    let mut chain = Vec::new();
    for provider in &provider_order {
        if !api_keys.contains_key(provider) {
            tracing::warn!(
                provider = %provider,
                "LLM_PROVIDERS lists `{provider}` but no `*_API_KEY` is set for it — skipping",
            );
            continue;
        }
        let model_env = KNOWN_PROVIDERS
            .iter()
            .find(|(n, _, _)| *n == provider.as_str())
            .map(|(_, _, m)| *m);
        let models = model_env
            .and_then(optional)
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default_models_for(provider));
        for model in models {
            chain.push(LlmChainEntry {
                provider: provider.clone(),
                model,
            });
        }
    }
    chain
}

fn require(key: &'static str) -> Result<String> {
    env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or(Error::MissingEnv(key))
}

fn optional(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn bool_env(key: &str, default: bool) -> bool {
    optional(key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn parse_env<T: FromStr>(key: &str, default: T) -> T {
    optional(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn ocr_files(det: &str, rec: &str, keys: &str) -> Option<OcrEngineFiles> {
    match (optional(det), optional(rec), optional(keys)) {
        (Some(d), Some(r), Some(k)) => Some(OcrEngineFiles {
            det: PathBuf::from(d),
            rec: PathBuf::from(r),
            keys: PathBuf::from(k),
        }),
        _ => None,
    }
}
