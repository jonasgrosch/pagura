//! Twelve-bit canonical Huffman prefixes over a bounded, unstuffed copy of
//! one entropy-coded segment (T.81 C.2, F.2.2).
//!
//! The segment up to its next marker is copied once without its stuffed
//! zero bytes, charging two operations per input byte and its storage.
//! Positions are mapped back to the stuffed data for errors and alignment.
use super::{Bits, Huffman};
use pagura_core::{Error, Result};

#[derive(Clone, Default)]
pub(crate) struct Stream {
    bytes: Vec<u8>,
    stuffed: Vec<usize>,
    start: usize,
    length: usize,
    bit: usize,
    loaded: bool,
    cap: usize,
    work: usize,
    work_cap: usize,
}

impl Stream {
    pub(crate) fn new(cap: usize, work_cap: usize) -> Self {
        Self {
            cap,
            work_cap,
            ..Self::default()
        }
    }
    pub(crate) fn work(&self) -> usize {
        self.work
    }
}

// A zero-byte test over the complemented word finds every 0xff byte.
// Subtraction is explicitly modulo 2^64; no target-specific operations are used.
fn plain_word(bytes: [u8; 8]) -> bool {
    let word = !u64::from_le_bytes(bytes);
    word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080 == 0
}

fn load(bits: &mut Bits<'_>) -> Result<()> {
    if bits.stream.loaded {
        return Ok(());
    }
    // Native scans read only through this stream, so nothing is loaded yet.
    let start = bits.next;
    let (mut end, mut count) = (start, 0usize);
    while let Some(&byte) = bits.data.get(end) {
        if end - start > (bits.stream.work_cap - bits.stream.work) / 2 {
            return Err(Error::limit("JPEG entropy work exceeded"));
        }

        if ((bits.stream.work_cap - bits.stream.work) / 2).saturating_sub(end - start) >= 8
            && bits.data[end..]
                .first_chunk::<8>()
                .is_some_and(|w| plain_word(*w))
        {
            end += 8;
            continue;
        }
        if byte == 255 {
            if bits.data.get(end + 1) != Some(&0) {
                break;
            }
            count += 1;
            end += 2;
        } else {
            end += 1;
        }
    }
    bits.stream.work = bits
        .stream
        .work
        .checked_add((end - start).saturating_mul(2))
        .filter(|&n| n <= bits.stream.work_cap)
        .ok_or(Error::limit("JPEG entropy work exceeded"))?;
    let length = end - start - count;
    let needed = length
        .checked_add(8)
        .and_then(|n| {
            count
                .checked_mul(size_of::<usize>())
                .and_then(|extra| n.checked_add(extra))
        })
        .ok_or(Error::limit("JPEG entropy storage overflow"))?;
    if needed > bits.stream.cap {
        return Err(Error::limit("JPEG entropy storage exceeded"));
    }
    let stream = &mut bits.stream;
    stream.bytes.clear();
    stream.stuffed.clear();
    stream
        .bytes
        .try_reserve_exact(length + 8)
        .map_err(|_| Error::limit("JPEG entropy allocation failed"))?;
    stream
        .stuffed
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("JPEG entropy allocation failed"))?;
    if stream
        .bytes
        .capacity()
        .checked_add(stream.stuffed.capacity() * size_of::<usize>())
        .is_none_or(|n| n > stream.cap)
    {
        return Err(Error::limit("JPEG entropy retained storage exceeded"));
    }
    let (mut at, mut run) = (start, start);
    while at < end {
        if end - at >= 8
            && bits.data[at..]
                .first_chunk::<8>()
                .is_some_and(|w| plain_word(*w))
        {
            at += 8;
            continue;
        }
        if bits.data[at] == 255 {
            stream.bytes.extend_from_slice(&bits.data[run..=at]);
            stream.stuffed.push(stream.bytes.len() - 1);
            at += 2;
            run = at;
        } else {
            at += 1;
        }
    }
    stream.bytes.extend_from_slice(&bits.data[run..end]);
    stream.bytes.resize(length + 8, 0);
    stream.start = start;
    stream.length = length;
    stream.bit = 0;
    stream.loaded = true;
    Ok(())
}

