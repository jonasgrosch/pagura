//! AES-128/256 (FIPS 197) and CBC/ECB (SP 800-38A).
use pagura_core::{Error, Position, Result};

const fn multiply(mut a: u8, mut b: u8) -> u8 {
    let mut value = 0;
    let mut i = 0;
    while i < 8 {
        if b & 1 != 0 {
            value ^= a;
        }
        a = if a & 0x80 != 0 {
            (a << 1) ^ 0x1b
        } else {
            a << 1
        };
        b >>= 1;
        i += 1;
    }
    value
}
const fn substitution(value: u8) -> u8 {
    let mut inverse = 1;
    let mut i = 0;
    // x^254 in GF(2^8); zero maps to zero before the affine transform.
    while i < 254 {
        inverse = multiply(inverse, value);
        i += 1;
    }
    inverse
        ^ inverse.rotate_left(1)
        ^ inverse.rotate_left(2)
        ^ inverse.rotate_left(3)
        ^ inverse.rotate_left(4)
        ^ 0x63
}
const fn tables() -> ([u8; 256], [u8; 256]) {
    let mut forward = [0; 256];
    let mut inverse = [0; 256];
    let mut value = 0u8;
    loop {
        let mapped = substitution(value);
        forward[value as usize] = mapped;
        inverse[mapped as usize] = value;
        if value == 255 {
            break;
        }
        value += 1;
    }
    (forward, inverse)
}
const TABLES: ([u8; 256], [u8; 256]) = tables();

/// Expanded AES key; Debug output never prints the key schedule.
#[derive(Clone)]
pub struct Aes {
    keys: [[u8; 16]; 15],
    rounds: usize,
}
impl std::fmt::Debug for Aes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Aes")
            .field("rounds", &self.rounds)
            .finish_non_exhaustive()
    }
}
impl Aes {
    /// Expands a 128-bit key (FIPS 197 section 5.2).
    pub fn new128(key: &[u8; 16]) -> Self {
        Self::expand(key, 10)
    }
    /// Expands a 256-bit key (FIPS 197 section 5.2).
    pub fn new256(key: &[u8; 32]) -> Self {
        Self::expand(key, 14)
    }
    fn expand(key: &[u8], rounds: usize) -> Self {
        let mut expanded = [0; 240];
        expanded[..key.len()].copy_from_slice(key);
        let mut offset = key.len();
        let mut rcon = 1;
        while offset < (rounds + 1) * 16 {
            let mut temp = [0; 4];
            temp.copy_from_slice(&expanded[offset - 4..offset]);
            if offset.is_multiple_of(key.len()) {
                temp.rotate_left(1);
                for byte in &mut temp {
                    *byte = TABLES.0[usize::from(*byte)];
                }
                temp[0] ^= rcon;
                rcon = multiply(rcon, 2);
            } else if key.len() == 32 && offset % 32 == 16 {
                for byte in &mut temp {
                    *byte = TABLES.0[usize::from(*byte)];
                }
            }
            for byte in temp {
                expanded[offset] = expanded[offset - key.len()] ^ byte;
                offset += 1;
            }
        }
        let mut keys = [[0; 16]; 15];
        for (round, chunk) in keys.iter_mut().zip(expanded.as_chunks::<16>().0.iter()) {
            round.copy_from_slice(chunk);
        }
        Self { keys, rounds }
    }
    /// Encrypts one block (FIPS 197 section 5.1).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_key(block, &self.keys[0]);
        for round in 1..=self.rounds {
            for byte in &mut *block {
                *byte = TABLES.0[usize::from(*byte)];
            }
            shift_rows(block, false);
            if round != self.rounds {
                mix_columns(block, false);
            }
            add_key(block, &self.keys[round]);
        }
    }
    /// Decrypts one block (FIPS 197 section 5.3, also unpadded ECB).
    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        add_key(block, &self.keys[self.rounds]);
        for round in (0..self.rounds).rev() {
            shift_rows(block, true);
            for byte in &mut *block {
                *byte = TABLES.1[usize::from(*byte)];
            }
            add_key(block, &self.keys[round]);
            if round != 0 {
                mix_columns(block, true);
            }
        }
    }
    /// Encrypts whole CBC blocks in place, without padding (SP 800-38A F.2).
    ///
    /// # Errors
    /// Returns `InvalidArgument` if input is not a multiple of 16 bytes; input is unchanged.
    pub fn encrypt_cbc(&self, iv: &[u8; 16], data: &mut [u8]) -> Result<()> {
        if !data.len().is_multiple_of(16) {
            return Err(Error::invalid_argument(
                "AES CBC plaintext must contain whole blocks",
            ));
        }
        let mut previous = *iv;
        for chunk in data.as_chunks_mut::<16>().0.iter_mut() {
            let mut block = [0; 16];
            block.copy_from_slice(chunk);
            add_key(&mut block, &previous);
            self.encrypt_block(&mut block);
            chunk.copy_from_slice(&block);
            previous = block;
        }
        Ok(())
    }
    /// Decrypts whole CBC blocks in place without unpadding (SP 800-38A F.2).
    ///
    /// # Errors
    /// Returns `Syntax` if ciphertext is not a multiple of 16 bytes; input is unchanged.
    pub fn decrypt_cbc(&self, iv: &[u8; 16], data: &mut [u8]) -> Result<()> {
        if !data.len().is_multiple_of(16) {
            return Err(
                Error::syntax("AES CBC ciphertext must contain whole blocks")
                    .at(Position::Buffer(data.len())),
            );
        }
        let mut previous = *iv;
        for chunk in data.as_chunks_mut::<16>().0.iter_mut() {
            let mut ciphertext = [0; 16];
            ciphertext.copy_from_slice(chunk);
            let mut block = ciphertext;
            self.decrypt_block(&mut block);
            add_key(&mut block, &previous);
            chunk.copy_from_slice(&block);
            previous = ciphertext;
        }
        Ok(())
    }
}
fn add_key(block: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, key_byte) in block.iter_mut().zip(key) {
        *byte ^= key_byte;
    }
}
fn shift_rows(block: &mut [u8; 16], inverse: bool) {
    let original = *block;
    for row in 0..4 {
        for column in 0..4 {
            let source = if inverse {
                (column + 4 - row) % 4
            } else {
                (column + row) % 4
            };
            block[4 * column + row] = original[4 * source + row];
        }
    }
}
fn mix_columns(block: &mut [u8; 16], inverse: bool) {
    let coefficients = if inverse {
        [14, 11, 13, 9]
    } else {
        [2, 3, 1, 1]
    };
    for column in block.as_chunks_mut::<4>().0.iter_mut() {
        let mut result = [0; 4];
        for (row, out) in result.iter_mut().enumerate() {
            for (i, byte) in column.iter().enumerate() {
                *out ^= multiply(*byte, coefficients[(i + 4 - row) % 4]);
            }
        }
        column.copy_from_slice(&result);
    }
}

