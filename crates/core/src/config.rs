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
    pub gemini: Option<GeminiConfig>,
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
    pub enable_gemini: bool,
    pub enable_auto_triggers: bool,
    pub register_global: bool,
    pub dev_guild: Option<Id<GuildMarker>>,
    /// How long the gallery-lookup auto-triggers wait for a user to click the
    /// reaction before giving up and removing it. Seconds.
    pub gallery_lookup_wait_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiConfig {
    pub api_key: String,
    /// Ordered list of Gemini model IDs. The client tries them in order and
    /// rotates to the next one when the active model returns a 429. Use the
    /// free-tier ladder (Flash → Flash-Lite, etc.) to keep responding while
    /// the busy quota recovers.
    pub models: Vec<String>,
    pub context_messages: usize,
    pub max_output_tokens: u32,
    pub rate_limit_per_minute: u32,
    pub system_prompt: String,
}

impl GeminiConfig {
    /// The model the client prefers — first in the chain. Always present
    /// because `from_env` rejects empty chains.
    pub fn primary_model(&self) -> &str {
        self.models
            .first()
            .map(String::as_str)
            .unwrap_or("gemini-2.5-flash")
    }
}

impl Config {
    /// Load `.env` (if present) and parse environment variables.
    ///
    /// `dotenvy::dotenv()` walks up from the *current working directory*
    /// looking for a `.env` file. We log which file (if any) it found so
    /// "I have it in .env but the bot says it's missing" is diagnosable from
    /// the startup output — the usual cause is running the binary from a
    /// directory above (or below) where the `.env` lives.
    pub fn from_env() -> Result<Self> {
        match dotenvy::dotenv() {
            Ok(path) => tracing::info!(path = %path.display(), "loaded .env"),
            Err(e) if e.not_found() => {
                tracing::warn!(
                    cwd = ?env::current_dir().ok(),
                    "no .env file found — relying on process environment only"
                );
            }
            Err(e) => tracing::warn!(error = %e, "failed to load .env"),
        }

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
            enable_gemini: bool_env("TOMO_ENABLE_GEMINI", true),
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

        let gemini = if discord.enable_gemini {
            // Explicit diagnostics here because "I have it in .env but the
            // bot says it's missing" comes up a lot when the var is set but
            // the dotenv file isn't on the search path the binary runs from,
            // or the value is quoted/whitespaced into uselessness.
            match env::var("GEMINI_API_KEY") {
                Ok(raw) if !raw.trim().is_empty() => {
                    // Free-tier fallback chain (verified against
                    // ai.google.dev/gemini-api/docs/pricing on 2026-05-11).
                    // Indicative free-tier RPM × RPD:
                    //   gemini-2.5-flash         10 ×  250
                    //   gemini-3-flash-preview   10 ×  100  (preview)
                    //   gemini-2.5-flash-lite    15 × 1000  (most permissive)
                    //   gemini-2.5-pro            5 ×  100  (slowest, smartest)
                    // Order: quality first, fall back to the high-quota lite,
                    // last resort the slow Pro. User-overridable via env.
                    let models = optional("GEMINI_MODEL")
                        .map(|raw| {
                            raw.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                        })
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| {
                            vec![
                                "gemini-2.5-flash".into(),
                                "gemini-3-flash-preview".into(),
                                "gemini-2.5-flash-lite".into(),
                                "gemini-2.5-pro".into(),
                            ]
                        });
                    Some(GeminiConfig {
                        api_key: raw.trim().to_string(),
                        models,
                        context_messages: parse_env("GEMINI_CONTEXT_MESSAGES", 10),
                        max_output_tokens: parse_env("GEMINI_MAX_OUTPUT_TOKENS", 1024),
                        rate_limit_per_minute: parse_env("GEMINI_RATE_LIMIT", 6),
                        system_prompt: optional("GEMINI_SYSTEM_PROMPT").unwrap_or_else(|| {
                            "You are Tomo, a helpful Discord bot. Reply concisely.".into()
                        }),
                    })
                }
                Ok(raw) => {
                    tracing::warn!(
                        "GEMINI_API_KEY is set but empty/whitespace ({} bytes) — gemini disabled",
                        raw.len()
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        "GEMINI_API_KEY not present in environment — gemini disabled. \
                         (Loaded vars start with: {})",
                        env::vars().map(|(k, _)| k).filter(|k| k.starts_with("GEMINI_")).collect::<Vec<_>>().join(", ")
                    );
                    None
                }
            }
        } else {
            tracing::info!("TOMO_ENABLE_GEMINI is false — gemini disabled by config");
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
            gemini,
            ocr,
            data_dir,
            script_dir,
            enable_hot_reload,
        })
    }
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