pub(crate) fn position(bits: &Bits<'_>) -> usize {
    if !bits.stream.loaded {
        return bits.offset();
    }
    let used = bits.stream.bit.div_ceil(8);
    bits.stream.start + used + bits.stream.stuffed.partition_point(|&p| p < used)
}

/// Hands the stream's position to the bit reader: past the last byte with a
/// consumed bit, with that byte's unconsumed low bits loaded for alignment.
pub(crate) fn sync(bits: &mut Bits<'_>) {
    if !bits.stream.loaded {
        return;
    }
    let used = bits.stream.bit.div_ceil(8);
    let left = (8 - bits.stream.bit % 8) % 8;
    let byte = if used == 0 {
        0
    } else {
        bits.stream.bytes[used - 1]
    };
    bits.next = position(bits);
    bits.start = bits.next;
    bits.count = u32::try_from(left).unwrap_or(0);
    bits.window = if left == 0 {
        0
    } else {
        u64::from(byte) << (64 - left)
    };
}

pub(crate) fn restart(bits: &mut Bits<'_>) {
    bits.stream.loaded = false;
}

#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "caller validates a result of at most 32 bits"
)]
fn peek(stream: &Stream, count: u8) -> u32 {
    let at = stream.bit / 8;
    let b = &stream.bytes[at..at + 8];
    let word = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    ((word << (stream.bit % 8)) >> (64 - count)) as u32
}

/// One packed AC prefix: either a code or one/two fused coefficients.
/// Storage stays one u32; only this type knows its bit layout.
#[derive(Clone, Copy)]
struct Ac(u32);

impl Ac {
    const LENGTH: u32 = 15;
    const RUN: u32 = 15;
    const RUN_SHIFT: u32 = 4;
    const LAST: u32 = 31;
    const LAST_SHIFT: u32 = 8;
    const VALUE_BITS: u32 = 11;
    const VALUE: u32 = (1 << Self::VALUE_BITS) - 1;
    const VALUE_SHIFT: u32 = 13;
    const SECOND_SHIFT: u32 = 24;
    const CODE_SHIFT: u32 = 16;

    fn code(symbol: u32, bits: u32) -> Self {
        Self(symbol << Self::LAST_SHIFT | bits << Self::CODE_SHIFT)
    }

    fn coefficient(bits: u32, run: u32, value: u32) -> Self {
        Self(
            bits | run << Self::RUN_SHIFT
                | run << Self::LAST_SHIFT
                | (value & Self::VALUE) << Self::VALUE_SHIFT,
        )
    }

    fn length(self) -> u32 {
        self.0 & Self::LENGTH
    }
    fn run(self) -> u32 {
        self.0 >> Self::RUN_SHIFT & Self::RUN
    }
    fn last(self) -> u32 {
        self.0 >> Self::LAST_SHIFT & Self::LAST
    }
    fn value(self) -> i32 {
        let align = u32::BITS - Self::VALUE_BITS;
        (self.0 << (align - Self::VALUE_SHIFT)).cast_signed() >> align
    }
    fn second(self) -> i32 {
        self.0.cast_signed() >> Self::SECOND_SHIFT
    }

    fn following(self, next: Self) -> Self {
        match i8::try_from(next.value()) {
            Ok(value)
                if self.length() != 0
                    && next.length() != 0
                    && self.length() + next.length() <= 12 =>
            {
                Self(
                    self.0
                        + next.length()
                        + ((1 + next.run()) << Self::LAST_SHIFT)
                        + (u32::from(value.cast_unsigned()) << Self::SECOND_SHIFT),
                )
            }
            _ => self,
        }
    }

    fn fallback(self) -> Option<(u8, u32)> {
        let [_, symbol, length, _] = self.0.to_le_bytes();
        (self.0 != 0 && self.length() == 0).then_some((symbol, u32::from(length)))
    }
}

