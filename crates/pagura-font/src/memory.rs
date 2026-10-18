//! Saturating storage estimates from the owned representation (ADR 0012).
use crate::Program;
use pagura_core::Name;
use std::collections::BTreeMap;

pub(super) fn estimate(data: &[u8], parsed: &Program) -> usize {
    let heap = match parsed {
        Program::Sfnt(p) => p.heap_bytes(),
        Program::Cff(p) => p.heap_bytes(),
        Program::Type1(p) => p.heap_bytes(),
    };
    data.len()
        .saturating_add(size_of::<Program>())
        .saturating_add(4 * size_of::<usize>()) // two Arc control blocks
        .saturating_add(heap)
}

pub(crate) fn vector<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

pub(crate) fn nested<T>(values: &Vec<Vec<T>>) -> usize {
    values
        .iter()
        .fold(vector(values), |sum, v| sum.saturating_add(vector(v)))
}

pub(crate) fn tree<K, V>(map: &BTreeMap<K, V>) -> usize {
    map.len().saturating_mul(
        size_of::<K>()
            .saturating_add(size_of::<V>())
            .saturating_add(64),
    )
}

pub(crate) fn names<V>(map: &BTreeMap<Name, V>) -> usize {
    map.keys()
        .fold(tree(map), |sum, name| sum.saturating_add(name.len()))
}

pub(crate) fn encoding(values: &[Option<Name>; 256]) -> usize {
    values
        .iter()
        .flatten()
        .fold(size_of_val(values), |sum, name| {
            sum.saturating_add(name.len())
        })
}
