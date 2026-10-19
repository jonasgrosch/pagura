//! Canonical Huffman coding, T.88 (02/2000), Annex B.
use super::{budget::Work, reader::Reader};
use pagura_core::{Error, Limits, Position, Result};

// Conservatively charges the entire standard-table catalogue before segment
// setup, including tables used by refinement/aggregation. This also bounds a
// stream consisting of many empty Huffman segments.
pub(super) const STANDARD_ROWS: usize = 184;

pub(super) struct Bits<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
}
impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8], base: usize) -> Self {
        Self {
            data,
            offset: 0,
            base,
        }
    }
    pub(super) fn position(&self) -> Position {
        Position::Buffer(self.base + self.offset / 8)
    }
    pub(super) fn read(&mut self, count: u8) -> Result<u32> {
        if count > 32
            || usize::from(count)
                > self
                    .data
                    .len()
                    .saturating_mul(8)
                    .saturating_sub(self.offset)
        {
            return Err(Error::syntax("truncated JBIG2 Huffman field").at(self.position()));
        }
        let mut value = 0;
        for _ in 0..count {
            value =
                (value << 1) | u32::from((self.data[self.offset / 8] >> (7 - self.offset % 8)) & 1);
            self.offset += 1;
        }
        Ok(value)
    }
    pub(super) fn small(&mut self, count: u8) -> Result<u8> {
        u8::try_from(self.read(count)?)
            .map_err(|_| Error::syntax("oversized JBIG2 bit field").at(self.position()))
    }
    pub(super) fn align(&mut self) {
        self.offset = self.offset.div_ceil(8) * 8;
    }
    pub(super) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        self.align();
        let start = self.offset / 8;
        let end = start
            .checked_add(count)
            .filter(|&n| n <= self.data.len())
            .ok_or_else(|| Error::syntax("truncated JBIG2 Huffman payload").at(self.position()))?;
        self.offset = end * 8;
        Ok(&self.data[start..end])
    }
    pub(super) fn finish(&mut self) -> Result<()> {
        self.align();
        if self.offset / 8 != self.data.len() {
            return Err(Error::syntax("trailing JBIG2 Huffman data").at(self.position()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Row {
    bits: u8,
    base: i64,
    kind: u8,
}
/// Rows sorted by prefix length, with constant-time canonical lookup at each depth.
pub(super) struct Table {
    first: [u64; 33],
    count: [usize; 33],
    start: [usize; 33],
    rows: Vec<Row>,
}
impl Table {
    pub(super) fn new(parameters: &[(u8, u8, i64, u8)]) -> Result<Self> {
        let mut table = Self {
            first: [0; 33],
            count: [0; 33],
            start: [0; 33],
            rows: Vec::with_capacity(parameters.len()),
        };
        for &(length, bits, _, kind) in parameters {
            if length > 32 || bits > 32 || kind > 2 {
                return Err(Error::syntax("invalid JBIG2 Huffman table row"));
            }
            if length != 0 {
                table.count[usize::from(length)] += 1;
            }
        }
        for length in 1..=32 {
            table.first[length] = (table.first[length - 1] + table.count[length - 1] as u64) << 1;
            if table.first[length] + table.count[length] as u64 > 1u64 << length {
                return Err(Error::syntax("oversubscribed JBIG2 Huffman table"));
            }
            table.start[length] = table.rows.len();
            for &(n, bits, base, kind) in parameters {
                if usize::from(n) == length {
                    table.rows.push(Row { bits, base, kind });
                }
            }
        }
        if table.rows.is_empty() {
            return Err(Error::syntax("empty JBIG2 Huffman table"));
        }
        Ok(table)
    }
    pub(super) fn standard(number: usize) -> Result<Self> {
        Self::new(STANDARD[number - 1])
    }
    pub(super) fn decode(&self, bits: &mut Bits<'_>) -> Result<Option<i64>> {
        let mut code = 0u64;
        for length in 1..=32 {
            code = (code << 1) | u64::from(bits.read(1)?);
            if let Some(index) = code
                .checked_sub(self.first[length])
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n < self.count[length])
            {
                let row = self.rows[self.start[length] + index];
                if row.kind == 2 {
                    return Ok(None);
                }
                let offset = i64::from(bits.read(row.bits)?);
                let value = if row.kind == 1 {
                    row.base - offset
                } else {
                    row.base + offset
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
    pub(super) fn required(&self, bits: &mut Bits<'_>) -> Result<i64> {
        self.decode(bits)?
            .ok_or_else(|| Error::syntax("unexpected JBIG2 Huffman OOB").at(bits.position()))
    }
    pub(super) fn has_oob(&self) -> bool {
        self.rows.iter().any(|row| row.kind == 2)
    }
    pub(super) fn bytes(&self) -> usize {
        size_of::<Self>() + self.rows.capacity() * size_of::<Row>()
    }
}

/// User table segment type 53, 7.4.13 and B.2–B.3. Prefix length zero disables a row.
pub(super) fn custom(reader: &mut Reader<'_>, limits: &Limits, work: &mut Work) -> Result<Table> {
    let flags = reader.byte()?;
    if flags & 128 != 0 {
        return Err(Error::syntax("reserved JBIG2 Huffman table flag"));
    }
    let prefix_bits = ((flags >> 1) & 7) + 1;
    let range_bits = ((flags >> 4) & 7) + 1;
    let low = i64::from(reader.u32()?.cast_signed());
    let high = i64::from(reader.u32()?.cast_signed());
    if low >= high {
        return Err(Error::syntax("invalid JBIG2 Huffman table bounds"));
    }
    let base = match reader.position() {
        Position::Buffer(n) => n,
        _ => 0,
    };
    let mut bits = Bits::new(reader.rest(), base);
    let mut parameters = Vec::new();
    let mut current = low;
    while current < high {
        work.operations(1, limits)?;
        if parameters.len() + 3 > limits.value_nodes
            || (parameters.len() + 3) * size_of::<(u8, u8, i64, u8)>() * 3 + size_of::<Table>()
                > limits.decoded_bytes
        {
            return Err(Error::limit("JBIG2 Huffman table exceeds budget"));
        }
        let length = bits.read(prefix_bits)?;
        let range = bits.read(range_bits)?;
        if length > 32 || range > 32 {
            return Err(Error::syntax("invalid JBIG2 Huffman table lengths"));
        }
        parameters.push((
            u8::try_from(length).map_err(|_| Error::syntax("invalid Huffman prefix length"))?,
            u8::try_from(range).map_err(|_| Error::syntax("invalid Huffman range length"))?,
            current,
            0,
        ));
        current += 1i64 << range;
    }
    parameters.push((bits.small(prefix_bits)?, 32, low - 1, 1));
    parameters.push((bits.small(prefix_bits)?, 32, high, 0));
    if flags & 1 != 0 {
        parameters.push((bits.small(prefix_bits)?, 0, 0, 2));
    }
    bits.finish()?;
    Table::new(&parameters)
}
// Numeric coding parameters from T.88 (02/2000), Annex B, Tables B.1–B.15.
const STANDARD: [&[(u8, u8, i64, u8)]; 15] = [
    &[
        (1, 4, 0, 0),
        (2, 8, 16, 0),
        (3, 16, 272, 0),
        (3, 32, 65808, 0),
    ],
    &[
        (1, 0, 0, 0),
        (2, 0, 1, 0),
        (3, 0, 2, 0),
        (4, 3, 3, 0),
        (5, 6, 11, 0),
        (6, 32, 75, 0),
        (6, 0, 0, 2),
    ],
    &[
        (8, 8, -256, 0),
        (1, 0, 0, 0),
        (2, 0, 1, 0),
        (3, 0, 2, 0),
        (4, 3, 3, 0),
        (5, 6, 11, 0),
        (8, 32, -257, 1),
        (7, 32, 75, 0),
        (6, 0, 0, 2),
    ],
    &[
        (1, 0, 1, 0),
        (2, 0, 2, 0),
        (3, 0, 3, 0),
        (4, 3, 4, 0),
        (5, 6, 12, 0),
        (5, 32, 76, 0),
    ],
    &[
        (7, 8, -255, 0),
        (1, 0, 1, 0),
        (2, 0, 2, 0),
        (3, 0, 3, 0),
        (4, 3, 4, 0),
        (5, 6, 12, 0),
        (7, 32, -256, 1),
        (6, 32, 76, 0),
    ],
    &[
        (5, 10, -2048, 0),
        (4, 9, -1024, 0),
        (4, 8, -512, 0),
        (4, 7, -256, 0),
        (5, 6, -128, 0),
        (5, 5, -64, 0),
        (4, 5, -32, 0),
        (2, 7, 0, 0),
        (3, 7, 128, 0),
        (3, 8, 256, 0),
        (4, 9, 512, 0),
        (4, 10, 1024, 0),
        (6, 32, -2049, 1),
        (6, 32, 2048, 0),
    ],
    &[
        (4, 9, -1024, 0),
        (3, 8, -512, 0),
        (4, 7, -256, 0),
        (5, 6, -128, 0),
        (5, 5, -64, 0),
        (4, 5, -32, 0),
        (4, 5, 0, 0),
        (5, 5, 32, 0),
        (5, 6, 64, 0),
        (4, 7, 128, 0),
        (3, 8, 256, 0),
        (3, 9, 512, 0),
        (3, 10, 1024, 0),
        (5, 32, -1025, 1),
        (5, 32, 2048, 0),
    ],
    &[
        (8, 3, -15, 0),
        (9, 1, -7, 0),
        (8, 1, -5, 0),
        (9, 0, -3, 0),
        (7, 0, -2, 0),
        (4, 0, -1, 0),
        (2, 1, 0, 0),
        (5, 0, 2, 0),
        (6, 0, 3, 0),
        (3, 4, 4, 0),
        (6, 1, 20, 0),
        (4, 4, 22, 0),
        (4, 5, 38, 0),
        (5, 6, 70, 0),
        (5, 7, 134, 0),
        (6, 7, 262, 0),
        (7, 8, 390, 0),
        (6, 10, 646, 0),
        (9, 32, -16, 1),
        (9, 32, 1670, 0),
        (2, 0, 0, 2),
    ],
    &[
        (8, 4, -31, 0),
        (9, 2, -15, 0),
        (8, 2, -11, 0),
        (9, 1, -7, 0),
        (7, 1, -5, 0),
        (4, 1, -3, 0),
        (3, 1, -1, 0),
        (3, 1, 1, 0),
        (5, 1, 3, 0),
        (6, 1, 5, 0),
        (3, 5, 7, 0),
        (6, 2, 39, 0),
        (4, 5, 43, 0),
        (4, 6, 75, 0),
        (5, 7, 139, 0),
        (5, 8, 267, 0),
        (6, 8, 523, 0),
        (7, 9, 779, 0),
        (6, 11, 1291, 0),
        (9, 32, -32, 1),
        (9, 32, 3339, 0),
        (2, 0, 0, 2),
    ],
    &[
        (7, 4, -21, 0),
        (8, 0, -5, 0),
        (7, 0, -4, 0),
        (5, 0, -3, 0),
        (2, 2, -2, 0),
        (5, 0, 2, 0),
        (6, 0, 3, 0),
        (7, 0, 4, 0),
        (8, 0, 5, 0),
        (2, 6, 6, 0),
        (5, 5, 70, 0),
        (6, 5, 102, 0),
        (6, 6, 134, 0),
        (6, 7, 198, 0),
        (6, 8, 326, 0),
        (6, 9, 582, 0),
        (6, 10, 1094, 0),
        (7, 11, 2118, 0),
        (8, 32, -22, 1),
        (8, 32, 4166, 0),
        (2, 0, 0, 2),
    ],
    &[
        (1, 0, 1, 0),
        (2, 1, 2, 0),
        (4, 0, 4, 0),
        (4, 1, 5, 0),
        (5, 1, 7, 0),
        (5, 2, 9, 0),
        (6, 2, 13, 0),
        (7, 2, 17, 0),
        (7, 3, 21, 0),
        (7, 4, 29, 0),
        (7, 5, 45, 0),
        (7, 6, 77, 0),
        (7, 32, 141, 0),
    ],
    &[
        (1, 0, 1, 0),
        (2, 0, 2, 0),
        (3, 1, 3, 0),
        (5, 0, 5, 0),
        (5, 1, 6, 0),
        (6, 1, 8, 0),
        (7, 0, 10, 0),
        (7, 1, 11, 0),
        (7, 2, 13, 0),
        (7, 3, 17, 0),
        (7, 4, 25, 0),
        (8, 5, 41, 0),
        (8, 32, 73, 0),
    ],
    &[
        (1, 0, 1, 0),
        (3, 0, 2, 0),
        (4, 0, 3, 0),
        (5, 0, 4, 0),
        (4, 1, 5, 0),
        (3, 3, 7, 0),
        (6, 1, 15, 0),
        (6, 2, 17, 0),
        (6, 3, 21, 0),
        (6, 4, 29, 0),
        (6, 5, 45, 0),
        (7, 6, 77, 0),
        (7, 32, 141, 0),
    ],
    &[
        (3, 0, -2, 0),
        (3, 0, -1, 0),
        (1, 0, 0, 0),
        (3, 0, 1, 0),
        (3, 0, 2, 0),
    ],
    &[
        (7, 4, -24, 0),
        (6, 2, -8, 0),
        (5, 1, -4, 0),
        (4, 0, -2, 0),
        (3, 0, -1, 0),
        (1, 0, 0, 0),
        (3, 0, 1, 0),
        (4, 0, 2, 0),
        (5, 1, 3, 0),
        (6, 2, 5, 0),
        (7, 4, 9, 0),
        (7, 32, -25, 1),
        (7, 32, 25, 0),
    ],
];

#[cfg(test)]
mod tests {
    use super::{Bits, Table};
    use pagura_testkit::fixture;
    fn walk(number: usize, expected_rows: usize) {
        let table = Table::standard(number).unwrap();
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
                table.decode(&mut Bits::new(&bytes, 0)).unwrap(),
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
        // B.4's worked binary example transmits standard Table B.1.
        let data = [0x42, 0, 0, 0, 0, 0, 1, 1, 0x10, 0x49, 0x23, 0x81, 0x80];
        let limits = pagura_core::Limits::default();
        let custom = super::custom(
            &mut super::Reader::new(&data, 0),
            &limits,
            &mut super::Work::default(),
        )
        .unwrap();
        for (data, expected) in [
            (&[0u8][..], 0),
            (&[0x80, 0][..], 16),
            (&[0xc0, 0, 0][..], 272),
            (&[0xe0, 0, 0, 0, 0][..], 65808),
        ] {
            assert_eq!(custom.required(&mut Bits::new(data, 0)).unwrap(), expected);
        }
    }
}
