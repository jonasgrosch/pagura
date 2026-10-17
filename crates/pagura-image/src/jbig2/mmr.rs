//! T.6 two-dimensional coding for generic regions (T.88 clause 6.2.6).
use super::Bitmap;
use pagura_core::{Error, Limits, Position, Result};

// Run-length codewords from the T.4 white/black terminating and makeup
// tables used by T.6. A word's index is the terminating run length or the
// multiple of 64 for makeup codes.
const WHITE: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100",
    "00111", "01000", "001000", "000011", "110100", "110101", "101010", "101011", "0100111",
    "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011",
    "0100100", "0011000", "00000010", "00000011", "00011010", "00011011", "00010010", "00010011",
    "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011",
    "00101100", "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011",
    "01010100", "01010101", "00100100", "00100101", "01011000", "01011001", "01011010", "01011011",
    "01001010", "01001011", "00110010", "00110011", "00110100",
];
const BLACK: [&str; 64] = [
    "0000110111",
    "010",
    "11",
    "10",
    "011",
    "0011",
    "0010",
    "00011",
    "000101",
    "000100",
    "0000100",
    "0000101",
    "0000111",
    "00000100",
    "00000111",
    "000011000",
    "0000010111",
    "0000011000",
    "0000001000",
    "00001100111",
    "00001101000",
    "00001101100",
    "00000110111",
    "00000101000",
    "00000010111",
    "00000011000",
    "000011001010",
    "000011001011",
    "000011001100",
    "000011001101",
    "000001101000",
    "000001101001",
    "000001101010",
    "000001101011",
    "000011010010",
    "000011010011",
    "000011010100",
    "000011010101",
    "000011010110",
    "000011010111",
    "000001101100",
    "000001101101",
    "000011011010",
    "000011011011",
    "000001010100",
    "000001010101",
    "000001010110",
    "000001010111",
    "000001100100",
    "000001100101",
    "000001010010",
    "000001010011",
    "000000100100",
    "000000110111",
    "000000111000",
    "000000100111",
    "000000101000",
    "000001011000",
    "000001011001",
    "000000101011",
    "000000101100",
    "000001011010",
    "000001100110",
    "000001100111",
];
const WHITE_MAKEUP: [&str; 27] = [
    "11011",
    "10010",
    "010111",
    "0110111",
    "00110110",
    "00110111",
    "01100100",
    "01100101",
    "01101000",
    "01100111",
    "011001100",
    "011001101",
    "011010010",
    "011010011",
    "011010100",
    "011010101",
    "011010110",
    "011010111",
    "011011000",
    "011011001",
    "011011010",
    "011011011",
    "010011000",
    "010011001",
    "010011010",
    "011000",
    "010011011",
];
const BLACK_MAKEUP: [&str; 27] = [
    "0000001111",
    "000011001000",
    "000011001001",
    "000001011011",
    "000000110011",
    "000000110100",
    "000000110101",
    "0000001101100",
    "0000001101101",
    "0000001001010",
    "0000001001011",
    "0000001001100",
    "0000001001101",
    "0000001110010",
    "0000001110011",
    "0000001110100",
    "0000001110101",
    "0000001110110",
    "0000001110111",
    "0000001010010",
    "0000001010011",
    "0000001010100",
    "0000001010101",
    "0000001011010",
    "0000001011011",
    "0000001100100",
    "0000001100101",
];
const COMMON_MAKEUP: [&str; 13] = [
    "00000001000",
    "00000001100",
    "00000001101",
    "000000010010",
    "000000010011",
    "000000010100",
    "000000010101",
    "000000010110",
    "000000010111",
    "000000011100",
    "000000011101",
    "000000011110",
    "000000011111",
];

#[derive(Clone, Copy)]
struct Node {
    children: [u16; 2],
    run: u16,
    terminal: bool,
}
const EMPTY: Node = Node {
    children: [0; 2],
    run: u16::MAX,
    terminal: false,
};

