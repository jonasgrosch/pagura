//! Twelve-bit canonical Huffman prefixes over a bounded, unstuffed copy of
//! one entropy-coded segment (T.81 C.2, F.2.2).
//!
//! The segment up to its next marker is copied once without its stuffed
//! zero bytes, charging two operations per input byte and its storage.
//! Positions are mapped back to the stuffed data for errors and alignment.
use super::{Bits, Huffman};
use pagura_core::{Error, Result};

#[derive(Default)]
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
    let start = bits.offset;
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
        return bits.offset;
    }
    let used = bits.stream.bit.div_ceil(8);
    bits.stream.start + used + bits.stream.stuffed.partition_point(|&p| p < used)
}

pub(crate) fn sync(bits: &mut Bits<'_>) {
    if !bits.stream.loaded {
        return;
    }
    bits.offset = position(bits);
    let used = bits.stream.bit.div_ceil(8);
    bits.left = u8::try_from((8 - bits.stream.bit % 8) % 8).unwrap_or(0);
    bits.byte = if used == 0 {
        0
    } else {
        bits.stream.bytes[used - 1]
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

pub(super) fn wide_table(
    first: &[u32; 16],
    count: &[u8; 16],
    offsets: &[usize; 16],
    symbols: &[u8],
    ac: bool,
) -> [u32; 4096] {
    let mut table = [0u32; 4096];
    for length in 1..=12usize {
        for i in 0..usize::from(count[length - 1]) {
            let code = (first[length - 1] as usize + i) << (12 - length);
            let symbol = symbols[offsets[length - 1] + i];
            let size = usize::from(if ac { symbol & 15 } else { symbol });
            for (suffix, entry) in table[code..code + (1 << (12 - length))]
                .iter_mut()
                .enumerate()
            {
                *entry = u32::from(symbol) | (u32::try_from(length).unwrap_or(12) << 8);
                if size <= if ac { 10 } else { 11 } && length + size <= 12 {
                    let magnitude = (suffix >> (12 - length - size)) & ((1 << size) - 1);
                    let value = i32::try_from(magnitude).unwrap_or(0);
                    let value = if size > 0 && value < (1 << (size - 1)) {
                        value + 1 - (1 << size)
                    } else {
                        value
                    };
                    let value = i16::try_from(value).unwrap_or(0);
                    *entry = u32::from(symbol)
                        | 0x8000
                        | (u32::try_from(length + size).unwrap_or(12) << 8)
                        | (u32::from(u16::from_ne_bytes(value.to_ne_bytes())) << 16);
                }
            }
        }
    }
    table
}

#[derive(Clone, Copy)]
struct Token {
    symbol: u8,
    value: i16,
    pending: u8,
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
        let following = ac.wide[(prefix << used) & 4095];
        let length = ((following >> 8) & 31) as usize;
        if following & 0x80ff != 0x8000 || length + used > 12 {
            return 0;
        }
        (entry & 0xffff_0000) | u32::try_from(used + length).unwrap_or(0)
    }))
}

/// Decodes directly to one dequantized block, avoiding retained coefficient planes.
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
    native_inner(dc, ac, &mut bits.stream, predictor, output, quant)
        .ok_or_else(|| bits.error("invalid JPEG native entropy"))
}
/// A 64-bit window on the unstuffed segment at bit `bit`, refilled with one
/// load. The segment's eight padding bytes make every load in bounds while
/// `bit` is within it; reads past its end are rejected by `limit`.
struct Window<'a> {
    bytes: &'a [u8],
    bit: usize,
    limit: usize,
    word: u64,
    valid: u32,
}

impl<'a> Window<'a> {
    fn new(stream: &'a Stream) -> Self {
        let mut window = Self {
            bytes: &stream.bytes,
            bit: stream.bit,
            limit: stream.length * 8,
            word: 0,
            valid: 0,
        };
        window.refill();
        window
    }

