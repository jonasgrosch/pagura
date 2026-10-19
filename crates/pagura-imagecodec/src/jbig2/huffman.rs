//! Canonical Huffman coding (T.88 Annex B).
use crate::budget::Budget;
use pagura_core::bytes::{BitCursor, Cursor};
use pagura_core::convert::usize_to_u64;
use pagura_core::{Error, Result};
use std::rc::Rc;

// Conservatively charges the entire standard-table catalogue before segment
// setup, including tables used by refinement/aggregation. This also bounds a
// stream consisting of many empty Huffman segments.
pub(super) const STANDARD_ROWS: usize = 184;

/// The rest of a segment's data as Huffman-coded bits, positioned in the
/// JBIG2 input.
pub(super) fn bits<'a>(cursor: &mut Cursor<'a>) -> Result<BitCursor<'a>> {
    let remaining = cursor.remaining();
    cursor.take_cursor(remaining).map(BitCursor::new)
}

/// The next `count` whole bytes after skipping to a byte boundary, for
/// embedded MMR, arithmetic or uncompressed data.
pub(super) fn bytes<'a>(bits: &mut BitCursor<'a>, count: usize) -> Result<Cursor<'a>> {
    bits.align()?;
    bits.byte_cursor()?.take_cursor(count)
}

/// Requires that no whole byte follows the Huffman-coded data.
pub(super) fn finish(bits: &mut BitCursor<'_>) -> Result<()> {
    bits.align()?;
    let rest = bits.byte_cursor()?;
    if rest.at_end() {
        Ok(())
    } else {
        Err(rest.error("trailing JBIG2 Huffman data"))
    }
}

/// Reads a field of `count` bits, at most 8.
pub(super) fn small(bits: &mut BitCursor<'_>, count: u8) -> Result<u8> {
    let at = bits.position();
    u8::try_from(bits.bits(u32::from(count))?)
        .map_err(|_| Error::syntax("oversized JBIG2 bit field").at(at))
}

/// What the values of a table line count from (T.88 B.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineKind {
    /// Up from the line's base: a range line or the upper range line.
    Upper,
    /// Down from the line's base: the lower range line.
    Lower,
    /// The line codes the out-of-band value, without a range.
    OutOfBand,
}

/// A table line (T.88 B.1): the length of its prefix (zero for an unused
/// line), the bit length of its range and the base of its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) prefix: u8,
    pub(super) range: u8,
    pub(super) base: i64,
    pub(super) kind: LineKind,
}

/// The longest prefix and range length a line may have.
const MAX_LENGTH: u8 = 32;

/// A table's lines sorted by prefix length, with constant-time canonical
/// lookup at each length (T.88 B.3).
pub(super) struct Table {
    first: [u64; 33],
    count: [usize; 33],
    start: [usize; 33],
    lines: Vec<Line>,
}
impl Table {
    /// Builds a table from its lines, charging its memory.
    pub(super) fn new(lines: &[Line], budget: &mut Budget) -> Result<Self> {
        budget.memory::<Self>(1)?;
        let used = lines.iter().filter(|line| line.prefix != 0).count();
        let mut sorted = Vec::new();
        budget.reserve(&mut sorted, used)?;
        let mut table = Self {
            first: [0; 33],
            count: [0; 33],
            start: [0; 33],
            lines: sorted,
        };
        for line in lines {
            if line.prefix > MAX_LENGTH || line.range > MAX_LENGTH {
                return Err(Error::syntax("invalid JBIG2 Huffman table row"));
            }
            if line.prefix != 0 {
                table.count[usize::from(line.prefix)] += 1;
            }
        }
        for length in 1..=usize::from(MAX_LENGTH) {
            table.first[length] =
                (table.first[length - 1] + usize_to_u64(table.count[length - 1])) << 1;
            if table.first[length] + usize_to_u64(table.count[length]) > 1 << length {
                return Err(Error::syntax("oversubscribed JBIG2 Huffman table"));
            }
            table.start[length] = table.lines.len();
            for line in lines {
                if usize::from(line.prefix) == length {
                    table.lines.push(*line);
                }
            }
        }
        if table.lines.is_empty() {
            return Err(Error::syntax("empty JBIG2 Huffman table"));
        }
        Ok(table)
    }
    pub(super) fn standard(number: usize, budget: &mut Budget) -> Result<Self> {
        Self::new(STANDARD[number - 1], budget)
    }

