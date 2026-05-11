//! Iterators over the cache's resources. Mirror of
//! `twilight_cache_inmemory::iter` — entry point is [`PersistentCache::iter`].

use std::collections::VecDeque;
use std::hash::Hash;
use std::ops::Deref;

use dashmap::iter::Iter;
use dashmap::mapref::multiple::RefMulti;

use twilight_model::channel::message::Sticker;
use twilight_model::channel::{Channel, Message, StageInstance};
use twilight_model::guild::scheduled_event::GuildScheduledEvent;
use twilight_model::guild::{Emoji, Guild, GuildIntegration, Member, Role};
use twilight_model::id::Id;
use twilight_model::id::marker::{
    ChannelMarker, EmojiMarker, GuildMarker, MessageMarker, RoleMarker, ScheduledEventMarker,
    StageMarker, StickerMarker, UserMarker,
};
use twilight_model::user::User;
use twilight_model::voice::VoiceState;

use crate::PersistentCache;
use crate::cache::{GuildIntegrationKey, GuildResource, GuildUserKey};

/// Reference yielded by [`ResourceIter`]. Derefs to the value; key/value
/// accessors mirror twilight's `IterReference`.
pub struct IterReference<'a, K, V> {
    inner: RefMulti<'a, K, V>,
}

impl<'a, K, V> IterReference<'a, K, V> {
    const fn new(inner: RefMulti<'a, K, V>) -> Self {
        Self { inner }
    }
}

impl<K: Eq + Hash, V> IterReference<'_, K, V> {
    pub fn key(&self) -> &K {
        self.inner.key()
    }

    pub fn value(&self) -> &V {
        self.inner.value()
    }
}

impl<K: Eq + Hash, V> Deref for IterReference<'_, K, V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

/// Handle exposing one iterator per resource kind. Mirror of
/// `InMemoryCacheIter` — iteration order is arbitrary.
#[derive(Debug)]
pub struct PersistentCacheIter<'a>(&'a PersistentCache);

impl<'a> PersistentCacheIter<'a> {
    pub(crate) const fn new(cache: &'a PersistentCache) -> Self {
        Self(cache)
    }

    /// Immutable reference to the underlying cache.
    pub const fn cache_ref(&'a self) -> &'a PersistentCache {
        self.0
    }

    pub fn channels(&self) -> ResourceIter<'a, Id<ChannelMarker>, Channel> {
        ResourceIter::new(self.0.channels.iter())
    }

    pub fn channel_messages(
        &self,
    ) -> ResourceIter<'a, Id<ChannelMarker>, VecDeque<Id<MessageMarker>>> {
        ResourceIter::new(self.0.channel_messages.iter())
    }

    pub fn emojis(&self) -> ResourceIter<'a, Id<EmojiMarker>, GuildResource<Emoji>> {
        ResourceIter::new(self.0.emojis.iter())
    }

    pub fn guilds(&self) -> ResourceIter<'a, Id<GuildMarker>, Guild> {
        ResourceIter::new(self.0.guilds.iter())
    }

    pub fn integrations(
        &self,
    ) -> ResourceIter<'a, GuildIntegrationKey, GuildResource<GuildIntegration>> {
        ResourceIter::new(self.0.integrations.iter())
    }

    pub fn members(&self) -> ResourceIter<'a, GuildUserKey, Member> {
        ResourceIter::new(self.0.members.iter())
    }

    pub fn messages(&self) -> ResourceIter<'a, Id<MessageMarker>, Message> {
        ResourceIter::new(self.0.messages.iter())
    }

    pub fn roles(&self) -> ResourceIter<'a, Id<RoleMarker>, GuildResource<Role>> {
        ResourceIter::new(self.0.roles.iter())
    }

    pub fn scheduled_events(
        &self,
    ) -> ResourceIter<'a, Id<ScheduledEventMarker>, GuildResource<GuildScheduledEvent>> {
        ResourceIter::new(self.0.scheduled_events.iter())
    }

    pub fn stage_instances(
        &self,
    ) -> ResourceIter<'a, Id<StageMarker>, GuildResource<StageInstance>> {
        ResourceIter::new(self.0.stage_instances.iter())
    }

    pub fn stickers(&self) -> ResourceIter<'a, Id<StickerMarker>, GuildResource<Sticker>> {
        ResourceIter::new(self.0.stickers.iter())
    }

    pub fn users(&self) -> ResourceIter<'a, Id<UserMarker>, User> {
        ResourceIter::new(self.0.users.iter())
    }

    pub fn voice_states(&self) -> ResourceIter<'a, GuildUserKey, VoiceState> {
        ResourceIter::new(self.0.voice_states.iter())
    }
}

/// `Iterator` adapter over a DashMap's underlying iterator that yields
/// [`IterReference`]s.
pub struct ResourceIter<'a, K, V> {
    iter: Iter<'a, K, V>,
}

impl<'a, K, V> ResourceIter<'a, K, V> {
    pub(crate) const fn new(iter: Iter<'a, K, V>) -> Self {
        Self { iter }
    }
}

impl<'a, K: Eq + Hash, V> Iterator for ResourceIter<'a, K, V> {
    type Item = IterReference<'a, K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(IterReference::new)
    }
}
