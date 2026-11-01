//! One budgeted store for every reusable value of rendering (ADR 0012,
//! ADR 0034): recorded scenes, decoded images, colour transforms, font
//! programs, glyph edge setups, prepared functions and shadings.

use crate::Result;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

/// A key of the [`Store`]: an identity that is never an address that can be
/// reused, together with the type of value it names.
///
/// Implement it on a small key type in the crate that builds the value, for
/// example `(ObjectId, Reduction)` wrapped in a newtype for a decoded image.
pub trait StoreKey: Hash + Eq + Send + Sync + 'static {
    /// The value built for this key.
    type Value: Send + Sync + 'static;
}

/// A value built for the [`Store`] together with the bytes it retains, which
/// the store charges against its budget while it holds the value.
#[derive(Debug)]
pub struct Built<V> {
    /// The value.
    pub value: V,
    /// Bytes the value retains, including its own allocations: an estimate
    /// in the manner of `retained_bytes`, not allocator telemetry.
    pub bytes: usize,
}

/// Shared values under one byte budget.
///
/// [`Store::get_or_build`] returns the value stored for a key or builds it.
/// When a new value would exceed the budget, the store evicts the least
/// recently used values that only it still holds; a value someone else holds
/// is never evicted while held, so its bytes stay counted. A value that still
/// does not fit is returned without being retained.
///
/// A rendering context owns one store and sets its capacity, which is not a
/// [`Limits`](crate::Limits) field because it bounds what persists across
/// renders (ADR 0012). A build charges the budgets of its own work; what a
/// value costs a render must not depend on whether it was found or built
/// (ADR 0016).
///
/// The store is `Send + Sync`. Building happens outside its lock, so two
/// threads may build the same key at once; the first to finish is kept.
#[derive(Debug)]
pub struct Store {
    capacity: usize,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<u64, Vec<Entry>>,
    bytes: usize,
    clock: u64,
}

