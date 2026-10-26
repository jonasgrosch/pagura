//! Order-preserving PDF dictionaries (ISO 32000-2 clause 7.3.7).

use crate::{INDEX_ENTRY_OVERHEAD, Name, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::iter::FusedIterator;

/// Up to this many entries, lookup scans the contiguous entries. Beyond it a
/// key index keeps lookup and insertion logarithmic, so a dictionary with many
/// distinct keys cannot make parsing or building it quadratic.
const LINEAR_SCAN: usize = 16;

/// A PDF dictionary that preserves source key order.
///
/// Source order matters for faithful rewriting and for diagnostics; it has no
/// semantic meaning, so equality ignores it. Small dictionaries, the common
/// case, are a linear scan over contiguous entries; larger ones keep an index.
#[derive(Clone, Default)]
pub struct Dictionary {
    entries: Vec<(Name, Value)>,
    /// Entry positions by key, present once `entries` outgrows [`LINEAR_SCAN`].
    index: Option<BTreeMap<Name, usize>>,
}

impl Dictionary {
    /// Creates an empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty dictionary with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            index: None,
        }
    }

    /// Estimate index storage with [`INDEX_ENTRY_OVERHEAD`] bytes of tree
    /// bookkeeping per entry; cloned index keys own their heap bytes
    /// independently of entry keys.
    pub(crate) fn allocation_bytes(&self) -> usize {
        self.allocation_bytes_with_capacity(self.entries.capacity())
    }

    pub(crate) fn cloned_allocation_bytes(&self) -> usize {
        self.allocation_bytes_with_capacity(self.entries.len())
    }

    fn allocation_bytes_with_capacity(&self, capacity: usize) -> usize {
        let entries = capacity.saturating_mul(size_of::<(Name, Value)>());
        let keys = self.entries.iter().fold(0_usize, |sum, (name, _)| {
            sum.saturating_add(name.heap_bytes())
        });
        let index = self.index.as_ref().map_or(0, |index| {
            index.keys().fold(0_usize, |sum, name| {
                sum.saturating_add(size_of::<(Name, usize)>())
                    .saturating_add(INDEX_ENTRY_OVERHEAD)
                    .saturating_add(name.heap_bytes())
            })
        });
        entries.saturating_add(keys).saturating_add(index)
    }

    fn position(&self, key: &[u8]) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).copied(),
            None => self.entries.iter().position(|(name, _)| name == key),
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the dictionary has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value stored under `key`, if any.
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        let position = self.position(key)?;
        self.entries.get(position).map(|(_, value)| value)
    }

    /// Mutable access to the value stored under `key`, if any.
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Value> {
        let position = self.position(key)?;
        self.entries.get_mut(position).map(|(_, value)| value)
    }

    /// Whether an entry for `key` exists.
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.position(key).is_some()
    }

    /// The name stored under `key`, if the entry exists and is a name.
    pub fn get_name(&self, key: &[u8]) -> Option<&Name> {
        self.get(key).and_then(Value::as_name)
    }

    /// Whether `key` holds exactly the name `expected` (e.g. `/Type /Page`).
    pub fn is_name(&self, key: &[u8], expected: &[u8]) -> bool {
        self.get_name(key).is_some_and(|name| name == expected)
    }

    /// Inserts or replaces an entry. A replaced entry keeps its position.
    pub fn insert(&mut self, key: Name, value: Value) -> Option<Value> {
        if let Some(existing) = self.get_mut(key.as_bytes()) {
            return Some(std::mem::replace(existing, value));
        }
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), self.entries.len());
        }
        self.entries.push((key, value));
        if self.index.is_none() && self.entries.len() > LINEAR_SCAN {
            self.index = Some(
                self.entries
                    .iter()
                    .enumerate()
                    .map(|(position, (name, _))| (name.clone(), position))
                    .collect(),
            );
        }
        None
    }

    /// Removes the entry for `key`, preserving the order of the others.
    pub fn remove(&mut self, key: &[u8]) -> Option<Value> {
        let position = self.position(key)?;
        let (_, value) = self.entries.remove(position);
        if let Some(index) = &mut self.index {
            index.remove(key);
            for later in index.values_mut().filter(|later| **later > position) {
                *later -= 1;
            }
        }
        Some(value)
    }

    /// Entries in source order. `for (key, value) in &dictionary` does the
    /// same.
    pub fn iter(&self) -> DictionaryIter<'_> {
        DictionaryIter(self.entries.iter())
    }

    /// Entries in source order, with mutable values and immutable keys.
    ///
    /// Changing a value preserves the dictionary's key uniqueness and order.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = (&Name, &mut Value)> {
        self.entries.iter_mut().map(|(name, value)| (&*name, value))
    }

    /// Keys in source order.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = &Name> {
        self.entries.iter().map(|(name, _)| name)
    }
}

/// Order-insensitive: two dictionaries are equal when they map the same keys
/// to equal values.
impl PartialEq for Dictionary {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key.as_bytes()) == Some(value))
    }
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Collects entries; a later duplicate key replaces the earlier value.
impl FromIterator<(Name, Value)> for Dictionary {
    fn from_iter<I: IntoIterator<Item = (Name, Value)>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let mut dictionary = Self::with_capacity(iter.size_hint().0);
        for (key, value) in iter {
            dictionary.insert(key, value);
        }
        dictionary
    }
}

