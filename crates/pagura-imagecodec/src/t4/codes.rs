//! The modified Huffman run-length codes (T.4 Tables 1–3) and the
//! two-dimensional mode codes (T.4 Table 4, T.6 Table 1).
use super::{Bits, Color};
use pagura_core::Result;

// Terminating codes by run length 0–63 (T.4 Table 2).
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
// Make-up codes for 64–1728 (T.4 Table 3), then the extended make-up codes
// for 1792–2560, which both colours share.
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

/// A node of a prefix tree: the children for a 0 and a 1 bit (0 for none)
/// and the run length a complete code stands for.
#[derive(Clone, Copy)]
struct Node {
    children: [u16; 2],
    value: Option<u16>,
}

const EMPTY: Node = Node {
    children: [0; 2],
    value: None,
};

/// Nodes of one colour's tree: the codes have at most 13 bits and share
/// prefixes, so 416 nodes hold every terminating and make-up code.
const NODES: usize = 416;

/// The longest run-length code, in bits.
const LONGEST: usize = 13;

/// Compiles one colour's normative code words into a prefix tree, so that
/// decoding a run visits at most thirteen nodes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "table indexes are below 64 and run lengths at most 2560"
)]
const fn tree(color: Color) -> [Node; NODES] {
    let mut nodes = [EMPTY; NODES];
    let mut next = 1;
    let mut table = 0;
    while table < 3 {
        let black = matches!(color, Color::Black);
        let (codes, base, step): (&[&str], u16, u16) = if table == 0 {
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
                node = nodes[node].children[bit] as usize;
                index += 1;
            }
            nodes[node].value = Some(base + symbol as u16 * step);
            symbol += 1;
        }
        table += 1;
    }
    nodes
}

static WHITE_TREE: [Node; NODES] = tree(Color::White);
static BLACK_TREE: [Node; NODES] = tree(Color::Black);

/// Decodes one terminating or make-up code of `color`: a run length, which
/// is a make-up length when it is 64 or more.
pub(super) fn run(bits: &mut Bits<'_, '_>, color: Color) -> Result<usize> {
    let tree = match color {
        Color::White => &WHITE_TREE,
        Color::Black => &BLACK_TREE,
    };
    let at = bits.position();
    let mut node = 0;
    for _ in 0..LONGEST {
        node = usize::from(tree[node].children[usize::from(bits.bit()?)]);
        if node == 0 {
            break;
        }
        if let Some(value) = tree[node].value {
            return Ok(usize::from(value));
        }
    }
    Err(super::syntax_at("invalid T.4 run-length code", at))
}

/// A two-dimensional coding mode (T.4 4.2.1.3, T.6 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Pass mode: the reference run below is passed over.
    Pass,
    /// Horizontal mode: two one-dimensional runs follow.
    Horizontal,
    /// Vertical mode: the changing element is this far from `b1`, -3 to 3.
    Vertical(i8),
    /// The extension code that enters uncompressed mode.
    Extension,
}

/// Decodes one mode code (T.4 Table 4, T.6 Table 1).
pub(super) fn mode(bits: &mut Bits<'_, '_>) -> Result<Mode> {
    let at = bits.position();
    let mut code = 0u8;
    for length in 1..=7 {
        code = (code << 1) | u8::from(bits.bit()?);
        let mode = match (length, code) {
            (1, 0b1) => Mode::Vertical(0),
            (3, 0b001) => Mode::Horizontal,
            (3, 0b010) => Mode::Vertical(-1),
            (3, 0b011) => Mode::Vertical(1),
            (4, 0b0001) => Mode::Pass,
            (6, 0b00_0010) => Mode::Vertical(-2),
            (6, 0b00_0011) => Mode::Vertical(2),
            (7, 0b000_0010) => Mode::Vertical(-3),
            (7, 0b000_0011) => Mode::Vertical(3),
            (7, 0b000_0001) => Mode::Extension,
            _ => continue,
        };
        return Ok(mode);
    }
    Err(super::syntax_at(
        "invalid T.4 two-dimensional mode code",
        at,
    ))
}
