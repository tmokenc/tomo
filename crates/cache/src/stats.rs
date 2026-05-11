//! Counters over every resource the cache holds. Mirror of
//! `twilight_cache_inmemory::InMemoryCacheStats` — entry point is
//! [`PersistentCache::stats`].

use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker};

use crate::PersistentCache;

/// Thin handle that surfaces resource counts from the cache. Cheap to create
/// — just borrows the cache.
#[derive(Clone, Debug)]
pub struct PersistentCacheStats<'a>(&'a PersistentCache);

impl<'a> PersistentCacheStats<'a> {
    pub(crate) const fn new(cache: &'a PersistentCache) -> Self {
        Self(cache)
    }

    /// Immutable reference to the underlying cache.
    pub const fn cache_ref(&'a self) -> &'a PersistentCache {
        self.0
    }

    /// Consume the handle, returning the borrowed cache reference.
    pub const fn into_cache(self) -> &'a PersistentCache {
        self.0
    }

    pub fn channels(&self) -> usize {
        self.0.channels.len()
    }

    /// Number of messages currently cached for `channel_id`. Returns `None`
    /// if the channel has never been seen.
    pub fn channel_messages(&self, channel_id: Id<ChannelMarker>) -> Option<usize> {
        self.0.channel_messages.get(&channel_id).map(|r| r.len())
    }

    pub fn emojis(&self) -> usize {
        self.0.emojis.len()
    }

    pub fn guilds(&self) -> usize {
        self.0.guilds.len()
    }

    pub fn guild_channels(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_channels.get(&guild_id).map(|r| r.len())
    }

    pub fn guild_emojis(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_emojis.get(&guild_id).map(|r| r.len())
    }

    pub fn guild_members(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_members.get(&guild_id).map(|r| r.len())
    }

    pub fn guild_presences(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_presences.get(&guild_id).map(|r| r.len())
    }

    pub fn guild_roles(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_roles.get(&guild_id).map(|r| r.len())
    }

    pub fn guild_voice_states(&self, guild_id: Id<GuildMarker>) -> Option<usize> {
        self.0.guild_voice_states.get(&guild_id).map(|r| r.len())
    }

    pub fn integrations(&self) -> usize {
        self.0.integrations.len()
    }

    pub fn members(&self) -> usize {
        self.0.members.len()
    }

    pub fn messages(&self) -> usize {
        self.0.messages.len()
    }

    pub fn roles(&self) -> usize {
        self.0.roles.len()
    }

    pub fn stage_instances(&self) -> usize {
        self.0.stage_instances.len()
    }

    pub fn stickers(&self) -> usize {
        self.0.stickers.len()
    }

    pub fn users(&self) -> usize {
        self.0.users.len()
    }

    pub fn voice_states(&self) -> usize {
        self.0.voice_states.len()
    }
}