/// Twelve-bit prefix lookups. A DC entry holds the code's symbol, its
/// length and, when its magnitude fits the prefix (flag 0x8000), the
/// magnitude's value (bits 16–31) and the length with it.
///
/// An AC entry where the prefix starts with a coefficient of nonzero size
/// whose magnitude fits holds its length with the magnitude (bits 0–3), its
/// run (4–7), the advance from the run's start to the last coefficient
/// (8–12, the run) and its value (13–23). With `pairs`, a following
/// coefficient that fits as well and whose value fits a byte joins it:
/// their total length, the advance to it and its value (24–31). Any other
/// AC code has its symbol (8–15) and code length (16–20), bits 0–3 clear.
pub(super) fn wide_table(
    first: &[u32; 16],
    count: &[u8; 16],
    offsets: &[usize; 16],
    symbols: &[u8],
    (ac, pairs): (bool, bool),
) -> [u32; 4096] {
    let mut table = [0u32; 4096];
    for length in 1..=12usize {
        for i in 0..usize::from(count[length - 1]) {
            let code = (first[length - 1] as usize + i) << (12 - length);
            let symbol = symbols[offsets[length - 1] + i];
            let size = usize::from(if ac { symbol & 15 } else { symbol });
            let entries = &mut table[code..code + (1 << (12 - length))];
            let (symbol, run) = (u32::from(symbol), u32::from(symbol >> 4));
            let (bits, used) = (
                u32::try_from(length).unwrap_or(12),
                u32::try_from(size).unwrap_or(0),
            );
            // The magnitude's value in a slot's suffix bits (T.81 F.2.2.1).
            let value = |suffix: usize| {
                let magnitude = (suffix >> (12 - length - size)) & ((1 << size) - 1);
                let value = i32::try_from(magnitude).unwrap_or(0);
                let value = if size > 0 && value < (1 << (size - 1)) {
                    value + 1 - (1 << size)
                } else {
                    value
                };
                u32::from(u16::from_ne_bytes(
                    i16::try_from(value).unwrap_or(0).to_ne_bytes(),
                ))
            };
            let slots = entries.iter_mut().enumerate();
            match (ac, size <= if ac { 10 } else { 11 } && length + size <= 12) {
                (false, true) => {
                    let entry = symbol | 0x8000 | (bits + used) << 8;
                    slots.for_each(|(suffix, slot)| *slot = entry | value(suffix) << 16);
                }
                (true, true) if size > 0 => {
                    // An AC value has at most eleven bits.
                    slots.for_each(|(suffix, slot)| {
                        *slot = Ac::coefficient(bits + used, run, value(suffix)).0;
                    });
                }
                (false, false) => entries.fill(symbol | bits << 8),
                (true, _) => entries.fill(Ac::code(symbol, bits).0),
            }
        }
    }
    if ac && pairs {
        let single = table;
        table = std::array::from_fn(|prefix| pair(&single, prefix));
    }
    table
}

/// The AC entry of `prefix` joined with the coefficient that follows, when
/// both fit the prefix and the second value fits a byte.
fn pair(table: &[u32; 4096], prefix: usize) -> u32 {
    let entry = Ac(table[prefix]);
    entry
        .following(Ac(table[(prefix << entry.length()) & 4095]))
        .0
}

/// Joins a DC magnitude and its following EOB into one twelve-bit prefix.
pub(super) fn shortcut(dc: &Huffman, ac: &Huffman, bits: &mut Bits<'_>) -> Result<[u32; 4096]> {
    bits.stream.work = bits
        .stream
        .work
        .checked_add(4096)
        .filter(|&n| n <= bits.stream.work_cap)
        .ok_or(Error::limit("JPEG native prefix work exceeded"))?;
    Ok(std::array::from_fn(|prefix| {
        let entry = dc.wide[prefix];
        let used = ((entry >> 8) & 31) as usize;
        if entry & 0x8000 == 0 || used >= 12 {
            return 0;
        }
        let Some((0, length)) = Ac(ac.wide[(prefix << used) & 4095]).fallback() else {
            return 0;
        };
        let length = length as usize;
        if length == 0 || length + used > 12 {
            return 0;
        }
        (entry & 0xffff_0000) | u32::try_from(used + length).unwrap_or(0)
    }))
}