    /// Drops a table, releasing its memory.
    pub(super) fn release(self, budget: &mut Budget) {
        budget.release::<Self>(1);
        budget.release::<Line>(self.lines.len());
    }
    pub(super) fn decode(&self, bits: &mut BitCursor<'_>) -> Result<Option<i64>> {
        let mut code = 0u64;
        for length in 1..=32 {
            code = (code << 1) | u64::from(bits.bit()?);
            if let Some(index) = code
                .checked_sub(self.first[length])
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n < self.count[length])
            {
                let line = self.lines[self.start[length] + index];
                let offset = i64::from(bits.bits(u32::from(line.range))?);
                let value = match line.kind {
                    LineKind::OutOfBand => return Ok(None),
                    LineKind::Lower => line.base - offset,
                    LineKind::Upper => line.base + offset,
                };
                if i32::try_from(value).is_err() {
                    return Err(
                        Error::syntax("JBIG2 Huffman integer outside signed 32-bit range")
                            .at(bits.position()),
                    );
                }
                return Ok(Some(value));
            }
        }
        Err(Error::syntax("invalid JBIG2 Huffman prefix").at(bits.position()))
    }
    pub(super) fn required(&self, bits: &mut BitCursor<'_>) -> Result<i64> {
        self.decode(bits)?
            .ok_or_else(|| Error::syntax("unexpected JBIG2 Huffman OOB").at(bits.position()))
    }
    pub(super) fn has_oob(&self) -> bool {
        self.lines
            .iter()
            .any(|line| line.kind == LineKind::OutOfBand)
    }
}

/// Drops a shared table, releasing its memory if this was its last
/// reference: a table built for one segment, not a code table segment's.
pub(super) fn unshare(table: Rc<Table>, budget: &mut Budget) {
    if let Ok(table) = Rc::try_unwrap(table) {
        table.release(budget);
    }
}

/// Whether a selected table must code the out-of-band value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Oob {
    With,
    Without,
}

/// Picks the tables a segment's flags select: standard tables, or the user
/// tables of the code table segments it refers to, in order (T.88
/// 7.4.2.1.6, 7.4.3.1.6).
pub(super) struct Selector<'t, 'b> {
    custom: std::slice::Iter<'t, Rc<Table>>,
    budget: &'b mut Budget,
}

impl<'t, 'b> Selector<'t, 'b> {
    /// The selection of the next user table.
    const USER: u16 = 3;

    pub(super) fn new(custom: &'t [Rc<Table>], budget: &'b mut Budget) -> Self {
        Self {
            custom: custom.iter(),
            budget,
        }
    }

    /// The table that the two-bit field `selection` picks: standard table
    /// B.`standards[selection]`, or the next user table.
    pub(super) fn select(
        &mut self,
        selection: u16,
        standards: &[usize],
        oob: Oob,
    ) -> Result<Rc<Table>> {
        let table = if let Some(&number) = standards.get(usize::from(selection)) {
            Rc::new(Table::standard(number, self.budget)?)
        } else if selection == Self::USER {
            self.custom
                .next()
                .cloned()
                .ok_or_else(|| Error::syntax("missing JBIG2 user Huffman table"))?
        } else {
            return Err(Error::syntax("reserved JBIG2 Huffman table selection"));
        };
        if table.has_oob() != (oob == Oob::With) {
            return Err(Error::syntax(
                "incorrect JBIG2 Huffman table OOB capability",
            ));
        }
        Ok(table)
    }

    /// The selection that a one-bit table field makes: the first standard
    /// table, or the next user table if the bit is set.
    pub(super) const fn one_bit(user: bool) -> u16 {
        if user { Self::USER } else { 0 }
    }

