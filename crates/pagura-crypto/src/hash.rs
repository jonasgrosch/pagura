//! Streaming hash primitives from RFC 1321 and FIPS 180-4.

mod sha1;
pub use sha1::{Sha1, sha1};

// Only fixed-size block offsets are indexed; input is consumed through slices.
fn absorb<const N: usize>(
    buffer: &mut [u8; N],
    used: &mut usize,
    mut data: &[u8],
    mut compress: impl FnMut(&[u8; N]),
) {
    while !data.is_empty() {
        let count = (N - *used).min(data.len());
        buffer[*used..*used + count].copy_from_slice(&data[..count]);
        *used += count;
        data = &data[count..];
        if *used == N {
            compress(buffer);
            *used = 0;
        }
    }
}
// RFC 1321 section 3.4: floor(2^32 * abs(sin(i))), i=1..64.
const MD5_K: [u32; 64] = [
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
    0x244_1453,
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
    0x488_1d05,
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
const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// Streaming MD5 (RFC 1321); used only by legacy PDF encryption.
#[derive(Clone, Debug)]
pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    used: usize,
    bytes: u64,
}
impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}
impl Md5 {
    /// Initializes the RFC 1321 chaining state.
    pub const fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buffer: [0; 64],
            used: 0,
            bytes: 0,
        }
    }
    /// Absorbs bytes without allocating. Length is counted modulo 2^64 bits.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            md5_compress(&mut self.state, block);
        });
    }
    /// Appends RFC 1321 padding and returns the digest.
    pub fn finalize(mut self) -> [u8; 16] {
        let bits = self.bytes.wrapping_mul(8).to_le_bytes();
        let count = if self.used < 56 {
            56 - self.used
        } else {
            120 - self.used
        };
        let mut padding = [0; 64];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut out = [0; 16];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}
fn md5_compress(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut words = [0; 16];
    for (word, chunk) in words.iter_mut().zip(block.as_chunks::<4>().0.iter()) {
        *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
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
                .wrapping_add(MD5_K[i])
                .wrapping_add(words[g])
                .rotate_left(MD5_S[i]),
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
/// Computes the MD5 digest (RFC 1321).
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut hash = Md5::new();
    hash.update(data);
    hash.finalize()
}
// FIPS 180-4 section 4.2.2: fractional cube roots of the first 64 primes.
const K32: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0xfc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x6ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Streaming SHA-256 (FIPS 180-4).
#[derive(Clone, Debug)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    used: usize,
    bytes: u64,
}
impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}
impl Sha256 {
    /// Initializes the FIPS 180-4 chaining state.
    pub const fn new() -> Self {
        Self {
            state: [
                1_779_033_703,
                3_144_134_277,
                1_013_904_242,
                2_773_480_762,
                1_359_893_119,
                2_600_822_924,
                528_734_635,
                1_541_459_225,
            ],
            buffer: [0; 64],
            used: 0,
            bytes: 0,
        }
    }
    /// Absorbs bytes without allocation, counting length modulo the specified length field.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            sha32_compress(&mut self.state, block);
        });
    }
    /// Appends FIPS 180-4 padding and returns the digest.
    pub fn finalize(mut self) -> [u8; 32] {
        let bits = self.bytes.wrapping_mul(8).to_be_bytes();
        let count = if self.used < 56 {
            56 - self.used
        } else {
            120 - self.used
        };
        let mut padding = [0; 64];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut out = [0; 32];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
/// Computes the SHA-256 digest (FIPS 180-4).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(data);
    hash.finalize()
}