/// Removes and validates every PKCS #7 padding byte from an AES plaintext.
///
/// # Errors
/// Returns `Syntax` for empty input, a partial block, or invalid padding.
pub fn unpad_pkcs7(data: &mut Vec<u8>) -> Result<()> {
    let Some(&last) = data.last() else {
        return Err(Error::syntax("AES plaintext has no padding").at(Position::Buffer(0)));
    };
    let count = usize::from(last);
    if !data.len().is_multiple_of(16)
        || !(1..=16).contains(&count)
        || data[data.len() - count..].iter().any(|&byte| byte != last)
    {
        return Err(
            Error::syntax("invalid AES PKCS #7 padding").at(Position::Buffer(data.len() - 1))
        );
    }
    data.truncate(data.len() - count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bytes<const N: usize>(hex: &str) -> [u8; N] {
        let mut out = [0; N];
        assert_eq!(hex.len(), N * 2);
        for (slot, chunk) in out.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            *slot = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
        }
        out
    }
    #[test]
    fn fips197_appendix_c() {
        let plain = bytes("00112233445566778899aabbccddeeff");
        for (aes, expected) in [
            (
                Aes::new128(&bytes("000102030405060708090a0b0c0d0e0f")),
                bytes("69c4e0d86a7b0430d8cdb78070b4c55a"),
            ),
            (
                Aes::new256(&bytes(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )),
                bytes("8ea2b7ca516745bfeafc49904b496089"),
            ),
        ] {
            let mut block = plain;
            aes.encrypt_block(&mut block);
            assert_eq!(block, expected);
            aes.decrypt_block(&mut block);
            assert_eq!(block, plain);
        }
    }
    #[test]
    fn sp800_38a_f2_cbc128_and_cbc256() {
        let plain: [u8; 64] = bytes(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        ));
        let iv = bytes("000102030405060708090a0b0c0d0e0f");
        for (aes, expected) in [
            (
                Aes::new128(&bytes("2b7e151628aed2a6abf7158809cf4f3c")),
                bytes(concat!(
                    "7649abac8119b246cee98e9b12e9197d",
                    "5086cb9b507219ee95db113a917678b2",
                    "73bed6b8e3c1743b7116e69e22229516",
                    "3ff1caa1681fac09120eca307586e1a7"
                )),
            ),
            (
                Aes::new256(&bytes(
                    "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
                )),
                bytes(concat!(
                    "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
                    "9cfc4e967edb808d679f777bc6702c7d",
                    "39f23369a9d9bacfa530e26304231461",
                    "b2eb05e2c39be9fcda6c19078c6a9d1b"
                )),
            ),
        ] {
            let mut data = plain;
            aes.encrypt_cbc(&iv, &mut data).unwrap();
            assert_eq!(data, expected);
            aes.decrypt_cbc(&iv, &mut data).unwrap();
            assert_eq!(data, plain);
        }
    }
    #[test]
    fn padding_and_partial_blocks() {
        for count in 1..=16u8 {
            let mut data = vec![0; 16 - usize::from(count)];
            data.extend(std::iter::repeat_n(count, usize::from(count)));
            unpad_pkcs7(&mut data).unwrap();
            assert_eq!(data.len(), 16 - usize::from(count));
        }
        for data in [
            vec![],
            vec![0; 16],
            vec![17; 16],
            vec![1; 15],
            vec![2; 16].into_iter().chain([1]).collect(),
        ] {
            assert!(unpad_pkcs7(&mut data.clone()).is_err());
        }
        let mut bad = vec![2; 16];
        bad[14] = 3;
        assert!(unpad_pkcs7(&mut bad).is_err());
        let aes = Aes::new128(&[0; 16]);
        let mut partial = [7; 17];
        assert!(aes.encrypt_cbc(&[0; 16], &mut partial).is_err());
        assert_eq!(partial, [7; 17]);
        assert!(aes.decrypt_cbc(&[0; 16], &mut partial).is_err());
        assert_eq!(partial, [7; 17]);
    }
}
