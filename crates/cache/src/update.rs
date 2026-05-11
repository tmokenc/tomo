//! Gateway-event dispatcher for [`PersistentCache`]. Mirrors the
//! per-event handler split that lives under `twilight_cache_inmemory::event/`.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use twilight_gateway::Event;
use twilight_model::channel::Channel;
use twilight_model::id::Id;
use twilight_model::id::marker::{
    ChannelMarker, GuildMarker, MessageMarker, RoleMarker, UserMarker,
};

use crate::cache::{GuildResource, PersistentCache};
use crate::persist::{encode, WriteOp};
use crate::resource::ResourceType;

pub fn dispatch(cache: &PersistentCache, event: &Event) {
    match event {
        Event::Ready(r) => on_ready(cache, r),
        Event::ChannelCreate(c) => on_channel_upsert(cache, &c.0),
        Event::ChannelUpdate(c) => on_channel_upsert(cache, &c.0),
        Event::ChannelDelete(c) => on_channel_delete(cache, &c.0),
        Event::ChannelPinsUpdate(_) => {}
        Event::ThreadCreate(t) => on_channel_upsert(cache, &t.0),
        Event::ThreadUpdate(t) => on_channel_upsert(cache, &t.0),
        Event::ThreadDelete(t) => on_thread_delete(cache, t.id, Some(t.guild_id)),
        Event::GuildCreate(g) => on_guild_create(cache, g),
        Event::GuildUpdate(g) => on_guild_update(cache, &g.0),
        Event::GuildDelete(g) => on_guild_delete(cache, g.id, g.unavailable.is_some()),
        Event::MemberAdd(m) => on_member_add(cache, m),
        Event::MemberUpdate(m) => on_member_update(cache, m),
        Event::MemberRemove(m) => on_member_remove(cache, m.guild_id, m.user.id),
        Event::MemberChunk(chunk) => on_member_chunk(cache, chunk.guild_id, &chunk.members),
        Event::RoleCreate(r) => on_role_upsert(cache, r.guild_id, r.role.clone()),
        Event::RoleUpdate(r) => on_role_upsert(cache, r.guild_id, r.role.clone()),
        Event::RoleDelete(r) => on_role_delete(cache, r.guild_id, r.role_id),
        Event::MessageCreate(m) => on_message_create(cache, &m.0),
        Event::MessageUpdate(m) => on_message_update(cache, m),
        Event::MessageDelete(m) => on_message_delete(cache, m.channel_id, m.id),
        Event::MessageDeleteBulk(b) => {
            for id in &b.ids {
                on_message_delete(cache, b.channel_id, *id);
            }
        }
        Event::UserUpdate(u) => on_user_update_current(cache, &u.0),
        // VoiceStateUpdate and PresenceUpdate are tracked in memory only —
        // they churn too much to benefit from persistence.
        Event::VoiceStateUpdate(v) => on_voice_state(cache, &v.0),
        // The remaining ~50 event variants either don't touch cache state
        // or aren't yet wired. Falling through is consistent with twilight's
        // own conservative defaults — listing them noisily would crowd the
        // dispatcher without changing semantics.
        _ => {}
    }
}

fn want(cache: &PersistentCache, what: ResourceType) -> bool {
    cache.config.resource_types.contains(what)
}

// ---------------- Ready ----------------

fn on_ready(cache: &PersistentCache, ready: &twilight_model::gateway::payload::incoming::Ready) {
    if want(cache, ResourceType::USER_CURRENT) {
        cache.current_user.store(Some(Arc::new(ready.user.clone())));
        if let Some(v) = encode(&ready.user) {
            cache.enqueue(WriteOp::PutCurrentUser(v));
        }
    }
}

// ---------------- Channels ----------------

