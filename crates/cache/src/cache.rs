//! [`PersistentCache`] — the main cache struct. Mirrors twilight's
//! `InMemoryCache` field-for-field, but every mutation is mirrored through
//! a flume channel to a persistent backing store.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use dashmap::DashMap;
use twilight_model::channel::message::Sticker;
use twilight_model::channel::{Channel, Message, StageInstance};
use twilight_model::guild::scheduled_event::GuildScheduledEvent;
use twilight_model::guild::{Emoji, Guild, GuildIntegration, Member, Role};
use twilight_model::id::Id;
use twilight_model::id::marker::{
    ChannelMarker, EmojiMarker, GuildMarker, IntegrationMarker, MessageMarker, RoleMarker,
    ScheduledEventMarker, StageMarker, StickerMarker, UserMarker,
};
use twilight_model::user::{CurrentUser, User};
use twilight_model::voice::VoiceState;

use crate::persist::WriteOp;
use crate::reference::Reference;
use crate::resource::ResourceType;

/// Configuration for [`PersistentCache`].
#[derive(Debug, Clone)]
pub struct Config {
    pub resource_types: ResourceType,
    pub message_cache_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { resource_types: ResourceType::all(), message_cache_size: 100 }
    }
}

/// Resource paired with the guild it belongs to. Mirrors
/// `twilight_cache_inmemory::GuildResource` so callers can pattern-match the
/// same way.
#[derive(Debug, Clone)]
pub struct GuildResource<T> {
    pub guild_id: Id<GuildMarker>,
    pub value: T,
}

impl<T> GuildResource<T> {
    pub const fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    pub const fn resource(&self) -> &T {
        &self.value
    }
}

impl<T> std::ops::Deref for GuildResource<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

/// `twilight-cache-inmemory::InMemoryCache` replacement with a persistent
/// backing store. All lookup methods are sync; mutations are queued for
/// background persistence.
pub struct PersistentCache {
    pub(crate) config: Config,
    pub(crate) tx: flume::Sender<WriteOp>,

    pub(crate) channels: DashMap<Id<ChannelMarker>, Channel>,
    pub(crate) channel_messages: DashMap<Id<ChannelMarker>, VecDeque<Id<MessageMarker>>>,
    pub(crate) current_user: ArcSwapOption<CurrentUser>,
    pub(crate) emojis: DashMap<Id<EmojiMarker>, GuildResource<Emoji>>,
    pub(crate) guilds: DashMap<Id<GuildMarker>, Guild>,
    pub(crate) guild_channels: DashMap<Id<GuildMarker>, HashSet<Id<ChannelMarker>>>,
    pub(crate) guild_emojis: DashMap<Id<GuildMarker>, HashSet<Id<EmojiMarker>>>,
    pub(crate) guild_integrations: DashMap<Id<GuildMarker>, HashSet<Id<IntegrationMarker>>>,
    pub(crate) guild_members: DashMap<Id<GuildMarker>, HashSet<Id<UserMarker>>>,
    pub(crate) guild_presences: DashMap<Id<GuildMarker>, HashSet<Id<UserMarker>>>,
    pub(crate) guild_roles: DashMap<Id<GuildMarker>, HashSet<Id<RoleMarker>>>,
    pub(crate) guild_scheduled_events:
        DashMap<Id<GuildMarker>, HashSet<Id<ScheduledEventMarker>>>,
    pub(crate) guild_stage_instances: DashMap<Id<GuildMarker>, HashSet<Id<StageMarker>>>,
    pub(crate) guild_stickers: DashMap<Id<GuildMarker>, HashSet<Id<StickerMarker>>>,
    pub(crate) guild_voice_states: DashMap<Id<GuildMarker>, HashSet<Id<UserMarker>>>,
    pub(crate) integrations:
        DashMap<(Id<GuildMarker>, Id<IntegrationMarker>), GuildResource<GuildIntegration>>,
    pub(crate) members: DashMap<(Id<GuildMarker>, Id<UserMarker>), Member>,
    pub(crate) messages: DashMap<Id<MessageMarker>, Message>,
    pub(crate) roles: DashMap<Id<RoleMarker>, GuildResource<Role>>,
    pub(crate) scheduled_events:
        DashMap<Id<ScheduledEventMarker>, GuildResource<GuildScheduledEvent>>,
    pub(crate) stage_instances: DashMap<Id<StageMarker>, GuildResource<StageInstance>>,
    pub(crate) stickers: DashMap<Id<StickerMarker>, GuildResource<Sticker>>,
    pub(crate) users: DashMap<Id<UserMarker>, User>,
    pub(crate) user_guilds: DashMap<Id<UserMarker>, HashSet<Id<GuildMarker>>>,
    pub(crate) voice_states: DashMap<(Id<GuildMarker>, Id<UserMarker>), VoiceState>,
}

