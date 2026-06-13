//! Persistence layer: writes go through a flume channel into a single
//! background task; reads happen at startup via [`load_all`].

use std::sync::Arc;

use bytes::Bytes;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, warn};
use twilight_model::id::Id;
use twilight_model::id::marker::{
    AutoModerationRuleMarker, ChannelMarker, EmojiMarker, GenericMarker, GuildMarker,
    IntegrationMarker, MessageMarker, RoleMarker, ScheduledEventMarker, StageMarker, StickerMarker,
    UserMarker,
};

use tomo_db::{KvOp, KvStore};

use crate::PARTITION;

/// One persisted mutation. The background writer drains these from a flume
/// channel and turns them into `tomo_db::KvOp`s.
#[derive(Debug, Clone)]
pub enum WriteOp {
    PutChannel(Id<ChannelMarker>, Bytes),
    DeleteChannel(Id<ChannelMarker>),

    PutUser(Id<UserMarker>, Bytes),
    DeleteUser(Id<UserMarker>),

    PutMessage(Id<MessageMarker>, Bytes),
    DeleteMessage(Id<MessageMarker>),

    PutChannelMessages(Id<ChannelMarker>, Bytes),
    DeleteChannelMessages(Id<ChannelMarker>),

    PutGuild(Id<GuildMarker>, Bytes),
    DeleteGuild(Id<GuildMarker>),

    PutMember(Id<GuildMarker>, Id<UserMarker>, Bytes),
    DeleteMember(Id<GuildMarker>, Id<UserMarker>),

    PutRole(Id<RoleMarker>, Bytes),
    DeleteRole(Id<RoleMarker>),

    PutEmoji(Id<EmojiMarker>, Bytes),
    DeleteEmoji(Id<EmojiMarker>),

    PutSticker(Id<StickerMarker>, Bytes),
    DeleteSticker(Id<StickerMarker>),

    PutStageInstance(Id<StageMarker>, Bytes),
    DeleteStageInstance(Id<StageMarker>),

    PutScheduledEvent(Id<ScheduledEventMarker>, Bytes),
    DeleteScheduledEvent(Id<ScheduledEventMarker>),

    PutIntegration(Id<GuildMarker>, Id<IntegrationMarker>, Bytes),
    DeleteIntegration(Id<GuildMarker>, Id<IntegrationMarker>),

    PutAutoModRule(Id<AutoModerationRuleMarker>, Bytes),
    DeleteAutoModRule(Id<AutoModerationRuleMarker>),

    PutCurrentUser(Bytes),