struct Entry {
    key: Box<dyn Any + Send + Sync>,
    value: Arc<dyn Any + Send + Sync>,
    bytes: usize,
    used: u64,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("bytes", &self.bytes)
            .field("used", &self.used)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Creates an empty store that retains at most `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// The byte budget.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently retained.
    pub fn retained_bytes(&self) -> usize {
        self.lock().bytes
    }

    /// The value stored for `key`, if any.
    pub fn get<K: StoreKey>(&self, key: &K) -> Option<Arc<K::Value>> {
        let hash = hash_key(key);
        let mut inner = self.lock();
        inner.clock = inner.clock.wrapping_add(1);
        let clock = inner.clock;
        let entry = inner
            .entries
            .get_mut(&hash)?
            .iter_mut()
            .find(|entry| entry.key.downcast_ref::<K>() == Some(key))?;
        entry.used = clock;
        Arc::clone(&entry.value).downcast::<K::Value>().ok()
    }

    /// The value stored for `key`, or the value `build` returns, which is
    /// then stored if it fits the budget.
    ///
    /// # Errors
    ///
    /// Returns the error of `build`; nothing is stored then.
    pub fn get_or_build<K: StoreKey>(
        &self,
        key: K,
        build: impl FnOnce() -> Result<Built<K::Value>>,
    ) -> Result<Arc<K::Value>> {
        if let Some(value) = self.get(&key) {
            return Ok(value);
        }
        let Built { value, bytes } = build()?;
        let value = Arc::new(value);
        self.insert(key, &value, bytes);
        Ok(value)
    }

    /// Drops every value only the store holds.
    pub fn clear(&self) {
        let mut inner = self.lock();
        let mut freed = 0;
        for bucket in inner.entries.values_mut() {
            bucket.retain(|entry| {
                let keep = Arc::strong_count(&entry.value) > 1;
                if !keep {
                    freed += entry.bytes;
                }
                keep
            });
        }
        inner.entries.retain(|_, bucket| !bucket.is_empty());
        inner.bytes -= freed;
    }

    fn insert<K: StoreKey>(&self, key: K, value: &Arc<K::Value>, bytes: usize) {
        if bytes > self.capacity {
            return;
        }
        let hash = hash_key(&key);
        let mut inner = self.lock();
        if inner.entries.get(&hash).is_some_and(|bucket| {
            bucket
                .iter()
                .any(|e| e.key.downcast_ref::<K>() == Some(&key))
        }) {
            return;
        }
        if inner.bytes.saturating_add(bytes) > self.capacity {
            inner.evict(self.capacity - bytes);
        }
        if inner.bytes.saturating_add(bytes) > self.capacity {
            return;
        }
        inner.clock = inner.clock.wrapping_add(1);
        let entry = Entry {
            key: Box::new(key),
            value: Arc::clone(value) as Arc<dyn Any + Send + Sync>,
            bytes,
            used: inner.clock,
        };
        inner.entries.entry(hash).or_default().push(entry);
        inner.bytes += bytes;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// Evicts least recently used values only the store holds until at most
    /// `target` bytes remain or nothing more can be evicted.
    fn evict(&mut self, target: usize) {
        while self.bytes > target {
            let oldest = self
                .entries
                .iter()
                .flat_map(|(&hash, bucket)| {
                    bucket
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| Arc::strong_count(&entry.value) == 1)
                        .map(move |(index, entry)| (entry.used, hash, index))
                })
                .min();
            let Some((_, hash, index)) = oldest else {
                return;
            };
            if let Some(bucket) = self.entries.get_mut(&hash) {
                let entry = bucket.swap_remove(index);
                self.bytes -= entry.bytes;
                if bucket.is_empty() {
                    self.entries.remove(&hash);
                }
            }
        }
    }
}

fn hash_key<K: StoreKey>(key: &K) -> u64 {
    let mut hasher = DefaultHasher::new();
    TypeId::of::<K>().hash(&mut hasher);
    key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Hash, PartialEq, Eq)]
    struct Key(u32);
    impl StoreKey for Key {
        type Value = Vec<u8>;
    }

    #[derive(Debug, Hash, PartialEq, Eq)]
    struct Other(u32);
    impl StoreKey for Other {
        type Value = String;
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "matches the signature get_or_build expects of a build closure"
    )]
    fn built(bytes: usize) -> Result<Built<Vec<u8>>> {
        Ok(Built {
            value: vec![0; bytes],
            bytes,
        })
    }

    #[test]
    fn builds_once_and_shares() {
        let store = Store::new(100);
        let a = store.get_or_build(Key(1), || built(10)).unwrap();
        let b = store
            .get_or_build(Key(1), || panic!("built twice"))
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(store.retained_bytes(), 10);
    }

    #[test]
    fn keys_of_different_types_do_not_collide() {
        let store = Store::new(100);
        store.get_or_build(Key(1), || built(1)).unwrap();
        let text = store
            .get_or_build(Other(1), || {
                Ok(Built {
                    value: "x".to_owned(),
                    bytes: 1,
                })
            })
            .unwrap();
        assert_eq!(text.as_str(), "x");
        assert_eq!(store.retained_bytes(), 2);
    }

    #[test]
    fn evicts_only_values_nobody_else_holds() {
        let store = Store::new(30);
        let held = store.get_or_build(Key(1), || built(10)).unwrap();
        drop(store.get_or_build(Key(2), || built(10)).unwrap());
        drop(store.get_or_build(Key(3), || built(15)).unwrap());
        assert!(store.get(&Key(1)).is_some());
        assert!(store.get(&Key(2)).is_none());
        assert!(store.get(&Key(3)).is_some());
        assert_eq!(store.retained_bytes(), 25);
        drop(held);
        store.clear();
        assert_eq!(store.retained_bytes(), 0);
    }

    #[test]
    fn a_value_larger_than_the_budget_is_returned_but_not_kept() {
        let store = Store::new(5);
        let value = store.get_or_build(Key(1), || built(10)).unwrap();
        assert_eq!(value.len(), 10);
        assert!(store.get(&Key(1)).is_none());
        assert_eq!(store.retained_bytes(), 0);
    }

    #[test]
    fn build_errors_store_nothing() {
        let store = Store::new(5);
        let error = store
            .get_or_build(Key(1), || Err(crate::Error::limit("test")))
            .unwrap_err();
        assert_eq!(error.message, "test");
        assert!(store.get(&Key(1)).is_none());
    }
}