fn on_channel_upsert(cache: &PersistentCache, channel: &Channel) {
    if !want(cache, ResourceType::CHANNEL) {
        return;
    }
    let id = channel.id;
    if let Some(guild_id) = channel.guild_id {
        cache
            .guild_channels
            .entry(guild_id)
            .or_default()
            .insert(id);
    }
    cache.channels.insert(id, channel.clone());
    if let Some(v) = encode(channel) {
        cache.enqueue(WriteOp::PutChannel(id, v));
    }
}

fn on_channel_delete(cache: &PersistentCache, channel: &Channel) {
    let id = channel.id;
    cache.channels.remove(&id);
    if let Some(guild_id) = channel.guild_id {
        if let Some(mut entry) = cache.guild_channels.get_mut(&guild_id) {
            entry.remove(&id);
        }
    }
    cache.channel_messages.remove(&id);
    cache.enqueue(WriteOp::DeleteChannel(id));
    cache.enqueue(WriteOp::DeleteChannelMessages(id));
}

fn on_thread_delete(
    cache: &PersistentCache,
    id: Id<ChannelMarker>,
    guild_id: Option<Id<GuildMarker>>,
) {
    cache.channels.remove(&id);
    if let Some(g) = guild_id {
        if let Some(mut entry) = cache.guild_channels.get_mut(&g) {
            entry.remove(&id);
        }
    }
    cache.channel_messages.remove(&id);
    cache.enqueue(WriteOp::DeleteChannel(id));
}

// ---------------- Guilds ----------------

fn on_guild_create(
    cache: &PersistentCache,
    payload: &twilight_model::gateway::payload::incoming::GuildCreate,
) {
    use twilight_model::gateway::payload::incoming::GuildCreate;
    let guild = match payload {
        GuildCreate::Available(g) => g,
        // Unavailable guilds during reconnect — nothing to cache.
        GuildCreate::Unavailable(_) => return,
    };

    if want(cache, ResourceType::GUILD) {
        cache.guilds.insert(guild.id, guild.clone());
        if let Some(v) = encode(guild) {
            cache.enqueue(WriteOp::PutGuild(guild.id, v));
        }
    }

    if want(cache, ResourceType::CHANNEL) {
        let mut channels = HashSet::new();
        for c in &guild.channels {
            let mut c2 = c.clone();
            c2.guild_id = Some(guild.id);
            cache.channels.insert(c2.id, c2.clone());
            if let Some(v) = encode(&c2) {
                cache.enqueue(WriteOp::PutChannel(c2.id, v));
            }
            channels.insert(c.id);
        }
        for t in &guild.threads {
            let mut t2 = t.clone();
            t2.guild_id = Some(guild.id);
            cache.channels.insert(t2.id, t2.clone());
            if let Some(v) = encode(&t2) {
                cache.enqueue(WriteOp::PutChannel(t2.id, v));
            }
            channels.insert(t.id);
        }
        cache.guild_channels.insert(guild.id, channels);
    }

    if want(cache, ResourceType::ROLE) {
        let mut role_ids = HashSet::new();
        for role in &guild.roles {
            cache.roles.insert(
                role.id,
                GuildResource { guild_id: guild.id, value: role.clone() },
            );
            if let Some(v) = encode(role) {
                cache.enqueue(WriteOp::PutRole(role.id, v));
            }
            role_ids.insert(role.id);
        }
        cache.guild_roles.insert(guild.id, role_ids);
    }

    if want(cache, ResourceType::MEMBER) {
        let mut member_ids = HashSet::new();
        for m in &guild.members {
            cache
                .members
                .insert((guild.id, m.user.id), m.clone());
            if let Some(v) = encode(m) {
                cache.enqueue(WriteOp::PutMember(guild.id, m.user.id, v));
            }
            if want(cache, ResourceType::USER) {
                cache.users.insert(m.user.id, m.user.clone());
                if let Some(uv) = encode(&m.user) {
                    cache.enqueue(WriteOp::PutUser(m.user.id, uv));
                }
                cache
                    .user_guilds
                    .entry(m.user.id)
                    .or_default()
                    .insert(guild.id);
            }
            member_ids.insert(m.user.id);
        }
        cache.guild_members.insert(guild.id, member_ids);
    }

    if want(cache, ResourceType::EMOJI) {
        let mut ids = HashSet::new();
        for e in &guild.emojis {
            cache
                .emojis
                .insert(e.id, GuildResource { guild_id: guild.id, value: e.clone() });
            ids.insert(e.id);
        }
        cache.guild_emojis.insert(guild.id, ids);
    }

    if want(cache, ResourceType::STICKER) {
        let mut ids = HashSet::new();
        for s in &guild.stickers {
            cache
                .stickers
                .insert(s.id, GuildResource { guild_id: guild.id, value: s.clone() });
            ids.insert(s.id);
        }
        cache.guild_stickers.insert(guild.id, ids);
    }

    if want(cache, ResourceType::VOICE_STATE) {
        let mut ids = HashSet::new();
        for v in &guild.voice_states {
            if let Some(user_id) = v.user_id.into() {
                let _ = user_id;
            }
            cache
                .voice_states
                .insert((guild.id, v.user_id), v.clone());
            ids.insert(v.user_id);
        }
        cache.guild_voice_states.insert(guild.id, ids);
    }
}