/// Decodes directly to one dequantized block, stored column by column,
/// avoiding retained coefficient planes.
pub(super) fn native(
    dc: &Huffman,
    ac: &Huffman,
    bits: &mut Bits<'_>,
    predictor: &mut i32,
    output: &mut [i32; 64],
    quant: &[u16; 64],
    shortcut: &[u32],
) -> Result<(bool, u64)> {
    if !bits.stream.loaded {
        load(bits)?;
    }
    let stream = &mut bits.stream;
    if stream.length * 8 - stream.bit >= 12 {
        let entry = shortcut[peek(stream, 12) as usize];
        if entry & 255 != 0 {
            let difference = i32::from(i16::from_ne_bytes(
                u16::try_from(entry >> 16).unwrap_or(0).to_ne_bytes(),
            ));
            *predictor = predictor
                .checked_add(difference)
                .filter(|&v| (-2048..2048).contains(&v))
                .ok_or_else(|| bits.error("JPEG DC predictor out of range"))?;
            bits.stream.bit += (entry & 255) as usize;
            output[0] = *predictor * i32::from(quant[0]);
            return Ok((false, u64::from(output[0].unsigned_abs())));
        }
    }
    let window = Window::new(&stream.bytes, stream.bit);
    // A block that used bits past the segment fails.
    let (result, bit) = block((dc, ac), window, predictor, output, quant)
        .filter(|&(_, bit)| bit <= stream.length * 8)
        .ok_or_else(|| bits.error("invalid JPEG native entropy"))?;
    bits.stream.bit = bit;
    Ok(result)
}

/// A 64-bit window on the unstuffed segment at bit `bit`, refilled with one
/// load. Bits past the segment read as zeros, so a block decodes without
/// testing for the segment's end until it is complete.
struct Window<'a> {
    bytes: &'a [u8],
    bit: usize,
    word: u64,
    valid: u32,
}

impl<'a> Window<'a> {
    fn new(bytes: &'a [u8], bit: usize) -> Self {
        let mut window = Self {
            bytes,
            bit,
            word: 0,
            valid: 0,
        };
        window.refill();
        window
    }

    /// Holds at least 57 bits from `bit` on. The segment's eight padding
    /// bytes make every load within it whole.
    #[expect(
        clippy::inline_always,
        reason = "one load serves several entropy tokens"
    )]
    #[inline(always)]
    fn refill(&mut self) {
        let at = self.bit / 8;
        let word = self
            .bytes
            .get(at..at + 8)
            .and_then(<[u8]>::first_chunk::<8>)
            .map_or(0, |bytes| u64::from_be_bytes(*bytes));
        let skip = u32::from(self.bit.to_le_bytes()[0] & 7);
        self.word = word << skip;
        self.valid = 64 - skip;
    }

    /// The next `count` bits, 1 ≤ count ≤ `valid`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most 32 bits are requested"
    )]
    fn peek(&self, count: u32) -> u32 {
        self.word.wrapping_shr(64u32.wrapping_sub(count)) as u32
    }

    // A block consumes at most 1,700 bits, `count` never exceeds `valid`,
    // and shifts stay below 64, so the window's arithmetic cannot wrap.
    fn consume(&mut self, count: u32) {
        self.bit = self.bit.wrapping_add(count as usize);
        self.word = self.word.wrapping_shl(count);
        self.valid = self.valid.wrapping_sub(count);
    }

    /// The symbol and value of the code at the window: `code`'s symbol and
    /// code length, or one decoded from the canonical table. None for an
    /// invalid code or a magnitude of more than eleven bits.
    #[expect(
        clippy::inline_always,
        reason = "the window stays in the block decoder's registers"
    )]
    #[inline(always)]
    fn token(&mut self, table: &Huffman, code: Option<(u8, u32)>, ac: bool) -> Option<(u8, i32)> {
        if self.valid < 32 {
            self.refill();
        }
        let (symbol, length) = match code {
            Some(code) => code,
            None => (1..=16).find_map(|length| {
                let i = length as usize - 1;
                let relative = self
                    .peek(length)
                    .checked_sub(table.first[i])
                    .filter(|&v| v < u32::from(table.count[i]))?;
                Some((table.symbols[table.offset[i] + relative as usize], length))
            })?,
        };
        self.consume(length);
        let size = if ac { symbol & 15 } else { symbol };
        if size > 11 {
            return None;
        }
        let raw = if size == 0 {
            0
        } else {
            self.peek(u32::from(size))
        };
        self.consume(u32::from(size));
        Some((symbol, super::extend(raw, size)))
    }
}

