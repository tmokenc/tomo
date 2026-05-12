use twilight_gateway::{EventTypeFlags, Intents};

/// Intents the bot subscribes to. Privileged intents (`MESSAGE_CONTENT`,
/// `GUILD_MEMBERS`) must also be enabled in the Discord developer portal.
pub fn intents() -> Intents {
    Intents::GUILDS
        | Intents::GUILD_MESSAGES
        | Intents::GUILD_MESSAGE_REACTIONS
        | Intents::DIRECT_MESSAGES
        | Intents::DIRECT_MESSAGE_REACTIONS
        | Intents::MESSAGE_CONTENT
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_VOICE_STATES
}

/// Events to actually deliver. We let twilight filter aggressively so the
/// handler never sees things we do not act on.
pub fn event_types() -> EventTypeFlags {
    EventTypeFlags::READY
        | EventTypeFlags::RESUMED
        | EventTypeFlags::MESSAGE_CREATE
        | EventTypeFlags::MESSAGE_UPDATE
        | EventTypeFlags::MESSAGE_DELETE
        | EventTypeFlags::INTERACTION_CREATE
        | EventTypeFlags::REACTION_ADD
        | EventTypeFlags::REACTION_REMOVE
        | EventTypeFlags::GUILD_CREATE
        | EventTypeFlags::GUILD_DELETE
        | EventTypeFlags::GUILD_UPDATE
        | EventTypeFlags::CHANNEL_CREATE
        | EventTypeFlags::CHANNEL_UPDATE
        | EventTypeFlags::CHANNEL_DELETE
        | EventTypeFlags::MEMBER_ADD
        | EventTypeFlags::MEMBER_REMOVE
        | EventTypeFlags::MEMBER_UPDATE
        // Drives the Myon time tracker — joining/leaving voice flips the
        // "in_voice" state that keeps a user marked as awake even when
        // they aren't typing.
        | EventTypeFlags::VOICE_STATE_UPDATE
}