impl Extend<(Name, Value)> for Dictionary {
    fn extend<I: IntoIterator<Item = (Name, Value)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl IntoIterator for Dictionary {
    type Item = (Name, Value);
    type IntoIter = std::vec::IntoIter<(Name, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a Dictionary {
    type Item = (&'a Name, &'a Value);
    type IntoIter = DictionaryIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The entries of a [`Dictionary`] in source order, from
/// [`Dictionary::iter`] or `&Dictionary`'s `IntoIterator`.
#[derive(Debug, Clone)]
pub struct DictionaryIter<'a>(std::slice::Iter<'a, (Name, Value)>);

impl<'a> Iterator for DictionaryIter<'a> {
    type Item = (&'a Name, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(name, value)| (name, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for DictionaryIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(name, value)| (name, value))
    }
}

impl ExactSizeIterator for DictionaryIter<'_> {}

impl FusedIterator for DictionaryIter<'_> {}

#[cfg(test)]
mod tests {
    use super::Dictionary;
    use crate::{Name, Value};

    #[test]
    fn preserves_order_and_replaces_in_place() {
        let mut dictionary: Dictionary = [
            (Name::new(b"B"), Value::Integer(1)),
            (Name::new(b"A"), Value::Integer(2)),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            dictionary.insert(Name::new(b"B"), Value::Integer(3)),
            Some(Value::Integer(1))
        );
        let keys: Vec<_> = dictionary.keys().map(Name::as_bytes).collect();
        assert_eq!(keys, [b"B".as_slice(), b"A"]);
        assert_eq!(dictionary.get(b"B"), Some(&Value::Integer(3)));
    }

    #[test]
    fn mutable_iteration_preserves_keys_and_order_with_one_visit_per_value() {
        let mut dictionary: Dictionary = [
            (Name::new(b"B"), Value::Integer(1)),
            (Name::new(b"A"), Value::Integer(2)),
        ]
        .into_iter()
        .collect();
        let mut visited = Vec::new();
        let mut entries = dictionary.iter_mut();
        assert_eq!(entries.len(), 2);
        for (name, value) in &mut entries {
            visited.push(name.as_bytes().to_vec());
            *value = Value::String(name.as_bytes().to_vec());
        }
        assert_eq!(entries.len(), 0);
        drop(entries);
        assert_eq!(visited, [b"B".to_vec(), b"A".to_vec()]);
        assert_eq!(dictionary.get(b"B"), Some(&Value::String(b"B".to_vec())));
        assert_eq!(dictionary.get(b"A"), Some(&Value::String(b"A".to_vec())));
    }

    #[test]
    fn indexed_dictionaries_match_a_linear_model_across_the_threshold() {
        let mut dictionary = Dictionary::new();
        let mut model: Vec<(Vec<u8>, i64)> = Vec::new();
        // A deterministic mix of new keys, replacements and removals, so the
        // index is built, updated and shifted while order must be preserved.
        let mut state = 7_u32;
        for step in 0..2_000_i64 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let key = format!("K{}", (state >> 16) % 97).into_bytes();
            if state.is_multiple_of(5) {
                let expected = model
                    .iter()
                    .position(|(name, _)| *name == key)
                    .map(|position| model.remove(position).1);
                assert_eq!(dictionary.remove(&key), expected.map(Value::Integer));
            } else {
                let expected =
                    if let Some((_, value)) = model.iter_mut().find(|(name, _)| *name == key) {
                        Some(std::mem::replace(value, step))
                    } else {
                        model.push((key.clone(), step));
                        None
                    };
                assert_eq!(
                    dictionary.insert(Name::new(&key), Value::Integer(step)),
                    expected.map(Value::Integer)
                );
            }
            assert_eq!(dictionary.len(), model.len());
        }
        assert!(dictionary.index.is_some());
        let keys: Vec<_> = dictionary.keys().map(Name::as_bytes).collect();
        let expected: Vec<_> = model.iter().map(|(name, _)| name.as_slice()).collect();
        assert_eq!(keys, expected);
        for (name, value) in &model {
            assert_eq!(dictionary.get(name), Some(&Value::Integer(*value)));
        }
        assert_eq!(dictionary.get(b"absent"), None);
    }

    #[test]
    fn borrowed_iteration_visits_entries_in_source_order() {
        let dictionary: Dictionary = [
            (Name::new(b"B"), Value::Integer(1)),
            (Name::new(b"A"), Value::Integer(2)),
        ]
        .into_iter()
        .collect();
        let mut visited = Vec::new();
        for (key, value) in &dictionary {
            visited.push((key.as_bytes(), value.as_integer()));
        }
        assert_eq!(visited, [(b"B".as_slice(), Some(1)), (b"A", Some(2))]);
        let mut entries = dictionary.iter();
        assert_eq!(entries.len(), 2);
        let last = entries.next_back().map(|(key, _)| key.as_bytes());
        assert_eq!(last, Some(b"A".as_slice()));
    }

    #[test]
    fn equality_ignores_order() {
        let forward: Dictionary = [
            (Name::new(b"A"), Value::Null),
            (Name::new(b"B"), Value::Null),
        ]
        .into_iter()
        .collect();
        let backward: Dictionary = [
            (Name::new(b"B"), Value::Null),
            (Name::new(b"A"), Value::Null),
        ]
        .into_iter()
        .collect();
        assert_eq!(forward, backward);
    }
}