/// Composite key for `(guild, integration)` lookups.
pub type GuildIntegrationKey = (Id<GuildMarker>, Id<IntegrationMarker>);
/// Composite key for `(guild, user)` lookups — members and voice states.
pub type GuildUserKey = (Id<GuildMarker>, Id<UserMarker>);

impl std::fmt::Debug for PersistentCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentCache").finish_non_exhaustive()
    }
}

impl PersistentCache {
    /// Start building a cache.
    pub fn builder(db: Arc<dyn tomo_db::KvStore>) -> crate::builder::PersistentCacheBuilder {
        crate::builder::PersistentCacheBuilder::new(db)
    }

    /// Returns the cache's effective config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Process a gateway event — mirror of `InMemoryCache::update`.
    pub fn update(&self, event: &twilight_gateway::Event) {
        crate::update::dispatch(self, event);
    }

    /// Insert (or overwrite) a member fetched out-of-band — typically when
    /// the bot just made an HTTP `guild_member` call to hydrate state that
    /// the gateway hadn't delivered yet. Equivalent to the cache update a
    /// `MEMBER_ADD` event would have performed: stores the member, links
    /// it into `guild_members`, and back-fills the embedded `User` /
    /// `guild_members` indices.
    ///
    /// Useful for permission-gated dispatch: when the permission calculator
    /// returns `MemberUnavailable`, the caller can fetch + insert here and
    /// retry without paying the round-trip again next time.
    pub fn upsert_member(
        &self,
        guild_id: twilight_model::id::Id<twilight_model::id::marker::GuildMarker>,
        member: twilight_model::guild::Member,
    ) {
        use crate::resource::ResourceType;
        if self.config.resource_types.contains(ResourceType::USER) {
            self.users.insert(member.user.id, member.user.clone());
        }
        self.guild_members
            .entry(guild_id)
            .or_default()
            .insert(member.user.id);
        self.members.insert((guild_id, member.user.id), member);
    }

    /// Insert (or overwrite) a single role on a guild. Same out-of-band
    /// helper as [`upsert_member`] but for roles — used when the
    /// permission calculator hits `RoleUnavailable` for a role that's
    /// only just been created or that the cache missed.
    pub fn upsert_role(
        &self,
        guild_id: twilight_model::id::Id<twilight_model::id::marker::GuildMarker>,
        role: twilight_model::guild::Role,
    ) {
        self.guild_roles
            .entry(guild_id)
            .or_default()
            .insert(role.id);
        self.roles.insert(
            role.id,
            crate::cache::GuildResource { guild_id, value: role },
        );
    }