    /// Requires that every user table referred to was selected.
    pub(super) fn finish(mut self) -> Result<()> {
        if self.custom.next().is_some() {
            return Err(Error::syntax("unused JBIG2 user Huffman table"));
        }
        Ok(())
    }
}

/// Code table segment flags (T.88 B.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TableFlags(u8);

impl TableFlags {
    /// Bit 0, HTOOB: the table codes the out-of-band value.
    const OOB: u8 = 0x01;
    /// Bits 1–3, HTPS − 1: the width of the prefix length fields.
    const PREFIX_SHIFT: u32 = 1;
    /// Bits 4–6, HTRS − 1: the width of the range length fields.
    const RANGE_SHIFT: u32 = 4;
    /// Bit 7: reserved.
    const RESERVED: u8 = 0x80;

    const fn prefix_bits(self) -> u8 {
        ((self.0 >> Self::PREFIX_SHIFT) & 7) + 1
    }

    const fn range_bits(self) -> u8 {
        ((self.0 >> Self::RANGE_SHIFT) & 7) + 1
    }
}

/// A code table segment's user table (T.88 7.4.13, B.2–B.3).
pub(super) fn custom(cursor: &mut Cursor<'_>, budget: &mut Budget) -> Result<Table> {
    let flags = TableFlags(cursor.u8()?);
    if flags.0 & TableFlags::RESERVED != 0 {
        return Err(Error::syntax("reserved JBIG2 Huffman table flag"));
    }
    let (prefix_bits, range_bits) = (flags.prefix_bits(), flags.range_bits());
    let low = i64::from(cursor.i32()?);
    let high = i64::from(cursor.i32()?);
    if low >= high {
        return Err(Error::syntax("invalid JBIG2 Huffman table bounds"));
    }
    let mut bits = bits(cursor)?;
    let mut lines = Vec::new();
    let mut current = low;
    while current < high {
        budget.work(1)?;
        let prefix = small(&mut bits, prefix_bits)?;
        let range = small(&mut bits, range_bits)?;
        if prefix > MAX_LENGTH || range > MAX_LENGTH {
            return Err(Error::syntax("invalid JBIG2 Huffman table lengths"));
        }
        budget.push(&mut lines, line(prefix, range, current))?;
        current += 1_i64 << range;
    }
    budget.push(&mut lines, lower(small(&mut bits, prefix_bits)?, low - 1))?;
    budget.push(&mut lines, line(small(&mut bits, prefix_bits)?, 32, high))?;
    if flags.0 & TableFlags::OOB != 0 {
        budget.push(&mut lines, oob(small(&mut bits, prefix_bits)?))?;
    }
    finish(&mut bits)?;
    let table = Table::new(&lines, budget)?;
    budget.release::<Line>(lines.capacity());
    Ok(table)
}
/// A range line or the upper range line.
pub(super) const fn line(prefix: u8, range: u8, base: i64) -> Line {
    Line {
        prefix,
        range,
        base,
        kind: LineKind::Upper,
    }
}

/// A lower range line, whose 32-bit offset counts down from `base`.
const fn lower(prefix: u8, base: i64) -> Line {
    Line {
        prefix,
        range: 32,
        base,
        kind: LineKind::Lower,
    }
}

/// The out-of-band line.
const fn oob(prefix: u8) -> Line {
    Line {
        prefix,
        range: 0,
        base: 0,
        kind: LineKind::OutOfBand,
    }
}

