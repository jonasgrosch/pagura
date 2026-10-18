//! Lazily parsed resources, published once under their inventory key.
use pagura_core::Result;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

/// Parsed resources shared by every caller of one provider.
///
/// Keys are the names in the crate's constant tables, so a cache never holds
/// more entries than the inventory, and unknown names are never retained.
pub(crate) struct Cache<T>(Mutex<BTreeMap<&'static [u8], Arc<T>>>);

impl<T> Cache<T> {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(BTreeMap::new()))
    }

    /// The resource cached under `key`, parsing and publishing it first if
    /// it is absent. A failed parse publishes nothing.
    ///
    /// `parse` runs outside the lock, so it may load other keys of the same
    /// cache (a CMap's `usecmap` parent). Concurrent first lookups of one
    /// key may each parse it; the first result published is returned to all.
    pub(crate) fn get_or_try_insert(
        &self,
        key: &'static [u8],
        parse: impl FnOnce() -> Result<T>,
    ) -> Result<Arc<T>> {
        if let Some(value) = self.lock().get(key) {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(parse()?);
        Ok(Arc::clone(self.lock().entry(key).or_insert(value)))
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<&'static [u8], Arc<T>>> {
        // Entries are inserted whole, so a panic while the lock was held
        // cannot have left a partial one behind.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T> fmt::Debug for Cache<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Keys only: a cached font program would print every byte it holds.
        f.debug_set()
            .entries(self.lock().keys().map(|key| String::from_utf8_lossy(key)))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::Cache;
    use pagura_core::{Error, ErrorKind, Result};
    use std::sync::Arc;

    #[test]
    fn values_are_published_once_and_failures_not_at_all() -> Result<()> {
        let cache = Cache::new();
        let error = cache
            .get_or_try_insert(b"a", || Err::<u32, _>(Error::limit("budget")))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(format!("{cache:?}"), "{}");
        let first = cache.get_or_try_insert(b"a", || Ok(1))?;
        let again = cache.get_or_try_insert(b"a", || Err(Error::syntax("parsed again")))?;
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*again, 1);
        Ok(())
    }

    #[test]
    fn parsing_may_load_other_keys_and_debug_shows_only_keys() -> Result<()> {
        let cache = Cache::new();
        let child = cache.get_or_try_insert(b"child", || {
            Ok(*cache.get_or_try_insert(b"parent", || Ok(41))? + 1)
        })?;
        assert_eq!(*child, 42);
        assert_eq!(format!("{cache:?}"), r#"{"child", "parent"}"#);
        Ok(())
    }
}