/// Decodes a block's DC difference into `predictor` and its dequantized
/// coefficients into `output`, column by column. Returns whether an AC
/// coefficient is nonzero, the coefficients' L1 norm and the bit after
/// the block, or None for invalid entropy; the caller rejects a block
/// read past the segment.
#[inline(never)]
fn block(
    (dc, ac): (&Huffman, &Huffman),
    mut window: Window<'_>,
    predictor: &mut i32,
    output: &mut [i32; 64],
    quant: &[u16; 64],
) -> Option<((bool, u64), usize)> {
    let dc_prefixes: &[u32; 4096] = dc.wide.as_slice().try_into().ok()?;
    let ac_prefixes: &[u32; 4096] = ac.wide.as_slice().try_into().ok()?;
    let entry = dc_prefixes[window.peek(12) as usize];
    let difference = if entry & 0x8000 != 0 {
        window.consume(entry >> 8 & 31);
        entry.cast_signed() >> 16
    } else {
        let [symbol, length, ..] = entry.to_le_bytes();
        let code = (entry != 0).then_some((symbol, u32::from(length)));
        window.token(dc, code, false)?.1
    };
    *predictor = predictor
        .checked_add(difference)
        .filter(|&v| (-2048..2048).contains(&v))?;
    output[0] = *predictor * i32::from(quant[0]);
    let constant = u64::from(output[0].unsigned_abs());
    let mut l1 = constant;
    output[1..].fill(0);
    // Stores the coefficient at zigzag position `k`.
    let mut store = |k: usize, value: i32| {
        // Baseline magnitudes have at most 11 bits and quantizers 8.
        let product = value.wrapping_mul(i32::from(quant[k]));
        l1 = l1.wrapping_add(u64::from(product.unsigned_abs()));
        output[crate::dct::transform::COLUMN_ORDER[k]] = product;
    };
    let mut k = 1;
    while k < 64 {
        if window.valid < 12 {
            window.refill();
        }
        let entry = Ac(ac_prefixes[window.peek(12) as usize]);
        // One or two coefficients, unless the block ends before the last.
        // A single one stores a zero second value at its own position first.
        let last = k + entry.last() as usize;
        if entry.length() != 0 && last < 64 {
            window.consume(entry.length());
            store(last, entry.second());
            store(k + entry.run() as usize, entry.value());
            k = last + 1;
            continue;
        }
        // Any other code, or coefficients decoded one at a time.
        let (symbol, value) = window.token(ac, entry.fallback(), true)?;
        if symbol == 0 {
            break;
        }
        if symbol == 0xf0 {
            k += 16;
            if k > 64 {
                return None;
            }
            continue;
        }
        if !(1..=10).contains(&(symbol & 15)) {
            return None;
        }
        k += usize::from(symbol >> 4);
        if k >= 64 {
            return None;
        }
        store(k, value);
        k += 1;
    }
    // Quantizers are nonzero, so any nonzero AC coefficient adds to `l1`.
    Some(((l1 != constant, l1), window.bit))
}