/// The standard tables, T.88 Tables B.1–B.15.
const STANDARD: [&[Line]; 15] = [
    &[
        line(1, 4, 0),
        line(2, 8, 16),
        line(3, 16, 272),
        line(3, 32, 65808),
    ],
    &[
        line(1, 0, 0),
        line(2, 0, 1),
        line(3, 0, 2),
        line(4, 3, 3),
        line(5, 6, 11),
        line(6, 32, 75),
        oob(6),
    ],
    &[
        line(8, 8, -256),
        line(1, 0, 0),
        line(2, 0, 1),
        line(3, 0, 2),
        line(4, 3, 3),
        line(5, 6, 11),
        lower(8, -257),
        line(7, 32, 75),
        oob(6),
    ],
    &[
        line(1, 0, 1),
        line(2, 0, 2),
        line(3, 0, 3),
        line(4, 3, 4),
        line(5, 6, 12),
        line(5, 32, 76),
    ],
    &[
        line(7, 8, -255),
        line(1, 0, 1),
        line(2, 0, 2),
        line(3, 0, 3),
        line(4, 3, 4),
        line(5, 6, 12),
        lower(7, -256),
        line(6, 32, 76),
    ],
    &[
        line(5, 10, -2048),
        line(4, 9, -1024),
        line(4, 8, -512),
        line(4, 7, -256),
        line(5, 6, -128),
        line(5, 5, -64),
        line(4, 5, -32),
        line(2, 7, 0),
        line(3, 7, 128),
        line(3, 8, 256),
        line(4, 9, 512),
        line(4, 10, 1024),
        lower(6, -2049),
        line(6, 32, 2048),
    ],
    &[
        line(4, 9, -1024),
        line(3, 8, -512),
        line(4, 7, -256),
        line(5, 6, -128),
        line(5, 5, -64),
        line(4, 5, -32),
        line(4, 5, 0),
        line(5, 5, 32),
        line(5, 6, 64),
        line(4, 7, 128),
        line(3, 8, 256),
        line(3, 9, 512),
        line(3, 10, 1024),
        lower(5, -1025),
        line(5, 32, 2048),
    ],
    &[
        line(8, 3, -15),
        line(9, 1, -7),
        line(8, 1, -5),
        line(9, 0, -3),
        line(7, 0, -2),
        line(4, 0, -1),
        line(2, 1, 0),
        line(5, 0, 2),
        line(6, 0, 3),
        line(3, 4, 4),
        line(6, 1, 20),
        line(4, 4, 22),
        line(4, 5, 38),
        line(5, 6, 70),
        line(5, 7, 134),
        line(6, 7, 262),
        line(7, 8, 390),
        line(6, 10, 646),
        lower(9, -16),
        line(9, 32, 1670),
        oob(2),
    ],
    &[
        line(8, 4, -31),
        line(9, 2, -15),
        line(8, 2, -11),
        line(9, 1, -7),
        line(7, 1, -5),
        line(4, 1, -3),
        line(3, 1, -1),
        line(3, 1, 1),
        line(5, 1, 3),
        line(6, 1, 5),
        line(3, 5, 7),
        line(6, 2, 39),
        line(4, 5, 43),
        line(4, 6, 75),
        line(5, 7, 139),
        line(5, 8, 267),
        line(6, 8, 523),
        line(7, 9, 779),
        line(6, 11, 1291),
        lower(9, -32),
        line(9, 32, 3339),
        oob(2),
    ],
    &[
        line(7, 4, -21),
        line(8, 0, -5),
        line(7, 0, -4),
        line(5, 0, -3),
        line(2, 2, -2),
        line(5, 0, 2),
        line(6, 0, 3),
        line(7, 0, 4),
        line(8, 0, 5),
        line(2, 6, 6),
        line(5, 5, 70),
        line(6, 5, 102),
        line(6, 6, 134),
        line(6, 7, 198),
        line(6, 8, 326),
        line(6, 9, 582),
        line(6, 10, 1094),
        line(7, 11, 2118),
        lower(8, -22),
        line(8, 32, 4166),
        oob(2),
    ],
    &[
        line(1, 0, 1),
        line(2, 1, 2),
        line(4, 0, 4),
        line(4, 1, 5),
        line(5, 1, 7),
        line(5, 2, 9),
        line(6, 2, 13),
        line(7, 2, 17),
        line(7, 3, 21),
        line(7, 4, 29),
        line(7, 5, 45),
        line(7, 6, 77),
        line(7, 32, 141),
    ],
    &[
        line(1, 0, 1),
        line(2, 0, 2),
        line(3, 1, 3),
        line(5, 0, 5),
        line(5, 1, 6),
        line(6, 1, 8),
        line(7, 0, 10),
        line(7, 1, 11),
        line(7, 2, 13),
        line(7, 3, 17),
        line(7, 4, 25),
        line(8, 5, 41),
        line(8, 32, 73),
    ],
    &[
        line(1, 0, 1),
        line(3, 0, 2),
        line(4, 0, 3),
        line(5, 0, 4),
        line(4, 1, 5),
        line(3, 3, 7),
        line(6, 1, 15),
        line(6, 2, 17),
        line(6, 3, 21),
        line(6, 4, 29),
        line(6, 5, 45),
        line(7, 6, 77),
        line(7, 32, 141),
    ],
    &[
        line(3, 0, -2),
        line(3, 0, -1),
        line(1, 0, 0),
        line(3, 0, 1),
        line(3, 0, 2),
    ],
    &[
        line(7, 4, -24),
        line(6, 2, -8),
        line(5, 1, -4),
        line(4, 0, -2),
        line(3, 0, -1),
        line(1, 0, 0),
        line(3, 0, 1),
        line(4, 0, 2),
        line(5, 1, 3),
        line(6, 2, 5),
        line(7, 4, 9),
        lower(7, -25),
        line(7, 32, 25),
    ],
];

