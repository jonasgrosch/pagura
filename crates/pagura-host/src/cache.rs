//! Session-local LRU for immutable compiled pages (ISO 32000-2, 7.8.2).

use pagura::DisplayList;
use std::collections::VecDeque;
use std::sync::Arc;

/// An entry's estimated accounting charge is a retention policy, not a
/// process-memory ceiling: the renderer does not expose retained heap bytes.
/// A compiled page owns only its commands, never document data, so the charge
/// scales with the list alone. Entry count independently limits the number of
/// retained lists. The fixed charges keep eviction identical on 32- and 64-bit
/// targets.
/// Estimated bytes per display-list command, including its geometry.
const COMMAND_CHARGE: usize = 4096;
/// Estimated bytes for a list header and its cache entry.
const LIST_CHARGE: usize = 512;

#[derive(Debug)]
pub(crate) struct PageCache {
    entries: VecDeque<Entry>,
    budget: usize,
    charged: usize,
}

#[derive(Debug)]
struct Entry {
    key: (u32, u32),
    charge: usize,
    list: Arc<DisplayList>,
}

impl PageCache {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            budget,
            charged: 0,
        }
    }

    pub(crate) fn get(&mut self, key: (u32, u32)) -> Option<Arc<DisplayList>> {
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let list = Arc::clone(&entry.list);
        self.entries.push_back(entry);
        Some(list)
    }

    pub(crate) fn insert(&mut self, key: (u32, u32), list: Arc<DisplayList>) {
        let Some(charge) = list
            .len()
            .checked_mul(COMMAND_CHARGE)
            .and_then(|n| n.checked_add(LIST_CHARGE))
        else {
            return;
        };
        if charge > self.budget || self.budget == 0 {
            return;
        }
        if self.entries.try_reserve(1).is_err() {
            return;
        }
        while self.entries.len() >= 8 || self.charged > self.budget - charge {
            if let Some(entry) = self.entries.pop_front() {
                self.charged -= entry.charge;
            }
        }
        self.charged += charge;
        self.entries.push_back(Entry { key, charge, list });
    }

    pub(crate) fn accounting(&self) -> (usize, usize) {
        (self.charged, self.entries.len())
    }

    pub(crate) fn close(&mut self, document: u32) {
        self.entries.retain(|entry| {
            if entry.key.0 == document {
                self.charged -= entry.charge;
                false
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura::{Document, OpenOptions, compile_page};
    use pagura_testkit::fixture;

    #[test]
    fn hits_refresh_lru_eviction_and_close_release_charges() {
        let document =
            Document::open(fixture("classic", "rectangle.pdf"), &OpenOptions::default()).unwrap();
        let list = Arc::new(compile_page(&document, document.page(0).unwrap()).unwrap());
        let charge = COMMAND_CHARGE + LIST_CHARGE;
        let mut cache = PageCache::new(charge * 2);
        cache.insert((1, 0), Arc::clone(&list));
        cache.insert((1, 1), Arc::clone(&list));
        assert!(Arc::ptr_eq(&cache.get((1, 0)).unwrap(), &list));
        cache.insert((2, 0), Arc::clone(&list));
        assert!(cache.get((1, 1)).is_none());
        cache.close(1);
        assert_eq!(cache.charged, charge);
        cache.close(2);
        assert_eq!(cache.charged, 0);
        let mut small = PageCache::new(charge - 1);
        small.insert((1, 0), Arc::clone(&list));
        assert!(small.entries.is_empty());
        let mut disabled = PageCache::new(0);
        disabled.insert((1, 0), list);
        assert!(disabled.entries.is_empty());
    }
}