// Compile the normative codewords into prefix trees once. Decoding visits
// at most thirteen nodes per run; it never searches all codewords.
const fn insert(tree: &mut [Node; 512], used: &mut u16, word: &str, run: u16, terminal: bool) {
    let mut node = 0usize;
    let mut bit = 0;
    while bit < word.len() {
        let branch = if word.as_bytes()[bit] == b'1' { 1 } else { 0 };
        if tree[node].children[branch] == 0 {
            *used += 1;
            tree[node].children[branch] = *used;
        }
        node = tree[node].children[branch] as usize;
        bit += 1;
    }
    tree[node].run = run;
    tree[node].terminal = terminal;
}

const fn tree(black: bool) -> [Node; 512] {
    let mut result = [EMPTY; 512];
    let mut used = 0;
    let terminating = if black { &BLACK } else { &WHITE };
    let makeup = if black { &BLACK_MAKEUP } else { &WHITE_MAKEUP };
    let mut index = 0;
    let mut run = 0;
    while index < terminating.len() {
        insert(&mut result, &mut used, terminating[index], run, true);
        index += 1;
        run += 1;
    }
    index = 0;
    run = 64;
    while index < makeup.len() {
        insert(&mut result, &mut used, makeup[index], run, false);
        index += 1;
        run += 64;
    }
    index = 0;
    run = 1792;
    while index < COMMON_MAKEUP.len() {
        insert(&mut result, &mut used, COMMON_MAKEUP[index], run, false);
        index += 1;
        run += 64;
    }
    result
}
const WHITE_TREE: [Node; 512] = tree(false);
const BLACK_TREE: [Node; 512] = tree(true);

struct Bits<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
    base: usize,
}
impl Bits<'_> {
    fn next(&mut self) -> Result<u8> {
        let byte = self.data.get(self.byte).ok_or_else(|| {
            Error::syntax("truncated JBIG2 MMR data").at(Position::Buffer(self.base + self.byte))
        })?;
        let value = (byte >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        Ok(value)
    }

    fn run(&mut self, black: bool, remaining: u32) -> Result<u32> {
        let mut total: u32 = 0;
        loop {
            let (run, terminal) = self.code(black)?;
            total = total
                .checked_add(run)
                .filter(|&n| n <= remaining)
                .ok_or_else(|| Error::syntax("JBIG2 MMR run exceeds row"))?;
            if terminal {
                return Ok(total);
            }
        }
    }

    fn code(&mut self, black: bool) -> Result<(u32, bool)> {
        let tree = if black { &BLACK_TREE } else { &WHITE_TREE };
        let mut node = 0;
        for _ in 0..13 {
            node = usize::from(tree[node].children[usize::from(self.next()?)]);
            if node == 0 {
                break;
            }
            if tree[node].run != u16::MAX {
                return Ok((u32::from(tree[node].run), tree[node].terminal));
            }
        }
        Err(Error::syntax("invalid JBIG2 MMR run code"))
    }

    fn mode(&mut self) -> Result<Mode> {
        let mut word = 0;
        for length in 1..=7 {
            word = (word << 1) | self.next()?;
            match (length, word) {
                (1, 1) => return Ok(Mode::Vertical(0)),
                (3, 1) => return Ok(Mode::Horizontal),
                (3, 2) => return Ok(Mode::Vertical(-1)),
                (3, 3) => return Ok(Mode::Vertical(1)),
                (4, 1) => return Ok(Mode::Pass),
                (6, 2) => return Ok(Mode::Vertical(-2)),
                (6, 3) => return Ok(Mode::Vertical(2)),
                (7, 2) => return Ok(Mode::Vertical(-3)),
                (7, 3) => return Ok(Mode::Vertical(3)),
                (7, 1) => {
                    return Err(Error::syntax("T.88 forbids T.6 extension codes"));
                }
                _ => {}
            }
        }
        Err(Error::syntax("invalid JBIG2 MMR mode"))
    }

    fn finish(&mut self, required: bool) -> Result<()> {
        let remaining = (self.data.len() - self.byte) * 8 - usize::from(self.bit);
        if required || remaining > 7 {
            for _ in 0..2 {
                for _ in 0..11 {
                    if self.next()? != 0 {
                        return Err(Error::syntax("invalid JBIG2 MMR EOFB"));
                    }
                }
                if self.next()? != 1 {
                    return Err(Error::syntax("invalid JBIG2 MMR EOFB"));
                }
            }
        }
        while self.bit != 0 {
            self.next()?;
        }
        Ok(())
    }
}