fn on_guild_update(cache: &PersistentCache, guild: &twilight_model::guild::PartialGuild) {
    if !want(cache, ResourceType::GUILD) {
        return;
    }
    if let Some(mut existing) = cache.guilds.get_mut(&guild.id) {
        existing.name = guild.name.clone();
        existing.icon = guild.icon;
        existing.owner_id = guild.owner_id;
        existing.verification_level = guild.verification_level;
        existing.default_message_notifications = guild.default_message_notifications;
        existing.afk_channel_id = guild.afk_channel_id;
        existing.afk_timeout = guild.afk_timeout;
        existing.banner = guild.banner;
        existing.description = guild.description.clone();
        existing.discovery_splash = guild.discovery_splash;
        existing.explicit_content_filter = guild.explicit_content_filter;
        existing.mfa_level = guild.mfa_level;
        existing.nsfw_level = guild.nsfw_level;
        existing.preferred_locale = guild.preferred_locale.clone();
        existing.premium_tier = guild.premium_tier;
        existing.system_channel_id = guild.system_channel_id;
        existing.vanity_url_code = guild.vanity_url_code.clone();
        existing.widget_channel_id = guild.widget_channel_id;
        existing.widget_enabled = guild.widget_enabled;
        if let Some(v) = encode(existing.value()) {
            cache.enqueue(WriteOp::PutGuild(existing.id, v));
        }
    }
}

fn on_guild_delete(cache: &PersistentCache, id: Id<GuildMarker>, _unavailable: bool) {
    cache.guilds.remove(&id);
    cache.enqueue(WriteOp::DeleteGuild(id));

    if let Some((_, channel_ids)) = cache.guild_channels.remove(&id) {
        for c in channel_ids {
            cache.channels.remove(&c);
            cache.channel_messages.remove(&c);
            cache.enqueue(WriteOp::DeleteChannel(c));
            cache.enqueue(WriteOp::DeleteChannelMessages(c));
        }
    }
    if let Some((_, role_ids)) = cache.guild_roles.remove(&id) {
        for r in role_ids {
            cache.roles.remove(&r);
            cache.enqueue(WriteOp::DeleteRole(r));
        }
    }
    if let Some((_, user_ids)) = cache.guild_members.remove(&id) {
        for u in user_ids {
            cache.members.remove(&(id, u));
            cache.enqueue(WriteOp::DeleteMember(id, u));
            if let Some(mut entry) = cache.user_guilds.get_mut(&u) {
                entry.remove(&id);
            }
        }
    }
    let _ = cache.guild_emojis.remove(&id);
    let _ = cache.guild_stickers.remove(&id);
    let _ = cache.guild_voice_states.remove(&id);
    let _ = cache.guild_presences.remove(&id);
    let _ = cache.guild_integrations.remove(&id);
    let _ = cache.guild_scheduled_events.remove(&id);
    let _ = cache.guild_stage_instances.remove(&id);
}

