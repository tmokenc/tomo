//! Storage abstraction for Tomo.
//!
//! The bot's persistent state — stats counters, per-guild settings, cached
//! pieces of API responses — flows through the [`KvStore`] trait defined
//! here. A single async-friendly trait keeps call sites identical regardless
//! of the underlying engine, so we can swap fjall for SurrealDB (or anything
//! else) by adding a new backend module and updating one wire-up line in the
//! binary.
//!
//! Backends:
//! - [`fjall_backend::FjallStore`] (default, embedded LSM-tree KV store).
//!
//! ```ignore
//! use std::sync::Arc;
//! use tomo_db::{FjallStore, KvStore};
//!
//! let store: Arc<dyn KvStore> = Arc::new(FjallStore::open("./data").await?);
//! store.insert("greetings", b"hello", b"world").await?;
//! let value = store.get("greetings", b"hello").await?;
//! ```

pub mod codec;
pub mod traits;

#[cfg(feature = "fjall-backend")]
pub mod fjall_backend;

pub use codec::{u64_from_bytes, u64_to_bytes};
pub use traits::{Bytes, DbError, DbResult, KvOp, KvStore};

#[cfg(feature = "fjall-backend")]
pub use fjall_backend::FjallStore;