    /// Holds at least 57 bits from `bit` on.
    #[expect(
        clippy::inline_always,
        reason = "one load serves several entropy tokens"
    )]
    #[inline(always)]
    fn refill(&mut self) {
        let at = self.bit / 8;
        let word = self.bytes[at..at + 8]
            .first_chunk::<8>()
            .map_or(0, |bytes| u64::from_be_bytes(*bytes));
        let skip = u32::from(self.bit.to_le_bytes()[0] & 7);
        self.word = word << skip;
        self.valid = 64 - skip;
    }

    fn available(&self) -> usize {
        self.limit - self.bit
    }

    /// The next `count` bits, 1 ≤ count ≤ `valid`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most 32 bits are requested"
    )]
    fn peek(&self, count: u32) -> u32 {
        (self.word >> (64 - count)) as u32
    }

    fn consume(&mut self, count: u32) {
        self.bit += count as usize;
        self.word <<= count;
        self.valid -= count;
    }

    /// Reads `count` ≤ 24 bits, or None past the segment.
    fn read(&mut self, count: u32) -> Option<u32> {
        if count == 0 {
            return Some(0);
        }
        if count as usize > self.available() {
            return None;
        }
        if self.valid < count {
            self.refill();
        }
        let value = self.peek(count);
        self.consume(count);
        Some(value)
    }

    /// The next Huffman token. Afterwards at least eleven bits of a pending
    /// magnitude remain in the window.
    #[expect(
        clippy::inline_always,
        reason = "token return overhead dominates the per-coefficient loop"
    )]
    #[inline(always)]
    fn token(&mut self, table: &Huffman) -> Option<Token> {
        if self.valid < 24 {
            self.refill();
        }
        let (mut code, mut start) = (0u32, 0usize);
        if self.available() >= 12 {
            code = self.peek(12);
            let entry = table.wide[code as usize];
            if entry != 0 {
                self.consume((entry >> 8) & 31);
                let symbol = entry.to_le_bytes()[0];
                let pending = (symbol & 15) & 0u8.wrapping_sub(u8::from(entry & 0x8000 == 0));
                return Some(Token {
                    symbol,
                    value: i16::from_ne_bytes(u16::try_from(entry >> 16).ok()?.to_ne_bytes()),
                    pending,
                });
            }
            self.consume(12);
            start = 12;
        }
        for length in start..16 {
            code = code * 2 + self.read(1)?;
            if let Some(relative) = code
                .checked_sub(table.first[length])
                .filter(|&v| v < u32::from(table.count[length]))
            {
                let symbol = table.symbols[table.offset[length] + relative as usize];
                if self.valid < 11 {
                    self.refill();
                }
                return Some(Token {
                    symbol,
                    value: 0,
                    pending: symbol & 15,
                });
            }
        }
        None
    }

    /// Adds a token's pending magnitude bits (T.81 F.2.2.1) to its value.
    #[expect(
        clippy::inline_always,
        reason = "magnitude extraction is on every nonzero coefficient"
    )]
    #[inline(always)]
    fn magnitude(&mut self, token: Token) -> Option<i32> {
        let size = u32::from(token.pending);
        if size as usize > self.available() {
            return None;
        }
        // Pending magnitudes have at most eleven bits, which `token` leaves in
        // the window. Reading one bit even for none removes a branch.
        let raw = self.peek(size.max(1)) & 0u32.wrapping_sub(u32::from(size != 0));
        self.consume(size);
        let negative = 0i32.wrapping_sub(i32::from(raw < ((1u32 << size) >> 1)));
        let correction = negative & (1i32 - (1i32 << size));
        Some(i32::from(token.value).wrapping_add(i32::try_from(raw).ok()?.wrapping_add(correction)))
    }
}

fn native_inner(
    dc: &Huffman,
    ac: &Huffman,
    stream: &mut Stream,
    predictor: &mut i32,
    output: &mut [i32; 64],
    quant: &[u16; 64],
) -> Option<(bool, u64)> {
    let mut window = Window::new(stream);
    let result = block(dc, ac, &mut window, predictor, output, quant);
    stream.bit = window.bit;
    result
}

fn block(
    dc: &Huffman,
    ac: &Huffman,
    window: &mut Window<'_>,
    predictor: &mut i32,
    output: &mut [i32; 64],
    quant: &[u16; 64],
) -> Option<(bool, u64)> {
    let token = window.token(dc)?;
    if token.symbol > 11 {
        return None;
    }
    *predictor = predictor
        .checked_add(window.magnitude(token)?)
        .filter(|&v| (-2048..2048).contains(&v))?;
    output[0] = *predictor * i32::from(quant[0]);
    let mut l1 = u64::from(output[0].unsigned_abs());
    let mut nonconstant = false;
    let mut k = 1;
    while k < 64 {
        let token = window.token(ac)?;
        if token.symbol == 0 {
            break;
        }
        if token.symbol == 0xf0 {
            k += 16;
            if k > 64 {
                return None;
            }
            continue;
        }
        let size = token.symbol & 15;
        if size == 0 || size > 10 {
            return None;
        }
        k += usize::from(token.symbol >> 4);
        if k >= 64 {
            return None;
        }
        let value = window.magnitude(token)?;
        if !nonconstant {
            output[1..].fill(0);
            nonconstant = true;
        }
        let product = value * i32::from(quant[k]);
        l1 = l1.wrapping_add(u64::from(product.unsigned_abs()));
        output[crate::dct::transform::ORDER[k]] = product;
        k += 1;
    }
    Some((nonconstant, l1))
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
    if stream.length * 8 - stream.bit < 12 {
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
        assert_eq!((bits.offset, bits.byte, bits.left), (3, 1, 7));
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