// ---------------- Members ----------------

fn on_member_add(
    cache: &PersistentCache,
    payload: &twilight_model::gateway::payload::incoming::MemberAdd,
) {
    if !want(cache, ResourceType::MEMBER) {
        return;
    }
    let guild_id = payload.guild_id;
    let user_id = payload.member.user.id;
    cache
        .members
        .insert((guild_id, user_id), payload.member.clone());
    if let Some(v) = encode(&payload.member) {
        cache.enqueue(WriteOp::PutMember(guild_id, user_id, v));
    }
    if want(cache, ResourceType::USER) {
        cache.users.insert(user_id, payload.member.user.clone());
        if let Some(uv) = encode(&payload.member.user) {
            cache.enqueue(WriteOp::PutUser(user_id, uv));
        }
        cache
            .user_guilds
            .entry(user_id)
            .or_default()
            .insert(guild_id);
    }
    cache
        .guild_members
        .entry(guild_id)
        .or_default()
        .insert(user_id);
}

fn on_member_update(
    cache: &PersistentCache,
    payload: &twilight_model::gateway::payload::incoming::MemberUpdate,
) {
    if !want(cache, ResourceType::MEMBER) {
        return;
    }
    let key = (payload.guild_id, payload.user.id);
    if let Some(mut existing) = cache.members.get_mut(&key) {
        existing.user = payload.user.clone();
        existing.nick = payload.nick.clone();
        existing.roles = payload.roles.clone();
        existing.joined_at = payload.joined_at;
        existing.premium_since = payload.premium_since;
        existing.pending = payload.pending;
        existing.avatar = payload.avatar;
        existing.communication_disabled_until = payload.communication_disabled_until;
        if let Some(v) = encode(existing.value()) {
            cache.enqueue(WriteOp::PutMember(payload.guild_id, payload.user.id, v));
        }
    }
}

fn on_member_remove(
    cache: &PersistentCache,
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
) {
    cache.members.remove(&(guild_id, user_id));
    cache.enqueue(WriteOp::DeleteMember(guild_id, user_id));
    if let Some(mut entry) = cache.guild_members.get_mut(&guild_id) {
        entry.remove(&user_id);
    }
    if let Some(mut entry) = cache.user_guilds.get_mut(&user_id) {
        entry.remove(&guild_id);
    }
}

fn on_member_chunk(
    cache: &PersistentCache,
    guild_id: Id<GuildMarker>,
    members: &[twilight_model::guild::Member],
) {
    if !want(cache, ResourceType::MEMBER) {
        return;
    }
    let mut ids = cache.guild_members.entry(guild_id).or_default();
    for m in members {
        cache.members.insert((guild_id, m.user.id), m.clone());
        if let Some(v) = encode(m) {
            cache.enqueue(WriteOp::PutMember(guild_id, m.user.id, v));
        }
        if want(cache, ResourceType::USER) {
            cache.users.insert(m.user.id, m.user.clone());
            if let Some(uv) = encode(&m.user) {
                cache.enqueue(WriteOp::PutUser(m.user.id, uv));
            }
        }
        ids.insert(m.user.id);
    }
}

// ---------------- Roles ----------------

fn on_role_upsert(
    cache: &PersistentCache,
    guild_id: Id<GuildMarker>,
    role: twilight_model::guild::Role,
) {
    if !want(cache, ResourceType::ROLE) {
        return;
    }
    let id = role.id;
    cache.guild_roles.entry(guild_id).or_default().insert(id);
    if let Some(v) = encode(&role) {
        cache.enqueue(WriteOp::PutRole(id, v));
    }
    cache.roles.insert(id, GuildResource { guild_id, value: role });
}

fn on_role_delete(
    cache: &PersistentCache,
    guild_id: Id<GuildMarker>,
    role_id: Id<RoleMarker>,
) {
    cache.roles.remove(&role_id);
    cache.enqueue(WriteOp::DeleteRole(role_id));
    if let Some(mut entry) = cache.guild_roles.get_mut(&guild_id) {
        entry.remove(&role_id);
    }
}

