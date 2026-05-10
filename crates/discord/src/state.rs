use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use twilight_cache_inmemory::DefaultInMemoryCache;
use twilight_http::Client;
use twilight_model::id::Id;
use twilight_model::id::marker::{ApplicationMarker, UserMarker};
use twilight_model::util::ImageHash;
use twilight_standby::Standby;

use ocr_rs::OcrEngine;

/// Named OCR engine. The name shows up in command output so users can tell
/// which engine produced which text on mixed-script documents.
#[derive(Clone)]
pub struct OcrSlot {
    pub name: &'static str,
    pub engine: Arc<OcrEngine>,
}

use tomo_core::Config;
use tomo_core::error::{Error, Result};
use tomo_db::KvStore;
use tomo_gemini::{ConversationStore, GeminiClient, RateLimiter};
use tomo_requester::Requester;
use tomo_scripting::ScriptManager;
use tomo_stats::Stats;

use crate::command::CommandRegistry;
use crate::trigger::TriggerRegistry;

/// Cheaply-clonable handle to bot state. Every component (event loop,
/// commands, triggers, gemini handler) takes this and only this.
pub type Bot = Arc<BotState>;

/// Holds every piece of long-lived state the bot needs at runtime.
pub struct BotState {
    pub http: Arc<Client>,
    pub cache: Arc<DefaultInMemoryCache>,
    pub standby: Arc<Standby>,
    pub config: Arc<Config>,
    pub db: Arc<dyn KvStore>,
    pub stats: Stats,
    pub scripts: Arc<ScriptManager>,
    pub requester: Requester,
    pub gemini: Option<GeminiContext>,
    /// Zero, one, or two OCR engines (Latin / CJK). The `ocr` command runs
    /// every loaded engine and merges the output, so configuring both gives
    /// full coverage of en / cs / vi / zh / ja.
    pub ocr: Vec<OcrSlot>,
    pub commands: Arc<ArcSwap<CommandRegistry>>,
    pub triggers: Arc<ArcSwap<TriggerRegistry>>,
    /// Everything we learned about the bot at startup — username,
    /// application id, owner, etc.
    pub identity: BotIdentity,
    /// User IDs allowed to use `owner_only` commands and the master prefix.
    /// Sourced from `TOMO_OWNERS`, falling back to the application owner.
    pub owners: Vec<Id<UserMarker>>,
    pub started_at: DateTime<Utc>,
    /// `chrono`'s `num_days_from_ce` of the last day we DM'd owners about a
    /// Gemini quota error. Atomic CAS on this guarantees only one DM per
    /// UTC day even under concurrent errors. `0` means "never alerted".
    pub gemini_quota_alert_day: AtomicI64,
}

#[derive(Clone)]
pub struct GeminiContext {
    pub client: Arc<GeminiClient>,
    pub conversations: Arc<ConversationStore>,
    pub rate_limit: Arc<RateLimiter>,
}

impl BotState {
    pub fn is_owner(&self, user_id: Id<UserMarker>) -> bool {
        self.owners.contains(&user_id)
    }

    pub fn uptime(&self) -> chrono::Duration {
        Utc::now() - self.started_at
    }
}

// ---------- Identity ----------

/// Identifying information about a bot owner — populated from the Discord
/// application info on startup.
#[derive(Debug, Clone)]
pub struct OwnerInfo {
    pub id: Id<UserMarker>,
    pub username: String,
}

/// Everything we know about *this* bot, fetched once at startup from
/// `/users/@me` and `/oauth2/applications/@me`.
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub user_id: Id<UserMarker>,
    pub username: String,
    pub discriminator: u16,
    pub avatar_url: Option<String>,
    pub verified: bool,
    pub bot: bool,

    pub application_id: Id<ApplicationMarker>,
    pub application_name: String,
    pub application_description: String,
    pub bot_public: bool,
    pub bot_require_code_grant: bool,

    pub owner: Option<OwnerInfo>,
}

impl BotIdentity {
    /// Hit `/users/@me` + `/oauth2/applications/@me` and roll the results
    /// into one struct.
    pub async fn fetch(http: &Client) -> Result<Self> {
        let user = http
            .current_user()
            .await
            .map_err(|e| Error::config(format!("current_user: {e}")))?
            .model()
            .await
            .map_err(|e| Error::config(format!("decode user: {e}")))?;

        let app = http
            .current_user_application()
            .await
            .map_err(|e| Error::config(format!("current_user_application: {e}")))?
            .model()
            .await
            .map_err(|e| Error::config(format!("decode app: {e}")))?;

        let avatar_url = user
            .avatar
            .as_ref()
            .map(|hash| user_avatar_url(user.id, hash));

        let owner = app.owner.map(|u| OwnerInfo { id: u.id, username: u.name });

        Ok(Self {
            user_id: user.id,
            username: user.name,
            discriminator: user.discriminator,
            avatar_url,
            verified: user.verified.unwrap_or(false),
            bot: user.bot,
            application_id: app.id,
            application_name: app.name,
            application_description: app.description,
            bot_public: app.bot_public,
            bot_require_code_grant: app.bot_require_code_grant,
            owner,
        })
    }

    /// `"Tomo#0042"` (legacy discriminator format) or `"Tomo"` if the
    /// account has migrated to the new username system.
    pub fn full_username(&self) -> String {
        if self.discriminator == 0 {
            self.username.clone()
        } else {
            format!("{}#{:04}", self.username, self.discriminator)
        }
    }
}

/// Compute a 256-px avatar URL from the bot's user id + image hash.
fn user_avatar_url(id: Id<UserMarker>, hash: &ImageHash) -> String {
    let hash_str = hash.to_string();
    let ext = if hash_str.starts_with("a_") { "gif" } else { "png" };
    format!("https://cdn.discordapp.com/avatars/{id}/{hash_str}.{ext}?size=256")
}