    /// Counts of each resource type. Mirror of `InMemoryCache::stats`.
    pub fn stats(&self) -> crate::stats::PersistentCacheStats<'_> {
        crate::stats::PersistentCacheStats::new(self)
    }

    /// Iterators over each cached resource. Mirror of `InMemoryCache::iter`.
    pub fn iter(&self) -> crate::iter::PersistentCacheIter<'_> {
        crate::iter::PersistentCacheIter::new(self)
    }

    /// Permission calculator. Only available with the
    /// `permission-calculator` feature enabled.
    #[cfg(feature = "permission-calculator")]
    pub fn permissions(&self) -> crate::permission::PersistentCachePermissions<'_> {
        crate::permission::PersistentCachePermissions::new(self)
    }

    // ---------------- Lookups (mirror twilight) ----------------

    pub fn current_user(&self) -> Option<CurrentUser> {
        self.current_user.load_full().as_deref().cloned()
    }

    pub fn channel(
        &self,
        channel_id: Id<ChannelMarker>,
    ) -> Option<Reference<'_, Id<ChannelMarker>, Channel>> {
        self.channels.get(&channel_id).map(Reference::new)
    }

    pub fn channel_messages(
        &self,
        channel_id: Id<ChannelMarker>,
    ) -> Option<Reference<'_, Id<ChannelMarker>, VecDeque<Id<MessageMarker>>>> {
        self.channel_messages.get(&channel_id).map(Reference::new)
    }

    pub fn emoji(
        &self,
        emoji_id: Id<EmojiMarker>,
    ) -> Option<Reference<'_, Id<EmojiMarker>, GuildResource<Emoji>>> {
        self.emojis.get(&emoji_id).map(Reference::new)
    }

    pub fn guild(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, Guild>> {
        self.guilds.get(&guild_id).map(Reference::new)
    }

    pub fn guild_channels(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<ChannelMarker>>>> {
        self.guild_channels.get(&guild_id).map(Reference::new)
    }

    pub fn guild_emojis(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<EmojiMarker>>>> {
        self.guild_emojis.get(&guild_id).map(Reference::new)
    }

    pub fn guild_integrations(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<IntegrationMarker>>>> {
        self.guild_integrations.get(&guild_id).map(Reference::new)
    }

    pub fn guild_members(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<UserMarker>>>> {
        self.guild_members.get(&guild_id).map(Reference::new)
    }

    pub fn guild_presences(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<UserMarker>>>> {
        self.guild_presences.get(&guild_id).map(Reference::new)
    }

    pub fn guild_roles(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<RoleMarker>>>> {
        self.guild_roles.get(&guild_id).map(Reference::new)
    }

    pub fn scheduled_events(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<ScheduledEventMarker>>>> {
        self.guild_scheduled_events.get(&guild_id).map(Reference::new)
    }

    pub fn guild_stage_instances(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<StageMarker>>>> {
        self.guild_stage_instances.get(&guild_id).map(Reference::new)
    }

    pub fn guild_stickers(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<StickerMarker>>>> {
        self.guild_stickers.get(&guild_id).map(Reference::new)
    }

    pub fn guild_voice_states(
        &self,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, Id<GuildMarker>, HashSet<Id<UserMarker>>>> {
        self.guild_voice_states.get(&guild_id).map(Reference::new)
    }

    pub fn integration(
        &self,
        guild_id: Id<GuildMarker>,
        integration_id: Id<IntegrationMarker>,
    ) -> Option<Reference<'_, GuildIntegrationKey, GuildResource<GuildIntegration>>> {
        self.integrations.get(&(guild_id, integration_id)).map(Reference::new)
    }

    pub fn member(
        &self,
        guild_id: Id<GuildMarker>,
        user_id: Id<UserMarker>,
    ) -> Option<Reference<'_, GuildUserKey, Member>> {
        self.members.get(&(guild_id, user_id)).map(Reference::new)
    }

    pub fn message(
        &self,
        message_id: Id<MessageMarker>,
    ) -> Option<Reference<'_, Id<MessageMarker>, Message>> {
        self.messages.get(&message_id).map(Reference::new)
    }

    pub fn presence(
        &self,
        _guild_id: Id<GuildMarker>,
        _user_id: Id<UserMarker>,
    ) -> Option<()> {
        // Presence tracking is intentionally skipped — presences churn so
        // much that they aren't worth persisting, and nothing in the bot
        // reads them today.
        None
    }

    pub fn role(
        &self,
        role_id: Id<RoleMarker>,
    ) -> Option<Reference<'_, Id<RoleMarker>, GuildResource<Role>>> {
        self.roles.get(&role_id).map(Reference::new)
    }

    pub fn scheduled_event(
        &self,
        event_id: Id<ScheduledEventMarker>,
    ) -> Option<Reference<'_, Id<ScheduledEventMarker>, GuildResource<GuildScheduledEvent>>> {
        self.scheduled_events.get(&event_id).map(Reference::new)
    }

    pub fn stage_instance(
        &self,
        stage_id: Id<StageMarker>,
    ) -> Option<Reference<'_, Id<StageMarker>, GuildResource<StageInstance>>> {
        self.stage_instances.get(&stage_id).map(Reference::new)
    }

    pub fn sticker(
        &self,
        sticker_id: Id<StickerMarker>,
    ) -> Option<Reference<'_, Id<StickerMarker>, GuildResource<Sticker>>> {
        self.stickers.get(&sticker_id).map(Reference::new)
    }

    pub fn user(
        &self,
        user_id: Id<UserMarker>,
    ) -> Option<Reference<'_, Id<UserMarker>, User>> {
        self.users.get(&user_id).map(Reference::new)
    }

    pub fn user_guilds(
        &self,
        user_id: Id<UserMarker>,
    ) -> Option<Reference<'_, Id<UserMarker>, HashSet<Id<GuildMarker>>>> {
        self.user_guilds.get(&user_id).map(Reference::new)
    }

    pub fn voice_state(
        &self,
        user_id: Id<UserMarker>,
        guild_id: Id<GuildMarker>,
    ) -> Option<Reference<'_, GuildUserKey, VoiceState>> {
        self.voice_states.get(&(guild_id, user_id)).map(Reference::new)
    }

    /// Wipe every collection. Persistent writes for these clears are *not*
    /// emitted — call this only when you intend to discard local state and
    /// optionally `flush()` the writer.
    pub fn clear(&self) {
        self.channels.clear();
        self.channel_messages.clear();
        self.current_user.store(None);
        self.emojis.clear();
        self.guilds.clear();
        self.guild_channels.clear();
        self.guild_emojis.clear();
        self.guild_integrations.clear();
        self.guild_members.clear();
        self.guild_presences.clear();
        self.guild_roles.clear();
        self.guild_scheduled_events.clear();
        self.guild_stage_instances.clear();
        self.guild_stickers.clear();
        self.guild_voice_states.clear();
        self.integrations.clear();
        self.members.clear();
        self.messages.clear();
        self.roles.clear();
        self.scheduled_events.clear();
        self.stage_instances.clear();
        self.stickers.clear();
        self.users.clear();
        self.user_guilds.clear();
        self.voice_states.clear();
    }

    pub(crate) fn enqueue(&self, op: WriteOp) {
        // The writer is unbounded; send only fails if the receiver dropped
        // (i.e. the cache itself is being torn down) — silently ignore.
        let _ = self.tx.send(op);
    }
}
