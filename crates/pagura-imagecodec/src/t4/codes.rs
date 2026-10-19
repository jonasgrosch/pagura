//! ITU-T T.4 Tables 1–3 run codes and T.6 two-dimensional modes.
use super::Bits;
use pagura_core::{Error, Result};

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
const COMMON: [&str; 13] = [
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
    children: [usize; 2],
    value: Option<usize>,
}
const EMPTY: Node = Node {
    children: [0; 2],
    value: None,
};

const fn tree(black: bool) -> [Node; 416] {
    let mut nodes = [EMPTY; 416];
    let mut next = 1;
    let mut table = 0;
    while table < 3 {
        let (codes, base, step): (&[&str], usize, usize) = if table == 0 {
            (if black { &BLACK } else { &WHITE }, 0, 1)
        } else if table == 1 {
            (if black { &BLACK_MAKEUP } else { &WHITE_MAKEUP }, 64, 64)
        } else {
            (&COMMON, 1792, 64)
        };
        let mut symbol = 0;
        while symbol < codes.len() {
            let bytes = codes[symbol].as_bytes();
            let (mut node, mut index) = (0, 0);
            while index < bytes.len() {
                let bit = (bytes[index] - b'0') as usize;
                if nodes[node].children[bit] == 0 {
                    nodes[node].children[bit] = next;
                    next += 1;
                }
                node = nodes[node].children[bit];
                index += 1;
            }
            nodes[node].value = Some(base + symbol * step);
            symbol += 1;
        }
        table += 1;
    }
    nodes
}
static WHITE_TREE: [Node; 416] = tree(false);
static BLACK_TREE: [Node; 416] = tree(true);

/// Every 13-bit prefix mapped to `run << 4 | length` for the code it starts
/// with, or zero when it starts no complete code.
#[expect(
    clippy::cast_possible_truncation,
    reason = "runs are at most 2560 and code lengths at most 13, so both fit 16 bits"
)]
const fn prefixes(tree: &[Node; 416]) -> [u16; 8192] {
    let mut result = [0; 8192];
    let mut prefix = 0;
    while prefix < 8192 {
        let (mut node, mut length) = (0, 0_usize);
        while length < 13 {
            node = tree[node].children[(prefix >> (12 - length)) & 1];
            length += 1;
            if node == 0 {
                break;
            }
            if let Some(value) = tree[node].value {
                result[prefix] = ((value as u16) << 4) | length as u16;
                break;
            }
        }
        prefix += 1;
    }
    result
}
static WHITE_PREFIXES: [u16; 8192] = prefixes(&WHITE_TREE);
static BLACK_PREFIXES: [u16; 8192] = prefixes(&BLACK_TREE);