#[cfg(test)]
mod tests {
    use super::Table;
    use crate::budget::Budget;
    use pagura_core::Limits;
    use pagura_core::bytes::{BitCursor, Cursor};
    use pagura_testkit::fixture;

    fn budget() -> Budget {
        Budget::new(&Limits::default(), crate::jbig2::MESSAGES)
    }

    fn walk(number: usize, expected_rows: usize) {
        let table = Table::standard(number, &mut budget()).unwrap();
        let data = fixture("jbig2", &format!("huffman-table-{number:02}.codes"));
        let mut seen = std::collections::BTreeSet::new();
        for line in std::str::from_utf8(&data).unwrap().lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            let index = fields[0].parse::<usize>().unwrap();
            seen.insert(index);
            let mut bytes = vec![0; fields[1].len().div_ceil(8)];
            for (n, b) in fields[1].bytes().enumerate() {
                bytes[n / 8] |= (b - b'0') << (7 - n % 8);
            }
            let expected = if fields[2] == "OOB" {
                None
            } else {
                Some(fields[2].parse::<i64>().unwrap())
            };
            assert_eq!(
                table
                    .decode(&mut BitCursor::new(Cursor::new(&bytes)))
                    .unwrap(),
                expected,
                "B.{number}, row {index}"
            );
        }
        assert_eq!(seen.len(), expected_rows, "B.{number}");
    }
    macro_rules! table_tests {($(($name:ident,$number:literal,$rows:literal)),*)=>{$(#[test] fn $name(){walk($number,$rows);})*};}
    table_tests!(
        (b1, 1, 4),
        (b2, 2, 7),
        (b3, 3, 9),
        (b4, 4, 6),
        (b5, 5, 8),
        (b6, 6, 14),
        (b7, 7, 15),
        (b8, 8, 21),
        (b9, 9, 22),
        (b10, 10, 21),
        (b11, 11, 13),
        (b12, 12, 13),
        (b13, 13, 13),
        (b14, 14, 5),
        (b15, 15, 13)
    );
    #[test]
    fn custom_table_known_answer_b4_example() {
        // The worked example of T.88 B.4 transmits standard table B.1.
        let data = [0x42, 0, 0, 0, 0, 0, 1, 1, 0x10, 0x49, 0x23, 0x81, 0x80];
        let custom = super::custom(&mut Cursor::new(&data), &mut budget()).unwrap();
        for (data, expected) in [
            (&[0u8][..], 0),
            (&[0x80, 0][..], 16),
            (&[0xc0, 0, 0][..], 272),
            (&[0xe0, 0, 0, 0, 0][..], 65808),
        ] {
            let mut bits = BitCursor::new(Cursor::new(data));
            assert_eq!(custom.required(&mut bits).unwrap(), expected);
        }
    }
}
