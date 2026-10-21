//! Lossless integer widening between `usize` and fixed-width integers.
//!
//! Pagura supports targets whose `usize` is 32 or 64 bits wide (WebAssembly
//! and native). On those, `u32 → usize` and `usize → u64` never lose
//! information, but the standard library offers no `From` for them, which
//! leads to a mix of `as` casts and `try_from(..).unwrap_or(MAX)` fallbacks
//! that can never fire. These helpers are the one spelling.

const _: () = assert!(usize::BITS >= 32 && usize::BITS <= 64);

/// Widens a `u32` to `usize`.
pub const fn u32_to_usize(value: u32) -> usize {
    value as usize
}

/// Widens a `usize` to `u64`.
pub const fn usize_to_u64(value: usize) -> u64 {
    value as u64
}

#[cfg(test)]
mod tests {
    use super::{u32_to_usize, usize_to_u64};

    #[test]
    fn widening_preserves_the_extremes() {
        assert_eq!(u32_to_usize(u32::MAX), 4_294_967_295);
        assert_eq!(usize_to_u64(usize::MAX), u64::try_from(usize::MAX).unwrap());
        assert_eq!(usize_to_u64(0), 0);
    }
}
