//! MD5 (RFC 1321), used only by legacy PDF encryption and ICC profile IDs.

// RFC 1321 3.4: floor(2^32 * abs(sin(i))), i=1..64.
const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];
// RFC 1321 3.4: per-round left rotations.
const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

hasher! {
    /// Streaming MD5 (RFC 1321). MD5 is broken; it serves only legacy PDF
    /// encryption and ICC profile identifiers.
    Md5 {
        state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
        block: 64,
        compress: compress,
        length: length_le64,
        output: [u8; 16] = le32,
    }
}

/// RFC 1321 3.5: the digest is the state words, low-order byte first.
fn le32(words: &[u32; 4]) -> [u8; 16] {
    let mut out = [0; 16];
    for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *bytes = word.to_le_bytes();
    }
    out
}

/// RFC 1321 3.4: one 512-bit block.
pub(super) fn compress(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut words = [0; 16];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i {
            0..16 => ((b & c) | (!b & d), i),
            16..32 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            32..48 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let next = b.wrapping_add(
            a.wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(words[g])
                .rotate_left(S[i]),
        );
        a = d;
        d = c;
        c = b;
        b = next;
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d]) {
        *word = word.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Digest;
    use pagura_testkit::{fixture, hex};

    #[test]
    fn every_constant_and_shift_matches_section_3_4() {
        let constants = fixture("crypto", "md5-round-constants.constants");
        let words: Vec<_> = constants
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .collect();
        assert_eq!(K.as_slice(), words);
        let shifts = fixture("crypto", "md5-round-constants.shifts");
        assert_eq!(
            S.as_slice(),
            shifts.into_iter().map(u32::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rfc1321_appendix_a5_suite() {
        for (input, expected) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(hex(&Md5::digest(input.as_bytes())), expected, "{input:?}");
        }
    }
}
