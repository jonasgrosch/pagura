//! Sequential Huffman blocks (T.81 F.2.2) with the reader's window in
//! registers.
//!
//! A block is decoded here while at least 32 bits are loaded before each
//! code, enough for any code and its magnitude, and its codes, magnitudes
//! and runs pass the reference decoder's checks. Otherwise the reader, the
//! predictor and the block are left as they were, and the reference decoder
//! decodes the block again and reports what it finds: a marker or the end
//! of the data within reach, or an invalid code, magnitude or run.
use super::{Bits, Cursor, Huffman, LOOKAHEAD};

impl Bits<'_> {
    /// Decodes one sequential block's coefficients in zigzag order, as the
    /// reference decoder does for a scan without successive approximation;
    /// returns the zigzag positions of the nonzero coefficients. `block`
    /// holds zeros on entry, and still does when this returns None. Kept
    /// apart from the scan loop, the block loop has registers for the
    /// reader, its data and its tables.
    #[inline(never)]
    pub(in crate::dct) fn sequential(
        &mut self,
        tables: (&Huffman, &Huffman),
        precision: u8,
        predictor: &mut i32,
        block: &mut [i32; 64],
    ) -> Option<u64> {
        let mut cursor = self.cursor();
        let Some((dc, nonzero)) =
            decode(&mut cursor, self.data, tables, precision, *predictor, block)
        else {
            block.fill(0);
            return None;
        };
        self.resume(cursor);
        *predictor = dc;
        Some(nonzero)
    }
}

#[expect(
    clippy::inline_always,
    reason = "the reader's window stays in registers only within one function"
)]
#[inline(always)]
fn decode(
    cursor: &mut Cursor,
    data: &[u8],
    (dc, ac): (&Huffman, &Huffman),
    precision: u8,
    predictor: i32,
    block: &mut [i32; 64],
) -> Option<(i32, u64)> {
    let (size, total) = cursor.code(data, dc, |symbol| symbol)?;
    if size > precision + 3 {
        return None;
    }
    // Both the predictor and a difference are below 2^15 in magnitude.
    let value = predictor + cursor.magnitude(total, size);
    let bound = 1 << (precision + 3);
    if !(-bound..bound).contains(&value) {
        return None;
    }
    block[0] = value;
    let mut nonzero = u64::from(value != 0);
    let mut k = 1;
    while k < block.len() {
        let (symbol, total) = cursor.code(data, ac, |symbol| symbol & 15)?;
        let (run, size) = (usize::from(symbol >> 4), symbol & 15);
        if size == 0 {
            cursor.consume(total);
            match run {
                0 => break,
                15 if k + 16 <= block.len() => k += 16,
                _ => return None,
            }
            continue;
        }
        if size > precision + 2 {
            return None;
        }
        k += run;
        *block.get_mut(k)? = cursor.magnitude(total, size);
        nonzero |= 1 << k;
        k += 1;
    }
    Some((value, nonzero))
}

impl Cursor {
    /// The next code's symbol and its code plus magnitude bits, loading
    /// bits first; `size` gives a symbol's magnitude bits. None unless 32
    /// bits are loaded and the code is valid.
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    fn code(&mut self, data: &[u8], table: &Huffman, size: impl Fn(u8) -> u8) -> Option<(u8, u32)> {
        self.fill(data);
        if self.count < 32 {
            return None;
        }
        let code = table.lookup[(self.window >> (64 - LOOKAHEAD)) as usize];
        if code.total != 0 {
            return Some((code.symbol, u32::from(code.total)));
        }
        let (symbol, length) = table.long((self.window >> 48) as usize, self.count)?;
        Some((symbol, length + u32::from(size(symbol))))
    }

    /// Consumes `width` loaded bits.
    fn consume(&mut self, width: u32) {
        // At eight or twelve bit precision, a code and its magnitude take at
        // most 31 of the 32 or more bits loaded before each code, so neither
        // the shift nor the count can wrap.
        debug_assert!(width <= self.count);
        self.window = self.window.wrapping_shl(width);
        self.count = self.count.wrapping_sub(width);
    }

    /// Consumes a code and its `size`-bit magnitude, `total` bits in all,
    /// returning the magnitude's value (T.81 F.2.2.1, Table F.2).
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    fn magnitude(&mut self, total: u32, size: u8) -> i32 {
        let size = u32::from(size);
        // The magnitude ends the 1 to 31 bits; it has at most 15.
        let raw = self.window.wrapping_shr(64_u32.wrapping_sub(total)) & ((1 << size) - 1);
        self.consume(total);
        // Values below half the size's range are negative.
        let negative = 0_i64.wrapping_sub(i64::from(raw < (1 << size) >> 1));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a magnitude of at most 15 bits"
        )]
        let value = (raw.cast_signed() + (negative & (1 - (1 << size)))) as i32;
        value
    }
}