// ---------------- Messages ----------------

fn on_message_create(
    cache: &PersistentCache,
    message: &twilight_model::channel::Message,
) {
    if !want(cache, ResourceType::MESSAGE) {
        return;
    }
    let channel_id = message.channel_id;
    let message_id = message.id;
    let cap = cache.config.message_cache_size;

    cache.messages.insert(message_id, message.clone());
    if let Some(v) = encode(message) {
        cache.enqueue(WriteOp::PutMessage(message_id, v));
    }

    let mut deque = cache.channel_messages.entry(channel_id).or_default();
    deque.push_back(message_id);
    while deque.len() > cap {
        if let Some(old) = deque.pop_front() {
            cache.messages.remove(&old);
            cache.enqueue(WriteOp::DeleteMessage(old));
        }
    }
    if let Some(v) = encode(&*deque) {
        cache.enqueue(WriteOp::PutChannelMessages(channel_id, v));
    }

    if want(cache, ResourceType::USER) {
        cache.users.insert(message.author.id, message.author.clone());
        if let Some(uv) = encode(&message.author) {
            cache.enqueue(WriteOp::PutUser(message.author.id, uv));
        }
    }
}

fn on_message_update(
    cache: &PersistentCache,
    update: &twilight_model::gateway::payload::incoming::MessageUpdate,
) {
    if !want(cache, ResourceType::MESSAGE) {
        return;
    }
    if let Some(mut existing) = cache.messages.get_mut(&update.id) {
        existing.content = update.content.clone();
        existing.mentions = update.mentions.clone();
        existing.attachments = update.attachments.clone();
        existing.embeds = update.embeds.clone();
        if let Some(edited_timestamp) = update.edited_timestamp {
            existing.edited_timestamp = Some(edited_timestamp);
        }
        if let Some(v) = encode(existing.value()) {
            cache.enqueue(WriteOp::PutMessage(update.id, v));
        }
    }
}

fn on_message_delete(
    cache: &PersistentCache,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
) {
    cache.messages.remove(&message_id);
    cache.enqueue(WriteOp::DeleteMessage(message_id));
    if let Some(mut deque) = cache.channel_messages.get_mut(&channel_id) {
        deque.retain(|id| *id != message_id);
        if let Some(v) = encode(&*deque) {
            cache.enqueue(WriteOp::PutChannelMessages(channel_id, v));
        }
    }
}

// ---------------- Users ----------------

fn on_user_update_current(cache: &PersistentCache, user: &twilight_model::user::CurrentUser) {
    if !want(cache, ResourceType::USER_CURRENT) {
        return;
    }
    cache.current_user.store(Some(Arc::new(user.clone())));
    if let Some(v) = encode(user) {
        cache.enqueue(WriteOp::PutCurrentUser(v));
    }
}

// ---------------- Voice ----------------

fn on_voice_state(cache: &PersistentCache, vs: &twilight_model::voice::VoiceState) {
    if !want(cache, ResourceType::VOICE_STATE) {
        return;
    }
    let Some(guild_id) = vs.guild_id else { return };
    let user_id = vs.user_id;
    if vs.channel_id.is_some() {
        cache.voice_states.insert((guild_id, user_id), vs.clone());
        cache
            .guild_voice_states
            .entry(guild_id)
            .or_default()
            .insert(user_id);
    } else {
        cache.voice_states.remove(&(guild_id, user_id));
        if let Some(mut entry) = cache.guild_voice_states.get_mut(&guild_id) {
            entry.remove(&user_id);
        }
    }
    // Voice state is intentionally transient — no persistence.
}

// Suppress an unused-variable warning until we wire some of the more
// rarely-touched event handlers. `VecDeque` is referenced through deque
// manipulation above.
#[allow(unused_imports)]
use VecDeque as _;
