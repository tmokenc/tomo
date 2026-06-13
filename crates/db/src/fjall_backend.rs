use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use tokio::task;
use tracing::trace;

use crate::codec::{u64_from_bytes, u64_to_bytes};
use crate::traits::{Bytes, DbError, DbResult, KvOp, KvStore};

/// Fjall-backed implementation of [`KvStore`].
///
/// One [`Database`] per directory; one [`Keyspace`] per partition, created on
/// demand and cached. Every sync fjall call is wrapped in `spawn_blocking` so
/// it never stalls the tokio runtime.
pub struct FjallStore {
    inner: Arc<Inner>,
}

struct Inner {
    db: Database,
    keyspaces: RwLock<HashMap<String, Keyspace>>,
    /// Serializes the read-modify-write in [`increment_blocking`] so the
    /// trait's "atomically adds" contract holds even with concurrent callers
    /// (today only the single stats writer increments, but the lock removes
    /// the latent lost-update footgun for any future caller).
    inc_lock: Mutex<()>,
}

impl FjallStore {
    pub async fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let db = task::spawn_blocking(move || {
            Database::builder(&path).open().map_err(|e| DbError::Backend(e.to_string()))
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))??;
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                keyspaces: RwLock::new(HashMap::new()),
                inc_lock: Mutex::new(()),
            }),
        })
    }

    fn keyspace(&self, name: &str) -> DbResult<Keyspace> {
        if let Some(ks) = self.read_keyspaces().get(name) {
            return Ok(ks.clone());
        }
        let ks = self
            .inner
            .db
            .keyspace(name, KeyspaceCreateOptions::default)
            .map_err(|e| DbError::Backend(format!("open keyspace `{name}`: {e}")))?;
        self.inner
            .keyspaces
            .write()
            .map_err(|_| DbError::Backend("keyspace cache poisoned".into()))?
            .insert(name.to_string(), ks.clone());
        Ok(ks)
    }

    fn read_keyspaces(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Keyspace>> {
        // Poisoned reads are extremely unlikely and not worth surfacing here.
        self.inner.keyspaces.read().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl KvStore for FjallStore {
    async fn get(&self, partition: &str, key: Bytes) -> DbResult<Option<Bytes>> {
        let ks = self.keyspace(partition)?;
        task::spawn_blocking(move || {
            let out = ks
                .get(key.as_ref())
                .map_err(|e| DbError::Backend(format!("get: {e}")))?
                .map(|v| Bytes::copy_from_slice(v.as_ref()));
            Ok::<_, DbError>(out)
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn insert(&self, partition: &str, key: Bytes, value: Bytes) -> DbResult<()> {
        let ks = self.keyspace(partition)?;
        task::spawn_blocking(move || {
            ks.insert(key.as_ref(), value.as_ref())
                .map_err(|e| DbError::Backend(format!("insert: {e}")))?;
            Ok::<_, DbError>(())
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn remove(&self, partition: &str, key: Bytes) -> DbResult<()> {
        let ks = self.keyspace(partition)?;
        task::spawn_blocking(move || {
            ks.remove(key.as_ref()).map_err(|e| DbError::Backend(format!("remove: {e}")))?;
            Ok::<_, DbError>(())
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn list_prefix(&self, partition: &str, prefix: Bytes) -> DbResult<Vec<(Bytes, Bytes)>> {
        let ks = self.keyspace(partition)?;
        task::spawn_blocking(move || {
            let mut out = Vec::new();
            for kv in ks.prefix(prefix.as_ref()) {
                let (k, v) = kv
                    .into_inner()
                    .map_err(|e| DbError::Backend(format!("prefix: {e}")))?;
                out.push((Bytes::copy_from_slice(k.as_ref()), Bytes::copy_from_slice(v.as_ref())));
            }
            Ok::<_, DbError>(out)
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn increment(&self, partition: &str, key: Bytes, by: u64) -> DbResult<u64> {
        let ks = self.keyspace(partition)?;
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let _guard = inner
                .inc_lock
                .lock()
                .map_err(|_| DbError::Backend("increment lock poisoned".into()))?;
            increment_blocking(&ks, key.as_ref(), by)
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn batch(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let store = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            // Collect blind writes into one atomic `WriteBatch`. Previously ops
            // were applied one-by-one, so a mid-batch failure left earlier ops
            // committed and later ones dropped — splitting paired writes (e.g.
            // a `DeleteChannel` landing without its `DeleteChannelMessages`,
            // resurrecting data). `commit()` now applies all-or-nothing.
            let mut wb = store.db.batch();
            let mut has_writes = false;
            for op in ops {
                match op {
                    KvOp::Insert { partition, key, value } => {
                        let ks = open_keyspace_blocking(&store, &partition)?;
                        wb.insert(&ks, key.as_ref(), value.as_ref());
                        has_writes = true;
                    }
                    KvOp::Remove { partition, key } => {
                        let ks = open_keyspace_blocking(&store, &partition)?;
                        wb.remove(&ks, key.as_ref());
                        has_writes = true;
                    }
                    KvOp::Increment { partition, key, by } => {
                        // A read-modify-write can't be expressed in a blind
                        // write batch; apply it directly under the increment
                        // lock. Cache batches are pure insert/remove and the
                        // stats writer's batches are increment-only, so no
                        // paired write is split by handling these separately.
                        let ks = open_keyspace_blocking(&store, &partition)?;
                        let _guard = store
                            .inc_lock
                            .lock()
                            .map_err(|_| DbError::Backend("increment lock poisoned".into()))?;
                        increment_blocking(&ks, key.as_ref(), by)?;
                    }
                }
            }
            if has_writes {
                wb.commit()
                    .map_err(|e| DbError::Backend(format!("batch commit: {e}")))?;
            }
            Ok::<_, DbError>(())
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }

    async fn flush(&self) -> DbResult<()> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            inner
                .db
                .persist(PersistMode::SyncAll)
                .map_err(|e| DbError::Backend(format!("persist: {e}")))?;
            Ok::<_, DbError>(())
        })
        .await
        .map_err(|e| DbError::Backend(format!("spawn_blocking: {e}")))?
    }
}

fn open_keyspace_blocking(inner: &Inner, name: &str) -> DbResult<Keyspace> {
    if let Some(ks) = inner.keyspaces.read().ok().and_then(|map| map.get(name).cloned()) {
        return Ok(ks);
    }
    let ks = inner
        .db
        .keyspace(name, KeyspaceCreateOptions::default)
        .map_err(|e| DbError::Backend(format!("open keyspace `{name}`: {e}")))?;
    if let Ok(mut guard) = inner.keyspaces.write() {
        guard.insert(name.to_string(), ks.clone());
    }
    Ok(ks)
}

fn increment_blocking(ks: &Keyspace, key: &[u8], by: u64) -> DbResult<u64> {
    let current = ks
        .get(key)
        .map_err(|e| DbError::Backend(format!("inc-get: {e}")))?
        .map(|v| u64_from_bytes(v.as_ref()))
        .unwrap_or(0);
    let next = current.saturating_add(by);
    ks.insert(key, u64_to_bytes(next))
        .map_err(|e| DbError::Backend(format!("inc-put: {e}")))?;
    trace!(key = ?key, by, next, "incremented counter");
    Ok(next)
}