/// Charges selectively repeated reference arithmetic against the JPEG work cap.
pub(super) fn charge(bits: &mut Bits<'_>, count: usize) -> Result<()> {
    bits.stream.work = bits
        .stream
        .work
        .checked_add(count)
        .filter(|&v| v <= bits.stream.work_cap)
        .ok_or(Error::limit("JPEG refinement work exceeded"))?;
    Ok(())
}

/// Consumes the already validated zero-difference DC plus EOB prefix.
#[expect(
    clippy::inline_always,
    reason = "measured fixed-size entropy prefix operations must fold into the block consumer"
)]
#[inline(always)]
pub(super) fn native_zero(bits: &mut Bits<'_>, shortcut: &[u32]) -> Result<bool> {
    if !bits.stream.loaded {
        load(bits)?;
    }
    let stream = &mut bits.stream;
    // The position never passes the segment's end.
    if stream.length.wrapping_mul(8).wrapping_sub(stream.bit) < 12 {
        return Ok(false);
    }
    let entry = shortcut[peek(stream, 12) as usize];
    if entry >> 16 == 0 && entry & 255 != 0 {
        stream.bit += (entry & 255) as usize;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Consumes the `length` bits `pattern`, 1 ≤ length ≤ 57, if they come next
/// within the segment.
pub(super) fn native_pattern(bits: &mut Bits<'_>, pattern: u64, length: u32) -> Result<bool> {
    if !bits.stream.loaded {
        load(bits)?;
    }
    let stream = &mut bits.stream;
    let length = length as usize;
    // The position never passes the segment's end.
    if stream.length.wrapping_mul(8).wrapping_sub(stream.bit) < length {
        return Ok(false);
    }
    let at = stream.bit / 8;
    let word = stream.bytes[at..]
        .first_chunk::<8>()
        .map_or(0, |bytes| u64::from_be_bytes(*bytes));
    if (word << (stream.bit % 8)) >> (64 - length) == pattern {
        stream.bit += length;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;
    #[test]
    fn entropy_materialization_obeys_work_and_storage_limits() {
        let data = vec![1; 4096];
        let mut bits = Bits::new(&data, 0);
        bits.limit_stream(8192, 4);
        assert_eq!(load(&mut bits).unwrap_err().kind, ErrorKind::Limit);
        let mut bits = Bits::new(&data, 0);
        bits.limit_stream(4, 8192);
        assert_eq!(load(&mut bits).unwrap_err().kind, ErrorKind::Limit);
    }
    #[test]
    fn stuffed_byte_positions_and_padding_are_preserved() {
        let mut bits = Bits::new(&[255, 0, 1, 255, 217], 0);
        bits.limit_stream(64, 64);
        load(&mut bits).unwrap();
        bits.stream.bit = 9;
        sync(&mut bits);
        assert_eq!((bits.next, bits.count, bits.window >> 57), (3, 7, 1));
        assert_eq!(bits.stream.bytes[..2], [255, 1]);
        assert_eq!(bits.stream_work(), 6);
    }
    #[test]
    fn wide_entropy_runs_keep_stuffed_bytes_at_every_alignment() {
        for length in 0..=33 {
            let mut bytes = vec![17; length];
            bytes.extend_from_slice(&[255, 0, 22, 255, 217]);
            let mut bits = Bits::new(&bytes, 0);
            bits.limit_stream(4096, 4096);
            load(&mut bits).unwrap();
            let mut expected = vec![17; length];
            expected.extend_from_slice(&[255, 22]);
            assert_eq!(&bits.stream.bytes[..bits.stream.length], expected);
            assert_eq!(bits.stream.stuffed, [length]);
            bits.stream.bit = (length + 2) * 8;
            assert_eq!(position(&bits), length + 3);
            assert_eq!(bits.stream_work(), 2 * (length + 3));
        }
    }
}
