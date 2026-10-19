//! Saturating storage estimates from the owned representation (ADR 0012).
use crate::{FontBytes, Program};
use pagura_core::Name;
use std::collections::BTreeMap;

pub(crate) fn estimate(data: &FontBytes, parsed: &Program) -> usize {
    let heap = match parsed {
        Program::Sfnt(p) => p.heap_bytes(),
        Program::Cff(p) => p.heap_bytes(),
        Program::Type1(p) => p.heap_bytes(),
    };
    // Each Arc control block holds two reference counts: one for the
    // parsed state, and one for shared input bytes.
    let arcs = if matches!(data, FontBytes::Shared(_)) {
        2
    } else {
        1
    };
    data.heap_len()
        .saturating_add(size_of::<Program>())
        .saturating_add(arcs * 2 * size_of::<usize>())
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
            .saturating_add(pagura_core::INDEX_ENTRY_OVERHEAD),
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