    /// Set of guild members / channels / roles etc. — used for index
    /// persistence so we can resurrect indexes on next boot.
    PutIndex(&'static str, Id<GenericMarker>, Bytes),
    DeleteIndex(&'static str, Id<GenericMarker>),
}

// ---------- key helpers ----------

pub fn key_channel(id: Id<ChannelMarker>) -> Bytes {
    format!("c:{id}").into_bytes().into()
}
pub fn key_user(id: Id<UserMarker>) -> Bytes {
    format!("u:{id}").into_bytes().into()
}
pub fn key_message(id: Id<MessageMarker>) -> Bytes {
    format!("m:{id}").into_bytes().into()
}
pub fn key_channel_messages(id: Id<ChannelMarker>) -> Bytes {
    format!("cm:{id}").into_bytes().into()
}
pub fn key_guild(id: Id<GuildMarker>) -> Bytes {
    format!("g:{id}").into_bytes().into()
}
pub fn key_member(guild: Id<GuildMarker>, user: Id<UserMarker>) -> Bytes {
    format!("mb:{guild}:{user}").into_bytes().into()
}
pub fn key_role(id: Id<RoleMarker>) -> Bytes {
    format!("r:{id}").into_bytes().into()
}
pub fn key_emoji(id: Id<EmojiMarker>) -> Bytes {
    format!("e:{id}").into_bytes().into()
}
pub fn key_sticker(id: Id<StickerMarker>) -> Bytes {
    format!("s:{id}").into_bytes().into()
}
pub fn key_stage_instance(id: Id<StageMarker>) -> Bytes {
    format!("si:{id}").into_bytes().into()
}
pub fn key_scheduled_event(id: Id<ScheduledEventMarker>) -> Bytes {
    format!("se:{id}").into_bytes().into()
}
pub fn key_integration(guild: Id<GuildMarker>, id: Id<IntegrationMarker>) -> Bytes {
    format!("ig:{guild}:{id}").into_bytes().into()
}
pub fn key_automod(id: Id<AutoModerationRuleMarker>) -> Bytes {
    format!("ar:{id}").into_bytes().into()
}
pub fn key_index(name: &str, id: Id<GenericMarker>) -> Bytes {
    format!("ix:{name}:{id}").into_bytes().into()
}
pub fn key_current_user() -> Bytes {
    Bytes::from_static(b"cu")
}

// ---------- serialization helpers ----------

pub fn encode<T: Serialize>(value: &T) -> Option<Bytes> {
    match serde_json::to_vec(value) {
        Ok(v) => Some(Bytes::from(v)),
        Err(e) => {
            warn!(error = %e, "cache: encode failed");
            None
        }
    }
}

pub fn decode<T: DeserializeOwned>(raw: &[u8]) -> Option<T> {
    match serde_json::from_slice(raw) {
        Ok(v) => Some(v),
        Err(e) => {
            debug!(error = %e, "cache: decode failed, dropping entry");
            None
        }
    }
}

// ---------- background writer ----------

/// How many times to re-attempt a failed batch before dropping it. The
/// backend commit is atomic, so a transient failure (e.g. a momentarily full
/// disk) leaves nothing applied and re-running the whole batch is safe.
const WRITER_MAX_ATTEMPTS: usize = 4;

pub fn spawn_writer(rx: flume::Receiver<WriteOp>, store: Arc<dyn KvStore>) {
    tokio::spawn(async move {
        loop {
            let Ok(first) = rx.recv_async().await else { return };
            let mut ops = Vec::with_capacity(64);
            push_kvops(&mut ops, first);
            while let Ok(op) = rx.try_recv() {
                push_kvops(&mut ops, op);
                if ops.len() >= 512 {
                    break;
                }
            }
            // Retry with exponential backoff. Dropping a batch silently is what
            // resurrects deleted data on the next boot (a lost `Delete*`), so
            // try hard before giving up rather than discarding on first error.
            let mut attempt = 1;
            loop {
                match store.batch(ops.clone()).await {
                    Ok(()) => break,
                    Err(e) if attempt >= WRITER_MAX_ATTEMPTS => {
                        warn!(error = %e, ops = ops.len(), attempts = attempt,
                              "cache writer batch failed — giving up and dropping");
                        break;
                    }
                    Err(e) => {
                        let backoff = std::time::Duration::from_millis(50 * (1 << attempt));
                        warn!(error = %e, attempt, backoff_ms = backoff.as_millis() as u64,
                              "cache writer batch failed — retrying");
                        tokio::time::sleep(backoff).await;
                        attempt += 1;
                    }
                }
            }
        }
    });
}

fn push_kvops(batch: &mut Vec<KvOp>, op: WriteOp) {
    match op {
        WriteOp::PutChannel(id, v) => put(batch, key_channel(id), v),
        WriteOp::DeleteChannel(id) => remove(batch, key_channel(id)),
        WriteOp::PutUser(id, v) => put(batch, key_user(id), v),
        WriteOp::DeleteUser(id) => remove(batch, key_user(id)),
        WriteOp::PutMessage(id, v) => put(batch, key_message(id), v),
        WriteOp::DeleteMessage(id) => remove(batch, key_message(id)),
        WriteOp::PutChannelMessages(id, v) => put(batch, key_channel_messages(id), v),
        WriteOp::DeleteChannelMessages(id) => remove(batch, key_channel_messages(id)),
        WriteOp::PutGuild(id, v) => put(batch, key_guild(id), v),
        WriteOp::DeleteGuild(id) => remove(batch, key_guild(id)),
        WriteOp::PutMember(g, u, v) => put(batch, key_member(g, u), v),
        WriteOp::DeleteMember(g, u) => remove(batch, key_member(g, u)),
        WriteOp::PutRole(id, v) => put(batch, key_role(id), v),
        WriteOp::DeleteRole(id) => remove(batch, key_role(id)),
        WriteOp::PutEmoji(id, v) => put(batch, key_emoji(id), v),
        WriteOp::DeleteEmoji(id) => remove(batch, key_emoji(id)),
        WriteOp::PutSticker(id, v) => put(batch, key_sticker(id), v),
        WriteOp::DeleteSticker(id) => remove(batch, key_sticker(id)),
        WriteOp::PutStageInstance(id, v) => put(batch, key_stage_instance(id), v),
        WriteOp::DeleteStageInstance(id) => remove(batch, key_stage_instance(id)),
        WriteOp::PutScheduledEvent(id, v) => put(batch, key_scheduled_event(id), v),
        WriteOp::DeleteScheduledEvent(id) => remove(batch, key_scheduled_event(id)),
        WriteOp::PutIntegration(g, i, v) => put(batch, key_integration(g, i), v),
        WriteOp::DeleteIntegration(g, i) => remove(batch, key_integration(g, i)),
        WriteOp::PutAutoModRule(id, v) => put(batch, key_automod(id), v),
        WriteOp::DeleteAutoModRule(id) => remove(batch, key_automod(id)),
        WriteOp::PutIndex(name, id, v) => put(batch, key_index(name, id), v),
        WriteOp::DeleteIndex(name, id) => remove(batch, key_index(name, id)),
        WriteOp::PutCurrentUser(v) => put(batch, key_current_user(), v),
    }
}

fn put(batch: &mut Vec<KvOp>, key: Bytes, value: Bytes) {
    batch.push(KvOp::Insert {
        partition: PARTITION.into(),
        key,
        value,
    });
}

fn remove(batch: &mut Vec<KvOp>, key: Bytes) {
    batch.push(KvOp::Remove { partition: PARTITION.into(), key });
}

// ---------- prefix iteration for startup load ----------

pub async fn scan_prefix(
    store: &Arc<dyn KvStore>,
    prefix: &'static str,
) -> Vec<(Bytes, Bytes)> {
    let prefix_bytes = Bytes::from(prefix.as_bytes().to_vec());
    match store.list_prefix(PARTITION, prefix_bytes).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, prefix, "cache: prefix scan failed");
            Vec::new()
        }
    }
}
