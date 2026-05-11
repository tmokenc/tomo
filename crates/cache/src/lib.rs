//! Persistent Discord cache — API-compatible with `twilight-cache-inmemory`,
//! backed by [`tomo_db::KvStore`].
//!
//! Hot state lives in in-memory concurrent maps so lookups stay sync. Every
//! mutation through [`PersistentCache::update`] is mirrored onto a flume
//! channel; a single background task drains it and batches writes through the
//! KvStore. On startup [`PersistentCache::builder().build()`] reloads every
//! persisted resource into memory.
//!
//! # Drop-in usage
//!
//! ```ignore
//! let cache = PersistentCache::builder(db.clone())
//!     .resource_types(ResourceType::CHANNEL | ResourceType::MESSAGE | …)
//!     .message_cache_size(200)
//!     .build()
//!     .await?;
//! let cache = Arc::new(cache);
//!
//! // Same surface as `DefaultInMemoryCache`:
//! cache.update(&event);
//! if let Some(c) = cache.channel(channel_id) {
//!     if c.nsfw.unwrap_or(false) { /* … */ }
//! }
//! ```

pub mod builder;
pub mod cache;
pub mod iter;
pub mod persist;
pub mod reference;
pub mod resource;
pub mod stats;
pub mod update;

#[cfg(feature = "permission-calculator")]
pub mod permission;

pub use builder::PersistentCacheBuilder;
pub use cache::{GuildIntegrationKey, GuildResource, GuildUserKey, PersistentCache};
pub use iter::{IterReference, PersistentCacheIter, ResourceIter};
pub use reference::Reference;
pub use resource::ResourceType;
pub use stats::PersistentCacheStats;

#[cfg(feature = "permission-calculator")]
pub use permission::{
    ChannelError, ChannelErrorType, MEMBER_COMMUNICATION_DISABLED_ALLOWLIST,
    PersistentCachePermissions, RootError, RootErrorType,
};

/// Partition (column family / fjall keyspace) used for every cache write.
pub const PARTITION: &str = "cache";
