use async_trait::async_trait;
use thiserror::Error;

// Re-export `bytes::Bytes` directly — cloning is O(1) (refcount bump), so we
// take it by value everywhere instead of fussing with a borrowed-slice type.
pub use bytes::Bytes;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type DbResult<T> = Result<T, DbError>;

/// Async, partitioned key-value interface used by every Tomo crate that
/// persists state. Backends are expected to do their own IO off the runtime
/// (typically via `tokio::task::spawn_blocking`).
///
/// Method semantics:
/// - `partition` is a logical namespace (column family / fjall keyspace /
///   table). Backends must auto-create unknown partitions on first write.
/// - `get` returns `None` for missing keys.
/// - `increment` atomically adds `by` to a u64 stored under `key`. If the key
///   does not exist it starts at zero.
#[async_trait]
pub trait KvStore: Send + Sync + 'static {
    async fn get(&self, partition: &str, key: Bytes) -> DbResult<Option<Bytes>>;
    async fn insert(&self, partition: &str, key: Bytes, value: Bytes) -> DbResult<()>;
    async fn remove(&self, partition: &str, key: Bytes) -> DbResult<()>;

    async fn list_prefix(&self, partition: &str, prefix: Bytes) -> DbResult<Vec<(Bytes, Bytes)>>;

    async fn increment(&self, partition: &str, key: Bytes, by: u64) -> DbResult<u64>;

    async fn batch(&self, ops: Vec<KvOp>) -> DbResult<()>;

    /// Force-persist pending writes. Backends may treat this as a hint.
    async fn flush(&self) -> DbResult<()>;
}

#[derive(Debug, Clone)]
pub enum KvOp {
    Insert { partition: String, key: Bytes, value: Bytes },
    Remove { partition: String, key: Bytes },
    Increment { partition: String, key: Bytes, by: u64 },
}
