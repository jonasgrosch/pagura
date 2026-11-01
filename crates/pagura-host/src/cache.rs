//! The session-local LRU of extracted text (ADR 0014). Recorded scenes live
//! in each document's render store instead (ADR 0012).
//!
//! An entry's estimated charge is a retention policy, not a process-memory
//! ceiling. The entry count independently limits the number of retained
//! values.

use pagura::TextPage;
use std::collections::VecDeque;
use std::sync::Arc;

/// Most values one cache retains, whatever their charges.
pub(crate) const MAX_ENTRIES: usize = 8;
/// Estimated bytes for a value's inline header and its cache entry.
const ENTRY_CHARGE: usize = 512;

/// A page of an open document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageKey {
    pub(crate) document: u32,
    pub(crate) page: u32,
}

#[derive(Debug)]
pub(crate) struct PageCache<T> {
    entries: VecDeque<Entry<T>>,
    budget: usize,
    charged: usize,
}

#[derive(Debug)]
struct Entry<T> {
    key: PageKey,
    charge: usize,
    value: Arc<T>,
}

impl<T> PageCache<T> {
    /// A cache charging at most `budget`; zero disables retention.
    pub(crate) const fn new(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            budget,
            charged: 0,
        }
    }

    /// The value for `key`, which becomes the most recently used.
    pub(crate) fn get(&mut self, key: PageKey) -> Option<Arc<T>> {
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let value = Arc::clone(&entry.value);
        self.entries.push_back(entry);
        Some(value)
    }

    /// Retains `value` unless its charge exceeds the whole budget, evicting
    /// the least recently used entries to make room.
    pub(crate) fn insert(&mut self, key: PageKey, value: Arc<T>, charge: usize) {
        // Replacing an entry never duplicates its charge.
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(entry) = self.entries.remove(index)
        {
            self.charged -= entry.charge;
        }
        if charge > self.budget || self.budget == 0 {
            return;
        }
        while (self.entries.len() >= MAX_ENTRIES || self.charged > self.budget - charge)
            && let Some(entry) = self.entries.pop_front()
        {
            self.charged -= entry.charge;
        }
        self.charged += charge;
        self.entries.push_back(Entry { key, charge, value });
    }

    /// Charged bytes and retained entries.
    pub(crate) fn accounting(&self) -> (usize, usize) {
        (self.charged, self.entries.len())
    }

    /// Releases every entry of `document`.
    pub(crate) fn close(&mut self, document: u32) {
        self.entries.retain(|entry| {
            if entry.key.document == document {
                self.charged -= entry.charge;
                false
            } else {
                true
            }
        });
    }
}

/// An extracted page's charge: the engine's owned-capacity estimate plus the
/// fixed entry allowance, which `TextPage` excludes.
pub(crate) fn text_charge(page: &TextPage) -> usize {
    page.retained_bytes().saturating_add(ENTRY_CHARGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(document: u32, page: u32) -> PageKey {
        PageKey { document, page }
    }

    #[test]
    fn hits_refresh_lru_eviction_and_close_release_charges() {
        let value = Arc::new("text");
        let charge = 100;
        let mut cache = PageCache::new(charge * 2);
        cache.insert(key(1, 0), Arc::clone(&value), charge);
        cache.insert(key(1, 1), Arc::clone(&value), charge);
        assert!(Arc::ptr_eq(&cache.get(key(1, 0)).unwrap(), &value));
        cache.insert(key(2, 0), Arc::clone(&value), charge);
        assert!(cache.get(key(1, 1)).is_none());
        cache.close(1);
        assert_eq!(cache.accounting(), (charge, 1));
        cache.close(2);
        assert_eq!(cache.accounting(), (0, 0));
        let mut small = PageCache::new(charge - 1);
        small.insert(key(1, 0), Arc::clone(&value), charge);
        assert_eq!(small.accounting(), (0, 0));
        let mut disabled = PageCache::new(0);
        disabled.insert(key(1, 0), value, charge);
        assert_eq!(disabled.accounting(), (0, 0));
    }

    #[test]
    fn the_entry_cap_evicts_even_when_charges_fit() {
        let mut cache = PageCache::new(usize::MAX);
        for page in 0..=u32::try_from(MAX_ENTRIES).unwrap() {
            cache.insert(key(1, page), Arc::new(page), 1);
        }
        assert_eq!(cache.accounting(), (MAX_ENTRIES, MAX_ENTRIES));
        assert!(cache.get(key(1, 0)).is_none());
        assert_eq!(cache.get(key(1, 1)).as_deref(), Some(&1));
    }
}
