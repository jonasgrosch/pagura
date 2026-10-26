//! Estimates of owned object storage; allocator bookkeeping is modelled, not measured.

use crate::{Dictionary, Value};

/// Estimated bookkeeping bytes per entry of a tree index, in addition to its
/// key and value: [`Dictionary`]'s key index and the document's slot and
/// object-stream tables use the same model (ADR 0012).
pub const INDEX_ENTRY_OVERHEAD: usize = 64;

impl Value {
    /// Estimates retained bytes, including this value and owned vector capacities,
    /// strings, heap names and dictionary indexes. References are counted as
    /// identities and are never followed (7.3.10). Shared owners count separately.
    ///
    /// This is a saturating accounting estimate, not allocator telemetry or a
    /// peak-memory bound. Traversal is iterative and linear in the value tree.
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(heap_bytes(std::iter::once(self), Storage::Retained))
    }
}

impl Dictionary {
    /// Estimates this dictionary's retained storage, including owned values,
    /// spare entry capacity, heap keys and its lookup index (7.3.7).
    /// Uses the same saturating estimate as [`Value::retained_bytes`].
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.allocation_bytes())
            .saturating_add(heap_bytes(
                self.iter().map(|(_, value)| value),
                Storage::Retained,
            ))
    }

    /// Estimates the storage of a clone before allocating it. Vector clones
    /// retain their populated elements, without copying unused capacity.
    /// Includes the dictionary header, heap names, nested values and key index,
    /// under the same accounting model as [`Self::retained_bytes`].
    pub fn cloned_retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.cloned_allocation_bytes())
            .saturating_add(heap_bytes(
                self.iter().map(|(_, value)| value),
                Storage::Cloned,
            ))
    }
}

#[derive(Clone, Copy)]
enum Storage {
    Retained,
    Cloned,
}

impl Storage {
    fn capacity(self, length: usize, capacity: usize) -> usize {
        match self {
            Self::Retained => capacity,
            Self::Cloned => length,
        }
    }
}

fn heap_bytes<'a>(values: impl Iterator<Item = &'a Value>, storage: Storage) -> usize {
    let mut pending: Vec<_> = values.collect();
    let mut bytes = 0_usize;
    while let Some(value) = pending.pop() {
        let owned = match value {
            Value::Name(name) => name.heap_bytes(),
            Value::String(string) => storage.capacity(string.len(), string.capacity()),
            Value::Array(array) => {
                pending.extend(array);
                storage
                    .capacity(array.len(), array.capacity())
                    .saturating_mul(size_of::<Value>())
            }
            Value::Dictionary(dictionary) => {
                pending.extend(dictionary.iter().map(|(_, value)| value));
                match storage {
                    Storage::Retained => dictionary.allocation_bytes(),
                    Storage::Cloned => dictionary.cloned_allocation_bytes(),
                }
            }
            _ => 0,
        };
        bytes = bytes.saturating_add(owned);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use crate::{Dictionary, Name, ObjectId, Value};

    #[test]
    fn estimates_include_spare_capacity_and_do_not_follow_references() {
        let mut string = Vec::with_capacity(123);
        string.extend(b"abc");
        let capacity = string.capacity();
        assert_eq!(
            Value::String(string).retained_bytes(),
            size_of::<Value>() + capacity
        );
        assert_eq!(
            Value::Reference(ObjectId::new(1, 0)).retained_bytes(),
            size_of::<Value>()
        );
        assert_eq!(Value::name(b"inline").retained_bytes(), size_of::<Value>());
        assert_eq!(
            Value::name(&[b'x'; 50]).retained_bytes(),
            size_of::<Value>() + 50
        );
        let mut array = Vec::with_capacity(10);
        array.push(Value::name(&[b'x'; 50]));
        let expected = size_of::<Value>() + array.capacity() * size_of::<Value>() + 50;
        assert_eq!(Value::Array(array).retained_bytes(), expected);
    }

    #[test]
    fn dictionary_estimates_include_entry_storage_and_the_large_key_index() {
        let mut dictionary = Dictionary::with_capacity(32);
        for number in 0..16 {
            dictionary.insert(
                Name::new(format!("K{number}").as_bytes()),
                Value::Integer(number),
            );
        }
        let small = dictionary.retained_bytes();
        dictionary.insert(Name::new(&[b'k'; 40]), Value::name(&[b'v'; 50]));
        assert!(dictionary.retained_bytes() >= small + 40 + 50 + 17 * size_of::<usize>());
        let standalone = dictionary.retained_bytes();
        assert_eq!(
            Value::Dictionary(dictionary).retained_bytes(),
            standalone + size_of::<Value>() - size_of::<Dictionary>()
        );
    }

    #[test]
    fn clone_admission_counts_nested_values_and_indexes_without_unused_capacity() {
        let mut string = Vec::with_capacity(96);
        string.extend(b"abc");
        let mut array = Vec::with_capacity(16);
        array.push(Value::String(string));
        array.push(Value::name(&[b'n'; 40]));
        let mut nested = Dictionary::with_capacity(64);
        for number in 0..20 {
            nested.insert(
                Name::new(format!("Name{number}").as_bytes()),
                Value::Integer(number),
            );
        }
        array.push(Value::Dictionary(nested));
        let mut dictionary = Dictionary::with_capacity(32);
        dictionary.insert(Name::new(b"Values"), Value::Array(array));
        let original = dictionary.retained_bytes();
        let admitted = dictionary.cloned_retained_bytes();
        let cloned = dictionary.clone();
        assert_eq!(cloned.retained_bytes(), admitted);
        assert!(admitted < original);
        assert_eq!(dictionary.retained_bytes(), original);
        assert_eq!(dictionary, cloned);
        assert_eq!(
            Dictionary::new().cloned_retained_bytes(),
            size_of::<Dictionary>()
        );
    }
}
