use bitflags::bitflags;

bitflags! {
    /// Mirror of `twilight_cache_inmemory::ResourceType`. Bit values are kept
    /// identical so existing call sites swap over verbatim.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    pub struct ResourceType: u64 {
        const CHANNEL                = 1;
        const EMOJI                  = 1 << 1;
        const GUILD                  = 1 << 2;
        const MEMBER                 = 1 << 3;
        const MESSAGE                = 1 << 4;
        const PRESENCE               = 1 << 5;
        const REACTION               = 1 << 6;
        const ROLE                   = 1 << 7;
        const USER_CURRENT           = 1 << 8;
        const USER                   = 1 << 9;
        const VOICE_STATE            = 1 << 10;
        const STAGE_INSTANCE         = 1 << 11;
        const INTEGRATION            = 1 << 12;
        const STICKER                = 1 << 13;
        const GUILD_SCHEDULED_EVENT  = 1 << 14;
    }
}
