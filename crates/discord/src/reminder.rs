//! Reminder subsystem.
//!
//! Storage layout (in the `reminders` keyspace of [`tomo_db::KvStore`]):
//! - **key**: 8 bytes big-endian `when_unix` + 8 bytes big-endian `id`.
//!   The natural sort order on the LSM-tree gives us "soonest first" for
//!   free, so the worker just reads the head of the partition.
//! - **value**: serde_json-encoded [`Reminder`].
//!
//! A single tokio task per `DiscordService::run` invocation polls the next
//! due entry, sleeps precisely until it should fire (capped at 60 s so a
//! shutdown signal can still be picked up promptly), DMs the user, and
//! removes the row.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

use tomo_core::error::{Error, Result};
use tomo_db::Bytes;

use crate::state::Bot;

pub const PARTITION: &str = "reminders";

/// Hard cap so a single user can't fill the partition.
pub const MAX_PER_USER: usize = 5;

/// 90-day ceiling on how far in the future a reminder can be scheduled.
pub const MAX_DURATION: Duration = Duration::from_secs(60 * 60 * 24 * 90);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: u64,
    pub user_id: u64,
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub when_unix: i64,
    pub content: String,
}

impl Reminder {
    pub fn db_key(&self) -> Bytes {
        let mut out = Vec::with_capacity(16);
        out.extend_from_slice(&self.when_unix.to_be_bytes());
        out.extend_from_slice(&self.id.to_be_bytes());
        Bytes::from(out)
    }
}

// ---------- CRUD ----------

pub async fn schedule(bot: &Bot, r: &Reminder) -> Result<()> {
    let value = serde_json::to_vec(r).map_err(|e| Error::config(format!("encode reminder: {e}")))?;
    bot.db
        .insert(PARTITION, r.db_key(), Bytes::from(value))
        .await
        .map_err(|e| Error::config(format!("save reminder: {e}")))?;
    Ok(())
}

pub async fn list_user(bot: &Bot, user_id: u64) -> Result<Vec<Reminder>> {
    let entries = bot
        .db
        .list_prefix(PARTITION, Bytes::new())
        .await
        .map_err(|e| Error::config(format!("list reminders: {e}")))?;
    let mut out: Vec<Reminder> = entries
        .into_iter()
        .filter_map(|(_, v)| serde_json::from_slice::<Reminder>(&v).ok())
        .filter(|r| r.user_id == user_id)
        .collect();
    out.sort_by_key(|r| r.when_unix);
    Ok(out)
}

pub async fn delete(bot: &Bot, r: &Reminder) -> Result<()> {
    bot.db
        .remove(PARTITION, r.db_key())
        .await
        .map_err(|e| Error::config(format!("delete reminder: {e}")))?;
    Ok(())
}

// ---------- Background worker ----------

pub async fn run_loop(bot: Bot, mut shutdown: watch::Receiver<bool>) {
    info!("reminder loop started");
    loop {
        let sleep_until = match next_due_in(&bot).await {
            Some(d) => Instant::now() + d.min(Duration::from_secs(60)),
            None => Instant::now() + Duration::from_secs(60),
        };

        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                info!("reminder loop shutting down");
                return;
            }
            _ = tokio::time::sleep_until(sleep_until) => {}
        }

        if let Err(e) = fire_due(&bot).await {
            warn!(error = %e, "reminder fire batch failed");
        }
    }
}

async fn next_due_in(bot: &Bot) -> Option<Duration> {
    let now = Utc::now().timestamp();
    let entries = bot
        .db
        .list_prefix(PARTITION, Bytes::new())
        .await
        .ok()?;
    // Keys are sorted, so the first entry whose deserialised payload parses
    // is the earliest pending reminder.
    let (_, v) = entries.into_iter().next()?;
    let reminder: Reminder = serde_json::from_slice(&v).ok()?;
    let secs = reminder.when_unix.saturating_sub(now);
    if secs <= 0 {
        Some(Duration::ZERO)
    } else {
        Some(Duration::from_secs(secs as u64))
    }
}

async fn fire_due(bot: &Bot) -> Result<()> {
    let now = Utc::now().timestamp();
    let entries = bot
        .db
        .list_prefix(PARTITION, Bytes::new())
        .await
        .map_err(|e| Error::config(format!("list reminders: {e}")))?;

    for (key, value) in entries {
        let Ok(reminder) = serde_json::from_slice::<Reminder>(&value) else {
            // Garbled row — wipe it.
            let _ = bot.db.remove(PARTITION, key).await;
            continue;
        };
        if reminder.when_unix > now {
            break; // keys sorted, anything after is also future
        }

        debug!(id = reminder.id, user = reminder.user_id, "firing reminder");
        if let Err(e) = deliver(bot, &reminder).await {
            warn!(id = reminder.id, error = %e, "failed to deliver reminder");
        }
        if let Err(e) = bot.db.remove(PARTITION, key).await {
            warn!(id = reminder.id, error = %e, "failed to delete fired reminder");
        }
    }
    Ok(())
}

async fn deliver(bot: &Bot, r: &Reminder) -> Result<()> {
    let user_id: Id<UserMarker> = Id::new(r.user_id);
    let channel = bot
        .http
        .create_private_channel(user_id)
        .await
        .map_err(|e| Error::config(format!("dm channel: {e}")))?
        .model()
        .await
        .map_err(|e| Error::config(format!("decode dm channel: {e}")))?;

    let set_at = DateTime::from_timestamp(r.when_unix, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "(unknown)".into());

    let body = format!(
        "⏰ **Reminder!**\n{}\n\nWas set to fire at `{set_at}`.",
        r.content
    );

    bot.http
        .create_message(channel.id)
        .content(&body)
        .await
        .map_err(|e| Error::config(format!("send reminder: {e}")))?;
    Ok(())
}

/// Generate a fresh, monotonically-increasing-ish id for new reminders.
/// 8 bytes of nanosecond clock — unique enough at our scale.
pub fn fresh_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
