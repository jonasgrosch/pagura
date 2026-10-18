//! Best-effort clearing of key material once it is no longer needed (ADR 0021).
//!
//! Safe Rust cannot promise that no copy survives. Moves, reallocation,
//! registers and stack spills are outside its control, and the optimizer may
//! drop a store that nothing reads; [`std::hint::black_box`] only discourages
//! that. Clearing shortens the time key bytes stay in freed memory. It is not a
//! security boundary, in line with the crate's other non-guarantees.

/// Overwrites `values` with their default (zero) value.
pub(crate) fn wipe<T: Copy + Default>(values: &mut [T]) {
    values.fill(T::default());
    std::hint::black_box(values);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiping_clears_values() {
        let mut words = [7_u32, 8, 9];
        wipe(&mut words);
        assert_eq!(words, [0; 3]);
    }
}