fn sha32_compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut words = [0u32; 64];
    for (word, chunk) in words.iter_mut().zip(block.as_chunks::<4>().0.iter()) {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(chunk);
        *word = u32::from_be_bytes(bytes);
    }
    for i in 16..64 {
        let x = words[i - 15];
        let y = words[i - 2];
        let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
        let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
        words[i] = words[i - 16]
            .wrapping_add(s0)
            .wrapping_add(words[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (word, k) in words.into_iter().zip(K32) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add((e & f) ^ (!e & g))
            .wrapping_add(k)
            .wrapping_add(word);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(value);
    }
}

/// Streaming SHA-224 (FIPS 180-4 section 6.3).
#[derive(Clone, Debug)]
pub struct Sha224 {
    state: [u32; 8],
    buffer: [u8; 64],
    used: usize,
    bytes: u64,
}
impl Default for Sha224 {
    fn default() -> Self {
        Self::new()
    }
}
impl Sha224 {
    /// Initializes the chaining state of FIPS 180-4 section 5.3.2; this is
    /// not truncated SHA-256.
    pub const fn new() -> Self {
        Self {
            state: [
                0xc105_9ed8,
                0x367c_d507,
                0x3070_dd17,
                0xf70e_5939,
                0xffc0_0b31,
                0x6858_1511,
                0x64f9_8fa7,
                0xbefa_4fa4,
            ],
            buffer: [0; 64],
            used: 0,
            bytes: 0,
        }
    }
    /// Absorbs bytes without allocation, counting length modulo the specified length field.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            sha32_compress(&mut self.state, block);
        });
    }
    /// Appends FIPS 180-4 padding and returns the first seven state words.
    pub fn finalize(mut self) -> [u8; 28] {
        let bits = self.bytes.wrapping_mul(8).to_be_bytes();
        let count = if self.used < 56 {
            56 - self.used
        } else {
            120 - self.used
        };
        let mut padding = [0; 64];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut out = [0; 28];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
/// Computes the SHA-224 digest (FIPS 180-4 section 6.3).
pub fn sha224(data: &[u8]) -> [u8; 28] {
    let mut hash = Sha224::new();
    hash.update(data);
    hash.finalize()
}

// FIPS 180-4 section 4.2.3: fractional cube roots of the first 80 primes.
const K64: [u64; 80] = [
    0x428a_2f98_d728_ae22,
    0x7137_4491_23ef_65cd,
    0xb5c0_fbcf_ec4d_3b2f,
    0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538,
    0x59f1_11f1_b605_d019,
    0x923f_82a4_af19_4f9b,
    0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242,
    0x1283_5b01_4570_6fbe,
    0x2431_85be_4ee4_b28c,
    0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f,
    0x80de_b1fe_3b16_96b1,
    0x9bdc_06a7_25c7_1235,
    0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2,
    0xefbe_4786_384f_25e3,
    0xfc1_9dc6_8b8c_d5b5,
    0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275,
    0x4a74_84aa_6ea6_e483,
    0x5cb0_a9dc_bd41_fbd4,
    0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab,
    0xa831_c66d_2db4_3210,
    0xb003_27c8_98fb_213f,
    0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2,
    0xd5a7_9147_930a_a725,
    0x6ca_6351_e003_826f,
    0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc,
    0x2e1b_2138_5c26_c926,
    0x4d2c_6dfc_5ac4_2aed,
    0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de,
    0x766a_0abb_3c77_b2a8,
    0x81c2_c92e_47ed_aee6,
    0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364,
    0xa81a_664b_bc42_3001,
    0xc24b_8b70_d0f8_9791,
    0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218,
    0xd699_0624_5565_a910,
    0xf40e_3585_5771_202a,
    0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8,
    0x1e37_6c08_5141_ab53,
    0x2748_774c_df8e_eb99,
    0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63,
    0x4ed8_aa4a_e341_8acb,
    0x5b9c_ca4f_7763_e373,
    0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc,
    0x78a5_636f_4317_2f60,
    0x84c8_7814_a1f0_ab72,
    0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28,
    0xa450_6ceb_de82_bde9,
    0xbef9_a3f7_b2c6_7915,
    0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c,
    0xd186_b8c7_21c0_c207,
    0xeada_7dd6_cde0_eb1e,
    0xf57d_4f7f_ee6e_d178,
    0x6f0_67aa_7217_6fba,
    0xa63_7dc5_a2c8_98a6,
    0x113f_9804_bef9_0dae,
    0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84,
    0x32ca_ab7b_40c7_2493,
    0x3c9e_be0a_15c9_bebc,
    0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6,
    0x597f_299c_fc65_7e2a,
    0x5fcb_6fab_3ad6_faec,
    0x6c44_198c_4a47_5817,
];

/// Streaming SHA-512 (FIPS 180-4).
#[derive(Clone, Debug)]
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    used: usize,
    bytes: u128,
}
impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}
impl Sha512 {
    /// Initializes the FIPS 180-4 chaining state.
    pub const fn new() -> Self {
        Self {
            state: [
                7_640_891_576_956_012_808,
                13_503_953_896_175_478_587,
                4_354_685_564_936_845_355,
                11_912_009_170_470_909_681,
                5_840_696_475_078_001_361,
                11_170_449_401_992_604_703,
                2_270_897_969_802_886_507,
                6_620_516_959_819_538_809,
            ],
            buffer: [0; 128],
            used: 0,
            bytes: 0,
        }
    }
    /// Absorbs bytes without allocation, counting length modulo the specified length field.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u128);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            sha64_compress(&mut self.state, block);
        });
    }
    /// Appends FIPS 180-4 padding and returns the digest.
    pub fn finalize(mut self) -> [u8; 64] {
        let bits = self.bytes.wrapping_mul(8).to_be_bytes();
        let count = if self.used < 112 {
            112 - self.used
        } else {
            240 - self.used
        };
        let mut padding = [0; 128];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut out = [0; 64];
        for (chunk, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
/// Computes the SHA-512 digest (FIPS 180-4).
pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut hash = Sha512::new();
    hash.update(data);
    hash.finalize()
}

fn sha64_compress(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut words = [0u64; 80];
    for (word, chunk) in words.iter_mut().zip(block.as_chunks::<8>().0.iter()) {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(chunk);
        *word = u64::from_be_bytes(bytes);
    }
    for i in 16..80 {
        let x = words[i - 15];
        let y = words[i - 2];
        let s0 = x.rotate_right(1) ^ x.rotate_right(8) ^ (x >> 7);
        let s1 = y.rotate_right(19) ^ y.rotate_right(61) ^ (y >> 6);
        words[i] = words[i - 16]
            .wrapping_add(s0)
            .wrapping_add(words[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (word, k) in words.into_iter().zip(K64) {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add((e & f) ^ (!e & g))
            .wrapping_add(k)
            .wrapping_add(word);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(value);
    }
}

/// Streaming SHA-384 (FIPS 180-4).
#[derive(Clone, Debug)]
pub struct Sha384 {
    state: [u64; 8],
    buffer: [u8; 128],
    used: usize,
    bytes: u128,
}
impl Default for Sha384 {
    fn default() -> Self {
        Self::new()
    }
}
impl Sha384 {
    /// Initializes the FIPS 180-4 chaining state.
    pub const fn new() -> Self {
        Self {
            state: [
                14_680_500_436_340_154_072,
                7_105_036_623_409_894_663,
                10_473_403_895_298_186_519,
                1_526_699_215_303_891_257,
                7_436_329_637_833_083_697,
                10_282_925_794_625_328_401,
                15_784_041_429_090_275_239,
                5_167_115_440_072_839_076,
            ],
            buffer: [0; 128],
            used: 0,
            bytes: 0,
        }
    }
    /// Absorbs bytes without allocation, counting length modulo the specified length field.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u128);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            sha64_compress(&mut self.state, block);
        });
    }
    /// Appends FIPS 180-4 padding and returns the digest.
    pub fn finalize(mut self) -> [u8; 48] {
        let bits = self.bytes.wrapping_mul(8).to_be_bytes();
        let count = if self.used < 112 {
            112 - self.used
        } else {
            240 - self.used
        };
        let mut padding = [0; 128];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut out = [0; 48];
        for (chunk, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}
/// Computes the SHA-384 digest (FIPS 180-4).
pub fn sha384(data: &[u8]) -> [u8; 48] {
    let mut hash = Sha384::new();
    hash.update(data);
    hash.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut output = String::new();
        for byte in bytes {
            write!(&mut output, "{byte:02x}").unwrap();
        }
        output
    }
    #[test]
    fn independent_hashlib_padding_boundaries() {
        let data = pagura_testkit::fixture("crypto", "hash-padding-boundaries.input");
        let digests = pagura_testkit::fixture("crypto", "hash-padding-boundaries.digests");
        for line in std::str::from_utf8(&digests).unwrap().lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            let length: usize = fields[0].parse().unwrap();
            let input = &data[..length];
            assert_eq!(hex(&md5(input)), fields[1], "length {length}");
            assert_eq!(hex(&sha256(input)), fields[2], "length {length}");
            assert_eq!(hex(&sha384(input)), fields[3], "length {length}");
            assert_eq!(hex(&sha512(input)), fields[4], "length {length}");
        }
    }
    #[test]
    fn rfc1321_suite() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"a")), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(b"message digest")),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            hex(&md5(b"abcdefghijklmnopqrstuvwxyz")),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            hex(&md5(
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
            )),
            "d174ab98d277d9f5a5611c2c9f419d9f"
        );
        assert_eq!(
            hex(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }
    #[test]
    fn hashlib_known_answers_and_streaming() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha384(b"")),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(
            hex(&sha512(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha384(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex(&sha512(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha384(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "3391fdddfc8dc7393707a65b1b4709397cf8b1d162af05abfe8f450de5f36bc6b0455a8520bc4e6f5fe95b1fe3c8452b"
        );
        assert_eq!(
            hex(&sha512(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "204a8fc6dda82f0a0ced7beb8e08a41657c16ef468b228a8279be331a703c33596fd15c13b1b07f9aa1d3bea57789ca031ad85c7a71dd70354ec631238ca3445"
        );
        assert_eq!(hex(&sha256(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")),"cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1");
        assert_eq!(hex(&sha384(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")),"09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039");
        assert_eq!(hex(&sha512(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")),"8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909");
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex(&md5(&million)), "7707d6ae4e027c70eea2a935c2296f21");
        assert_eq!(
            hex(&sha256(&million)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_eq!(
            hex(&sha384(&million)),
            "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"
        );
        assert_eq!(
            hex(&sha512(&million)),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
        // Every split around both block sizes and both length-field boundaries.
        let input: Vec<u8> = (0..=255).cycle().take(521).collect();
        for split in 0..=input.len() {
            let mut hash = Md5::new();
            hash.update(&input[..split]);
            hash.update(&[]);
            hash.update(&input[split..]);
            assert_eq!(hash.finalize(), md5(&input));
            let mut hash = Sha256::new();
            hash.update(&input[..split]);
            hash.update(&[]);
            hash.update(&input[split..]);
            assert_eq!(hash.finalize(), sha256(&input));
            let mut hash = Sha384::new();
            hash.update(&input[..split]);
            hash.update(&[]);
            hash.update(&input[split..]);
            assert_eq!(hash.finalize(), sha384(&input));
            let mut hash = Sha512::new();
            hash.update(&input[..split]);
            hash.update(&[]);
            hash.update(&input[split..]);
            assert_eq!(hash.finalize(), sha512(&input));
        }
    }
}

#[cfg(test)]
mod standard_tests;
