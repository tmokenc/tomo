//! `Reference` mirrors `twilight_cache_inmemory::Reference` — a guard that
//! borrows from the cache and derefs to the cached value.

use std::hash::Hash;
use std::ops::Deref;

use dashmap::mapref::one::Ref;

pub struct Reference<'a, K: Eq + Hash, V> {
    inner: Ref<'a, K, V>,
}

impl<'a, K: Eq + Hash, V> Reference<'a, K, V> {
    pub(crate) fn new(inner: Ref<'a, K, V>) -> Self {
        Self { inner }
    }

    pub fn key(&self) -> &K {
        self.inner.key()
    }

    pub fn value(&self) -> &V {
        self.inner.value()
    }
}

impl<K: Eq + Hash, V> Deref for Reference<'_, K, V> {
    type Target = V;
    fn deref(&self) -> &V {
        self.inner.value()
    }
}
