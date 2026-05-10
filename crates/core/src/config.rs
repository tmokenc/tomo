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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
    pub context_messages: usize,
    pub max_output_tokens: u32,
    pub rate_limit_per_minute: u32,
    pub system_prompt: String,
}

impl Config {
    /// Load `.env` (if present) and parse environment variables.
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();

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
        };

        let data_dir = optional("TOMO_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"));
        let script_dir = optional("TOMO_SCRIPT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./scripts"));
        let enable_hot_reload = bool_env("TOMO_ENABLE_HOT_RELOAD", true);

        let gemini = if discord.enable_gemini {
            optional("GEMINI_API_KEY").map(|api_key| GeminiConfig {
                api_key,
                model: optional("GEMINI_MODEL").unwrap_or_else(|| "gemini-2.5-flash".into()),
                context_messages: parse_env("GEMINI_CONTEXT_MESSAGES", 10),
                max_output_tokens: parse_env("GEMINI_MAX_OUTPUT_TOKENS", 1024),
                rate_limit_per_minute: parse_env("GEMINI_RATE_LIMIT", 6),
                system_prompt: optional("GEMINI_SYSTEM_PROMPT")
                    .unwrap_or_else(|| "You are Tomo, a helpful Discord bot. Reply concisely.".into()),
            })
        } else {
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
