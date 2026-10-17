//! Estimates of owned object storage; allocator bookkeeping is modelled, not measured.

use crate::{Dictionary, Value};

impl Value {
    /// Estimates retained bytes, including this value and owned vector capacities,
    /// strings, heap names and dictionary indexes. References are counted as
    /// identities and are never followed (7.3.10). Shared owners count separately.
    ///
    /// This is a saturating accounting estimate, not allocator telemetry or a
    /// peak-memory bound. Traversal is iterative and linear in the value tree.
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(heap_bytes(std::iter::once(self)))
    }
}

impl Dictionary {
    /// Estimates this dictionary's retained storage, including owned values,
    /// spare entry capacity, heap keys and its lookup index (7.3.7).
    /// Uses the same saturating estimate as [`Value::retained_bytes`].
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.allocation_bytes())
            .saturating_add(heap_bytes(self.iter().map(|(_, value)| value)))
    }
}

fn heap_bytes<'a>(values: impl Iterator<Item = &'a Value>) -> usize {
    let mut pending: Vec<_> = values.collect();
    let mut bytes = 0_usize;
    while let Some(value) = pending.pop() {
        let owned = match value {
            Value::Name(name) => name.heap_bytes(),
            Value::String(string) => string.capacity(),
            Value::Array(array) => {
                pending.extend(array);
                array.capacity().saturating_mul(size_of::<Value>())
            }
            Value::Dictionary(dictionary) => {
                pending.extend(dictionary.iter().map(|(_, value)| value));
                dictionary.allocation_bytes()
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
}