enum Mode {
    Pass,
    Horizontal,
    Vertical(i32),
}

#[derive(Default)]
struct ReferenceCursor {
    position: u32,
    pair: Option<(u32, u32)>,
}

fn reference(
    bitmap: &Bitmap,
    y: u32,
    start: u32,
    black: bool,
    first: bool,
    cursor: &mut ReferenceCursor,
) -> (u32, u32) {
    let mut b1 = bitmap.width;
    while cursor.position < bitmap.width {
        let x = cursor.position;
        let before = bitmap.pixel(i64::from(x) - 1, i64::from(y) - 1);
        let after = bitmap.pixel(i64::from(x), i64::from(y) - 1);
        if (x > start || (first && x == 0)) && before != after && after != u8::from(black) {
            b1 = x;
            break;
        }
        cursor.position += 1;
    }
    // Horizontal codes before b1 may reuse this pair many times. Cache b2
    // so a long reference run is scanned once, rather than once per code.
    if let Some(pair) = cursor.pair
        && pair.0 == b1
    {
        return pair;
    }
    let mut b2 = b1;
    if b2 < bitmap.width {
        b2 += 1;
        while b2 < bitmap.width
            && bitmap.pixel(i64::from(b2), i64::from(y) - 1)
                == bitmap.pixel(i64::from(b1), i64::from(y) - 1)
        {
            b2 += 1;
        }
    }
    cursor.pair = Some((b1, b2));
    (b1, b2)
}

fn paint(bitmap: &mut Bitmap, y: u32, start: u32, end: u32, black: bool) {
    if black {
        for x in start..end {
            bitmap.set(x, y, 1);
        }
    }
}

pub(super) fn decode(
    data: &[u8],
    width: u32,
    height: u32,
    limits: &Limits,
    base: usize,
) -> Result<Bitmap> {
    let (bitmap, consumed) = decode_prefix(data, width, height, limits, base, false)?;
    if consumed != data.len() {
        return Err(Error::syntax("trailing JBIG2 MMR data"));
    }
    Ok(bitmap)
}

/// A gray-scale bitplane has an obligatory EOFB and consumes whole bytes (6.2.6).
pub(super) fn decode_prefix(
    data: &[u8],
    width: u32,
    height: u32,
    limits: &Limits,
    base: usize,
    required: bool,
) -> Result<(Bitmap, usize)> {
    let mut bits = Bits {
        data,
        byte: 0,
        bit: 0,
        base,
    };
    let mut bitmap = Bitmap::new(width, height, false, limits)?;
    for y in 0..height {
        let mut x = 0;
        let mut black = false;
        let mut first = true;
        let mut cursor = ReferenceCursor::default();
        while x < width {
            let (b1, b2) = reference(&bitmap, y, x, black, first, &mut cursor);
            let end = match bits.mode()? {
                Mode::Pass => {
                    paint(&mut bitmap, y, x, b2, black);
                    b2
                }
                Mode::Horizontal => {
                    let a1 = x + bits.run(black, width - x)?;
                    let a2 = a1 + bits.run(!black, width - a1)?;
                    if a2 == x {
                        return Err(Error::syntax("nonprogressing JBIG2 MMR horizontal mode"));
                    }
                    paint(&mut bitmap, y, x, a1, black);
                    paint(&mut bitmap, y, a1, a2, !black);
                    a2
                }
                Mode::Vertical(delta) => {
                    let end = u32::try_from(i64::from(b1) + i64::from(delta))
                        .ok()
                        .filter(|&end| end >= x && end <= width)
                        .ok_or_else(|| Error::syntax("JBIG2 MMR vertical mode exceeds row"))?;
                    if end == x && !first {
                        return Err(Error::syntax("nonprogressing JBIG2 MMR vertical mode"));
                    }
                    paint(&mut bitmap, y, x, end, black);
                    black = !black;
                    end
                }
            };
            first = false;
            x = end;
        }
    }
    bits.finish(required)?;
    Ok((bitmap, bits.byte))
}
