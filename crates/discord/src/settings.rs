//! Per-guild settings backed by `KvStore`.
//!
//! The struct is intentionally `#[serde(default)]` on every field plus a
//! catch-all rename layer so we can add new keys later without breaking
//! deserialisation of older blobs already on disk. JSON is the wire format
//! (one blob per guild), keyed by the guild id as 8 big-endian bytes.
//!
//! A `DashMap` cache sits in front of the DB. The hot path (prefix lookup
//! on every message, log-channel lookup on every edit/delete) hits memory;
//! the DB is consulted only on cache miss or on save.

use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::warn;
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker};

use tomo_db::{u64_to_bytes, KvStore};

/// fjall keyspace / column family for per-guild settings. Bumping this name
/// is a one-way migration — keep it stable.
pub const PARTITION: &str = "guild_settings";

/// Everything configurable from `/settings`. Every field is optional / has
/// a default so old blobs deserialise fine after we add new fields here.
///
/// The struct is `#[serde(default)]` so a missing field just falls back to
/// `Default::default()` — no migration needed when we add a new option.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildSettings {
    /// Where edit / delete log embeds get posted. `None` disables logging
    /// for this guild.
    pub log_channel: Option<Id<ChannelMarker>>,
    /// Per-guild command prefix override. `None` (or empty) falls back to
    /// the global `TOMO_PREFIX`.
    pub prefix: Option<String>,
    // Future fields:
    //   pub welcome_channel: Option<...>,
    //   pub auto_mod_level: Option<u8>,
    //   ...
    // Each new field needs `#[serde(default)]` semantics (covered by the
    // struct-level attribute) so older blobs keep loading.
}

impl GuildSettings {
    /// Effective prefix for this guild — trims the override and falls back
    /// to the global default if it's missing or whitespace.
    pub fn effective_prefix<'a>(&'a self, default: &'a str) -> &'a str {
        match self.prefix.as_deref() {
            Some(p) if !p.trim().is_empty() => p,
            _ => default,
        }
    }
}

/// Read-through cache for `GuildSettings`. Cheap to clone.
#[derive(Clone)]
pub struct SettingsStore {
    db: Arc<dyn KvStore>,
    cache: Arc<DashMap<Id<GuildMarker>, GuildSettings>>,
}

impl SettingsStore {
    pub fn new(db: Arc<dyn KvStore>) -> Self {
        Self {
            db,
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Resolve the settings for a guild. Returns `Default::default()` for
    /// guilds with no row on disk yet — so callers can read every field
    /// unconditionally without `Option`-wrapping the result.
    pub async fn get(&self, guild: Id<GuildMarker>) -> GuildSettings {
        if let Some(hit) = self.cache.get(&guild) {
            return hit.clone();
        }
        let key = Bytes::copy_from_slice(&u64_to_bytes(guild.get()));
        match self.db.get(PARTITION, key).await {
            Ok(Some(raw)) => {
                let parsed: GuildSettings = match serde_json::from_slice(&raw) {
                    Ok(s) => s,
                    Err(e) => {
                        // Bad JSON on disk — log and fall back to defaults
                        // rather than crash. Saving fresh settings via
                        // `set` will overwrite the bad blob.
                        warn!(
                            guild = %guild,
                            error = %e,
                            "settings: stored JSON failed to deserialise — using defaults"
                        );
                        GuildSettings::default()
                    }
                };
                self.cache.insert(guild, parsed.clone());
                parsed
            }
            Ok(None) => {
                let blank = GuildSettings::default();
                self.cache.insert(guild, blank.clone());
                blank
            }
            Err(e) => {
                warn!(guild = %guild, error = %e, "settings: db read failed — using defaults");
                GuildSettings::default()
            }
        }
    }

    /// Persist + refresh the cache.
    pub async fn set(
        &self,
        guild: Id<GuildMarker>,
        settings: GuildSettings,
    ) -> Result<(), tomo_db::DbError> {
        let raw = serde_json::to_vec(&settings)
            .map_err(|e| tomo_db::DbError::Backend(format!("settings serialise: {e}")))?;
        let key = Bytes::copy_from_slice(&u64_to_bytes(guild.get()));
        self.db.insert(PARTITION, key, Bytes::from(raw)).await?;
        self.cache.insert(guild, settings);
        Ok(())
    }

    /// Drop a guild's row entirely (e.g. on guild-leave).
    pub async fn remove(&self, guild: Id<GuildMarker>) -> Result<(), tomo_db::DbError> {
        let key = Bytes::copy_from_slice(&u64_to_bytes(guild.get()));
        self.db.remove(PARTITION, key).await?;
        self.cache.remove(&guild);
        Ok(())
    }
}

/// Parse a user-provided channel identifier. Accepts:
///   * bare id        — `"123456789012345678"`
///   * mention syntax — `"<#123456789012345678>"`
///   * empty string   — returns `Ok(None)` so the caller can clear the field.
///
/// Anything else returns an `Err` so the modal-submit handler can echo a
/// "couldn't read your input" message back to the user.
pub fn parse_channel_input(raw: &str) -> Result<Option<Id<ChannelMarker>>, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // <#id>
    if let Some(rest) = trimmed.strip_prefix("<#").and_then(|s| s.strip_suffix('>')) {
        return rest
            .parse::<u64>()
            .ok()
            .and_then(Id::new_checked)
            .map(Some)
            .ok_or("not a valid channel id");
    }
    // bare digits
    trimmed
        .parse::<u64>()
        .ok()
        .and_then(Id::new_checked)
        .map(Some)
        .ok_or("not a valid channel id")
}

#[cfg(test)]
mod tests {
    use super::{parse_channel_input, GuildSettings};

    #[test]
    fn json_round_trip_with_unknown_fields_survives() {
        // Forward-compatibility: a future build added a new field; this
        // build must still decode the blob fine, treating the unknown key
        // as a no-op.
        let raw = r#"{"log_channel": "123456789012345678",
                       "prefix": "%",
                       "some_future_setting": {"nested": true}}"#;
        let parsed: GuildSettings = serde_json::from_str(raw).expect("decode");
        assert!(parsed.log_channel.is_some());
        assert_eq!(parsed.prefix.as_deref(), Some("%"));
    }

    #[test]
    fn parses_bare_channel_id() {
        assert_eq!(
            parse_channel_input("123456789012345678").unwrap().map(|i| i.get()),
            Some(123456789012345678),
        );
    }

    #[test]
    fn parses_mention_syntax() {
        assert_eq!(
            parse_channel_input("<#987654321098765432>").unwrap().map(|i| i.get()),
            Some(987654321098765432),
        );
    }

    #[test]
    fn empty_clears() {
        assert_eq!(parse_channel_input("").unwrap(), None);
        assert_eq!(parse_channel_input("   ").unwrap(), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_channel_input("not-a-channel").is_err());
        assert!(parse_channel_input("<#abc>").is_err());
    }

    #[test]
    fn effective_prefix_falls_back_when_unset() {
        let s = GuildSettings::default();
        assert_eq!(s.effective_prefix("tomo>"), "tomo>");
    }

    #[test]
    fn effective_prefix_uses_override() {
        let s = GuildSettings { prefix: Some("%".into()), ..Default::default() };
        assert_eq!(s.effective_prefix("tomo>"), "%");
    }

    #[test]
    fn effective_prefix_treats_whitespace_as_unset() {
        let s = GuildSettings { prefix: Some("   ".into()), ..Default::default() };
        assert_eq!(s.effective_prefix("tomo>"), "tomo>");
    }
}