pub(super) fn run(bits: &mut Bits<'_>, black: bool) -> Result<usize> {
    // One lookup when 13 bits remain. Truncated, invalid and over-budget
    // codes take the bitwise walk below, which positions their errors.
    if let Some(prefix) = bits.peek(13) {
        let table = if black {
            &BLACK_PREFIXES
        } else {
            &WHITE_PREFIXES
        };
        let entry = table[prefix];
        if entry != 0 && bits.advance(usize::from(entry & 15)) {
            return Ok(usize::from(entry >> 4));
        }
    }
    let tree = if black { &BLACK_TREE } else { &WHITE_TREE };
    let mut node = 0;
    for _ in 0..13 {
        node = tree[node].children[usize::from(bits.bit()?)];
        if node == 0 {
            break;
        }
        if let Some(value) = tree[node].value {
            return Ok(value);
        }
    }
    Err(bits.error("invalid CCITT run code"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Pass,
    Horizontal,
    Vertical(i8),
}
/// The mode code a 7-bit prefix starts with, and its length (T.6 Table 1).
/// Extension and invalid prefixes yield `None`.
const fn prefix_mode(prefix: usize) -> Option<(usize, Mode)> {
    Some(match prefix {
        64..=127 => (1, Mode::Vertical(0)),
        16..=31 => (3, Mode::Horizontal),
        32..=47 => (3, Mode::Vertical(-1)),
        48..=63 => (3, Mode::Vertical(1)),
        8..=15 => (4, Mode::Pass),
        4 | 5 => (6, Mode::Vertical(-2)),
        6 | 7 => (6, Mode::Vertical(2)),
        2 => (7, Mode::Vertical(-3)),
        3 => (7, Mode::Vertical(3)),
        _ => return None,
    })
}

pub(super) fn mode(bits: &mut Bits<'_>) -> Result<Mode> {
    if let Some((length, mode)) = bits.peek(7).and_then(prefix_mode)
        && bits.advance(length)
    {
        return Ok(mode);
    }
    let mut code = 0u8;
    for length in 1..=7 {
        code = (code << 1) | bits.bit()?;
        match (length, code) {
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
                return Err(Error::unsupported(
                    crate::features::CCITT_EXTENSION,
                    "CCITT extension mode is unsupported",
                ));
            }
            _ => (),
        }
    }
    Err(bits.error("invalid CCITT two-dimensional mode"))
}

#[cfg(test)]
mod tests {
    use super::{
        BLACK, BLACK_MAKEUP, BLACK_PREFIXES, Bits, COMMON, Mode, WHITE, WHITE_MAKEUP,
        WHITE_PREFIXES, prefix_mode, run,
    };
    use pagura_core::{ErrorKind, Position};

    fn code(bits: &str) -> usize {
        usize::from_str_radix(bits, 2).unwrap()
    }

    #[test]
    fn run_prefixes_are_exactly_the_extensions_of_tables_1_to_3() {
        for (black, table) in [(false, &WHITE_PREFIXES), (true, &BLACK_PREFIXES)] {
            let lists: [(&[&str], usize, usize); 3] = [
                (if black { &BLACK } else { &WHITE }, 0, 1),
                (if black { &BLACK_MAKEUP } else { &WHITE_MAKEUP }, 64, 64),
                (&COMMON, 1792, 64),
            ];
            let mut expected = [0_u16; 8192];
            for (codes, base, step) in lists {
                for (symbol, bits) in codes.iter().enumerate() {
                    let first = code(bits) << (13 - bits.len());
                    for entry in &mut expected[first..first + (1 << (13 - bits.len()))] {
                        assert_eq!(*entry, 0, "run codes are prefix-free");
                        *entry = u16::try_from((base + symbol * step) << 4 | bits.len()).unwrap();
                    }
                }
            }
            assert_eq!(table, &expected);
        }
    }

    #[test]
    fn mode_prefixes_are_exactly_the_extensions_of_t6_table_1() {
        let modes = [
            ("1", Mode::Vertical(0)),
            ("001", Mode::Horizontal),
            ("010", Mode::Vertical(-1)),
            ("011", Mode::Vertical(1)),
            ("0001", Mode::Pass),
            ("000010", Mode::Vertical(-2)),
            ("000011", Mode::Vertical(2)),
            ("0000010", Mode::Vertical(-3)),
            ("0000011", Mode::Vertical(3)),
        ];
        for prefix in 0..128 {
            let expected = modes
                .iter()
                .find(|(bits, _)| prefix >> (7 - bits.len()) == code(bits))
                .map(|&(bits, mode)| (bits.len(), mode));
            assert_eq!(prefix_mode(prefix), expected, "{prefix:07b}");
        }
    }

    #[test]
    fn exhausted_work_inside_a_looked_up_code_fails_at_the_same_bit() {
        // White make-up 1792 is 00000001000; the remaining bits are padding.
        let data = [0b0000_0001, 0b0000_0000, 0];
        for cap in 0..=11 {
            let mut bits = Bits {
                data: &data,
                at: 0,
                work: 0,
                cap,
            };
            let result = run(&mut bits, false);
            if cap == 11 {
                assert_eq!(result.unwrap(), 1792);
                assert_eq!((bits.at, bits.work), (11, 11));
                continue;
            }
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert_eq!(error.message, "CCITT work budget exceeded");
            // Every affordable bit is read before the walk stops.
            assert_eq!(error.position, Position::Buffer(cap / 8));
            assert_eq!((bits.at, bits.work), (cap, cap + 1));
        }
    }
}
