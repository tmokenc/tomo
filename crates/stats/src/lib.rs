//! Stats collection backed by [`tomo_db::KvStore`].
//!
//! Every command, message, and Gemini call is recorded as a counter
//! increment. The hot path is non-blocking: call-sites push an [`Event`] onto
//! a flume channel and a dedicated writer task drains the channel in batches.
//!
//! Reads (top commands, per-user stats, global totals) are fully async — they
//! call `KvStore::list_prefix` or `KvStore::get` under the hood.

use std::sync::Arc;
use std::time::Duration;

use flume::Sender;
use serde::{Deserialize, Serialize};
use tracing::{error, warn};
use twilight_model::id::Id;
use twilight_model::id::marker::{GuildMarker, UserMarker};

use tomo_core::error::{Error, Result};
use tomo_db::{KvOp, KvStore};
use tomo_db::codec::u64_from_bytes;

/// Bounded channel size — back-pressures producers if the writer cannot keep up.
const CHANNEL_CAPACITY: usize = 8_192;

/// Partition (column family / fjall keyspace) the stats counters live in.
pub const PARTITION: &str = "stats";

/// Public, cheaply-cloneable stats handle.
#[derive(Clone)]
pub struct Stats {
    tx: Sender<Event>,
    store: Arc<dyn KvStore>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CommandKind {
    Prefix,
    Slash,
    Reaction,
    Auto,
    Script,
}

impl CommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Slash => "slash",
            Self::Reaction => "reaction",
            Self::Auto => "auto",
            Self::Script => "script",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandUsage {
    pub command: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserStats {
    pub user_id: u64,
    pub messages: u64,
    pub commands: u64,
    pub gemini_calls: u64,
    pub gemini_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuildStats {
    pub guild_id: u64,
    pub messages: u64,
    pub commands: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalStats {
    pub messages: u64,
    pub commands: u64,
    pub gemini_calls: u64,
    pub gemini_tokens: u64,
}

#[derive(Debug)]
enum Event {
    Command {
        name: String,
        kind: CommandKind,
        user_id: u64,
        guild_id: Option<u64>,
        success: bool,
        duration_ms: u64,
    },
    Message {
        user_id: u64,
        guild_id: Option<u64>,
    },
    Gemini {
        user_id: u64,
        guild_id: Option<u64>,
        prompt_tokens: u32,
        output_tokens: u32,
    },
}

impl Stats {
    /// Wire up a [`Stats`] handle to the given backing store and start the
    /// writer task.
    pub fn new(store: Arc<dyn KvStore>) -> Self {
        let (tx, rx) = flume::bounded::<Event>(CHANNEL_CAPACITY);
        spawn_writer(Arc::clone(&store), rx);
        Self { tx, store }
    }

    // ---- Recording (non-blocking, fire-and-forget) ----

    pub fn record_command(
        &self,
        name: impl Into<String>,
        kind: CommandKind,
        user_id: Id<UserMarker>,
        guild_id: Option<Id<GuildMarker>>,
        success: bool,
        duration: Duration,
    ) {
        self.send(Event::Command {
            name: name.into(),
            kind,
            user_id: user_id.get(),
            guild_id: guild_id.map(|g| g.get()),
            success,
            duration_ms: duration.as_millis() as u64,
        });
    }

    pub fn record_message(&self, user_id: Id<UserMarker>, guild_id: Option<Id<GuildMarker>>) {
        self.send(Event::Message {
            user_id: user_id.get(),
            guild_id: guild_id.map(|g| g.get()),
        });
    }

    pub fn record_gemini(
        &self,
        user_id: Id<UserMarker>,
        guild_id: Option<Id<GuildMarker>>,
        prompt_tokens: u32,
        output_tokens: u32,
    ) {
        self.send(Event::Gemini {
            user_id: user_id.get(),
            guild_id: guild_id.map(|g| g.get()),
            prompt_tokens,
            output_tokens,
        });
    }

    fn send(&self, ev: Event) {
        if let Err(e) = self.tx.try_send(ev) {
            warn!(error = %e, "stats channel full, dropping event");
        }
    }

    // ---- Queries (async) ----

    pub async fn top_commands(&self, limit: usize) -> Result<Vec<CommandUsage>> {
        let entries = self
            .store
            .list_prefix(PARTITION, bytes::Bytes::from_static(key_prefix::COMMAND_NAME.as_bytes()))
            .await
            .map_err(map_db_err)?;
        let mut out: Vec<CommandUsage> = entries
            .into_iter()
            .map(|(k, v)| CommandUsage {
                command: String::from_utf8_lossy(&k[key_prefix::COMMAND_NAME.len()..]).into_owned(),
                count: u64_from_bytes(&v),
            })
            .collect();
        out.sort_by_key(|row| std::cmp::Reverse(row.count));
        out.truncate(limit);
        Ok(out)
    }

    pub async fn global(&self) -> Result<GlobalStats> {
        Ok(GlobalStats {
            messages: read_u64(&self.store, key_global::MESSAGES).await,
            commands: read_u64(&self.store, key_global::COMMANDS).await,
            gemini_calls: read_u64(&self.store, key_global::GEMINI_CALLS).await,
            gemini_tokens: read_u64(&self.store, key_global::GEMINI_TOKENS).await,
        })
    }

    pub async fn user_stats(&self, user_id: Id<UserMarker>) -> Result<UserStats> {
        let id = user_id.get();
        Ok(UserStats {
            user_id: id,
            messages: read_u64(&self.store, &key_user::messages(id)).await,
            commands: read_u64(&self.store, &key_user::commands(id)).await,
            gemini_calls: read_u64(&self.store, &key_user::gemini(id)).await,
            gemini_tokens: read_u64(&self.store, &key_user::gemini_tokens(id)).await,
        })
    }

    pub async fn guild_stats(&self, guild_id: Id<GuildMarker>) -> Result<GuildStats> {
        let id = guild_id.get();
        Ok(GuildStats {
            guild_id: id,
            messages: read_u64(&self.store, &key_guild::messages(id)).await,
            commands: read_u64(&self.store, &key_guild::commands(id)).await,
        })
    }
}

fn map_db_err(e: tomo_db::DbError) -> Error {
    Error::config(format!("stats db: {e}"))
}

async fn read_u64(store: &Arc<dyn KvStore>, key: &str) -> u64 {
    let key_bytes = bytes::Bytes::copy_from_slice(key.as_bytes());
    match store.get(PARTITION, key_bytes).await {
        Ok(Some(bytes)) => u64_from_bytes(&bytes),
        Ok(None) => 0,
        Err(e) => {
            warn!(key, error = %e, "failed reading counter");
            0
        }
    }
}

fn spawn_writer(store: Arc<dyn KvStore>, rx: flume::Receiver<Event>) {
    tokio::spawn(async move {
        loop {
            // Block until at least one event arrives.
            let Ok(first) = rx.recv_async().await else { return };
            let mut batch = Vec::with_capacity(64);
            push_events(&mut batch, first);
            // Greedy-drain anything else queued without blocking.
            while let Ok(ev) = rx.try_recv() {
                push_events(&mut batch, ev);
                if batch.len() >= 256 {
                    break;
                }
            }
            if let Err(e) = store.batch(batch).await {
                error!(error = %e, "stats writer batch failed");
            }
        }
    });
}

fn push_events(batch: &mut Vec<KvOp>, ev: Event) {
    match ev {
        Event::Command { name, kind, user_id, guild_id, success, duration_ms } => {
            push_inc(batch, key_global::COMMANDS, 1);
            push_inc(batch, &key_command_name(&name), 1);
            push_inc(batch, &key_command_kind(kind.as_str()), 1);
            push_inc(batch, &key_user::commands(user_id), 1);
            push_inc(batch, &key_user_command(user_id, &name), 1);
            if let Some(gid) = guild_id {
                push_inc(batch, &key_guild::commands(gid), 1);
            }
            if !success {
                push_inc(batch, &key_command_failure(&name), 1);
            }
            // Latency aggregate — running sum, used to compute averages.
            push_inc(batch, &key_command_latency_sum(&name), duration_ms);
        }
        Event::Message { user_id, guild_id } => {
            push_inc(batch, key_global::MESSAGES, 1);
            push_inc(batch, &key_user::messages(user_id), 1);
            if let Some(gid) = guild_id {
                push_inc(batch, &key_guild::messages(gid), 1);
            }
        }
        Event::Gemini { user_id, guild_id, prompt_tokens, output_tokens } => {
            push_inc(batch, key_global::GEMINI_CALLS, 1);
            push_inc(batch, key_global::GEMINI_TOKENS, (prompt_tokens + output_tokens) as u64);
            push_inc(batch, &key_user::gemini(user_id), 1);
            push_inc(batch, &key_user::gemini_tokens(user_id), (prompt_tokens + output_tokens) as u64);
            if let Some(gid) = guild_id {
                push_inc(batch, &key_guild::gemini(gid), 1);
            }
        }
    }
}

fn push_inc(batch: &mut Vec<KvOp>, key: &str, by: u64) {
    batch.push(KvOp::Increment {
        partition: PARTITION.into(),
        key: bytes::Bytes::copy_from_slice(key.as_bytes()),
        by,
    });
}

// ---- Key naming helpers (kept centralized to make schema audits trivial) ----

mod key_prefix {
    pub const COMMAND_NAME: &str = "cmd:name:";
    pub const COMMAND_KIND: &str = "cmd:kind:";
    pub const COMMAND_FAILURE: &str = "cmd:fail:";
    pub const COMMAND_LATENCY: &str = "cmd:lat:";
}

mod key_global {
    pub const MESSAGES: &str = "global:messages";
    pub const COMMANDS: &str = "global:commands";
    pub const GEMINI_CALLS: &str = "global:gemini_calls";
    pub const GEMINI_TOKENS: &str = "global:gemini_tokens";
}

mod key_user {
    pub fn messages(id: u64) -> String { format!("user:{id}:messages") }
    pub fn commands(id: u64) -> String { format!("user:{id}:commands") }
    pub fn gemini(id: u64) -> String   { format!("user:{id}:gemini") }
    pub fn gemini_tokens(id: u64) -> String { format!("user:{id}:gemini_tokens") }
}

mod key_guild {
    pub fn messages(id: u64) -> String { format!("guild:{id}:messages") }
    pub fn commands(id: u64) -> String { format!("guild:{id}:commands") }
    pub fn gemini(id: u64) -> String   { format!("guild:{id}:gemini") }
}

fn key_command_name(name: &str) -> String {
    format!("{}{name}", key_prefix::COMMAND_NAME)
}
fn key_command_kind(kind: &str) -> String {
    format!("{}{kind}", key_prefix::COMMAND_KIND)
}
fn key_command_failure(name: &str) -> String {
    format!("{}{name}", key_prefix::COMMAND_FAILURE)
}
fn key_command_latency_sum(name: &str) -> String {
    format!("{}{name}", key_prefix::COMMAND_LATENCY)
}
fn key_user_command(user_id: u64, name: &str) -> String {
    format!("user:{user_id}:cmd:{name}")
}
