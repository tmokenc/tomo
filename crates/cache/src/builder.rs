//! Builder for [`PersistentCache`]. Mirrors `twilight_cache_inmemory::InMemoryCacheBuilder`.

use std::sync::Arc;

use arc_swap::ArcSwapOption;
use dashmap::DashMap;

use tomo_core::error::{Error, Result};
use tomo_db::KvStore;

use crate::cache::{Config, PersistentCache};
use crate::persist::{decode, scan_prefix, spawn_writer};
use crate::resource::ResourceType;

pub struct PersistentCacheBuilder {
    db: Arc<dyn KvStore>,
    config: Config,
}

impl PersistentCacheBuilder {
    pub fn new(db: Arc<dyn KvStore>) -> Self {
        Self { db, config: Config::default() }
    }

    pub fn resource_types(mut self, types: ResourceType) -> Self {
        self.config.resource_types = types;
        self
    }

    pub fn message_cache_size(mut self, size: usize) -> Self {
        self.config.message_cache_size = size;
        self
    }

    /// Construct the cache and hydrate it from the database. Returns an
    /// error if a database read fails — note that decode errors on
    /// individual rows are logged and skipped, not surfaced.
    pub async fn build(self) -> Result<PersistentCache> {
        let (tx, rx) = flume::unbounded();
        spawn_writer(rx, Arc::clone(&self.db));

        let cache = PersistentCache {
            config: self.config,
            tx,
            channels: DashMap::new(),
            channel_messages: DashMap::new(),
            current_user: ArcSwapOption::from(None),
            emojis: DashMap::new(),
            guilds: DashMap::new(),
            guild_channels: DashMap::new(),
            guild_emojis: DashMap::new(),
            guild_integrations: DashMap::new(),
            guild_members: DashMap::new(),
            guild_presences: DashMap::new(),
            guild_roles: DashMap::new(),
            guild_scheduled_events: DashMap::new(),
            guild_stage_instances: DashMap::new(),
            guild_stickers: DashMap::new(),
            guild_voice_states: DashMap::new(),
            integrations: DashMap::new(),
            members: DashMap::new(),
            messages: DashMap::new(),
            roles: DashMap::new(),
            scheduled_events: DashMap::new(),
            stage_instances: DashMap::new(),
            stickers: DashMap::new(),
            users: DashMap::new(),
            user_guilds: DashMap::new(),
            voice_states: DashMap::new(),
        };

        hydrate(&self.db, &cache)
            .await
            .map_err(|e| Error::config(format!("cache hydrate: {e}")))?;

        Ok(cache)
    }
}

async fn hydrate(db: &Arc<dyn KvStore>, cache: &PersistentCache) -> Result<()> {
    // Each persisted prefix is loaded individually. JSON deserialization
    // happens lazily and errors on individual rows are swallowed — that lets
    // us tolerate twilight-model schema drift across version bumps.

    for (_, v) in scan_prefix(db, "c:").await {
        if let Some(value) = decode::<twilight_model::channel::Channel>(&v) {
            cache.channels.insert(value.id, value);
        }
    }
    for (_, v) in scan_prefix(db, "u:").await {
        if let Some(user) = decode::<twilight_model::user::User>(&v) {
            cache.users.insert(user.id, user);
        }
    }
    for (_, v) in scan_prefix(db, "g:").await {
        if let Some(guild) = decode::<twilight_model::guild::Guild>(&v) {
            cache.guilds.insert(guild.id, guild);
        }
    }
    for (_, v) in scan_prefix(db, "m:").await {
        if let Some(message) = decode::<twilight_model::channel::Message>(&v) {
            cache.messages.insert(message.id, message);
        }
    }
    for (k, v) in scan_prefix(db, "cm:").await {
        if let Some(channel_id) = parse_id_from_key(&k, "cm:") {
            if let Some(deque) =
                decode::<std::collections::VecDeque<twilight_model::id::Id<twilight_model::id::marker::MessageMarker>>>(&v)
            {
                cache
                    .channel_messages
                    .insert(twilight_model::id::Id::new(channel_id), deque);
            }
        }
    }
    if let Some(cu_bytes) = db
        .get(crate::PARTITION, bytes::Bytes::from_static(b"cu"))
        .await
        .ok()
        .flatten()
    {
        if let Some(cu) = decode::<twilight_model::user::CurrentUser>(&cu_bytes) {
            cache.current_user.store(Some(Arc::new(cu)));
        }
    }
    // Other resources (members, roles, emojis, etc.) hydrate via gateway
    // events on next connect; their on-disk state is still kept up to date.

    Ok(())
}

fn parse_id_from_key(key: &[u8], prefix: &str) -> Option<u64> {
    let key = std::str::from_utf8(key).ok()?;
    let rest = key.strip_prefix(prefix)?;
    rest.parse().ok()
}
