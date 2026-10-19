//! AES-128/256 (FIPS 197-upd1) with CBC chaining, for the security handler.
//!
//! AES uses forward and inverse S-box tables and GF(2^8) product tables,
//! including in key expansion. Their indices depend on secret data, so cache
//! access can have data-dependent timing. This implementation is not
//! constant-time. CBC follows SP 800-38A 6.2, with the four-block AES-128/256
//! encryption and decryption examples in Appendices F.2.1/F.2.2 and F.2.5/F.2.6.
use pagura_core::{Error, Position, Result};

use crate::secret::wipe;

/// Multiplication in GF(2^8) modulo x^8 + x^4 + x^3 + x + 1 (FIPS 197 4.2).
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

/// Every product `factor • value`, computed at compile time.
const fn products(factor: u8) -> [u8; 256] {
    let mut table = [0; 256];
    let mut value = 0u8;
    loop {
        table[value as usize] = multiply(value, factor);
        if value == u8::MAX {
            break;
        }
        value += 1;
    }
    table
}

/// The S-box of FIPS 197 5.1.1: the inverse in GF(2^8), then the affine map.
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

const fn sbox() -> [u8; 256] {
    let mut table = [0; 256];
    let mut value = 0u8;
    loop {
        table[value as usize] = substitution(value);
        if value == u8::MAX {
            break;
        }
        value += 1;
    }
    table
}

const fn invert(table: &[u8; 256]) -> [u8; 256] {
    let mut inverse = [0; 256];
    let mut value = 0u8;
    loop {
        inverse[table[value as usize] as usize] = value;
        if value == u8::MAX {
            break;
        }
        value += 1;
    }
    inverse
}

/// The state is column-major (FIPS 197 3.4): byte `4c + r` is row `r` of
/// column `c`. Row `r` rotates left by `step * r` columns.
const fn row_rotation(step: usize) -> [usize; 16] {
    let mut sources = [0; 16];
    let mut index = 0;
    while index < 16 {
        let (column, row) = (index / 4, index % 4);
        sources[index] = 4 * ((column + step * row) % 4) + row;
        index += 1;
    }
    sources
}

const SBOX: [u8; 256] = sbox();
const INV_SBOX: [u8; 256] = invert(&SBOX);
const MUL2: [u8; 256] = products(2);
const MUL3: [u8; 256] = products(3);
const MUL9: [u8; 256] = products(9);
const MUL11: [u8; 256] = products(11);
const MUL13: [u8; 256] = products(13);
const MUL14: [u8; 256] = products(14);
/// FIPS 197 5.1.2: row `r` rotates left by `r` positions.
const SHIFT_ROWS: [usize; 16] = row_rotation(1);
/// FIPS 197 5.3.1: rotating left by `3r` rotates right by `r`.
const INV_SHIFT_ROWS: [usize; 16] = row_rotation(3);

/// Expanded AES key; Debug output never prints the key schedule, which is
/// cleared when dropped.
#[derive(Clone)]
pub(crate) struct Aes {
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

impl Drop for Aes {
    fn drop(&mut self) {
        wipe(self.keys.as_flattened_mut());
    }
}

impl Aes {
    /// Expands a 128-bit key (FIPS 197 5.2).
    pub(crate) fn new128(key: &[u8; 16]) -> Self {
        Self::expand(key, 10)
    }

    /// Expands a 256-bit key (FIPS 197 5.2).
    pub(crate) fn new256(key: &[u8; 32]) -> Self {
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
                sub_bytes(&mut temp);
                temp[0] ^= rcon;
                rcon = MUL2[usize::from(rcon)];
            } else if key.len() == 32 && offset % 32 == 16 {
                sub_bytes(&mut temp);
            }
            for byte in temp {
                expanded[offset] = expanded[offset - key.len()] ^ byte;
                offset += 1;
            }
        }
        let mut keys = [[0; 16]; 15];
        keys.as_flattened_mut().copy_from_slice(&expanded[..240]);
        wipe(&mut expanded);
        Self { keys, rounds }
    }

    /// Encrypts one block (FIPS 197 5.1).
    pub(crate) fn encrypt_block(&self, block: &mut [u8; 16]) {
        xor(block, &self.keys[0]);
        for key in &self.keys[1..self.rounds] {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            xor(block, key);
        }
        sub_bytes(block);
        shift_rows(block);
        xor(block, &self.keys[self.rounds]);
    }

    /// Decrypts one block (FIPS 197 5.3, also unpadded ECB).
    pub(crate) fn decrypt_block(&self, block: &mut [u8; 16]) {
        xor(block, &self.keys[self.rounds]);
        for key in self.keys[1..self.rounds].iter().rev() {
            inv_shift_rows(block);
            inv_sub_bytes(block);
            xor(block, key);
            inv_mix_columns(block);
        }
        inv_shift_rows(block);
        inv_sub_bytes(block);
        xor(block, &self.keys[0]);
    }

    /// Encrypts whole blocks in place with CBC chaining (SP 800-38A 6.2).
    pub(crate) fn encrypt_cbc(&self, iv: &[u8; 16], blocks: &mut [[u8; 16]]) {
        let mut previous = *iv;
        for block in blocks {
            xor(block, &previous);
            self.encrypt_block(block);
            previous = *block;
        }
    }

    /// Decrypts whole blocks in place with CBC chaining (SP 800-38A 6.2),
    /// without removing padding.
    pub(crate) fn decrypt_cbc(&self, iv: &[u8; 16], blocks: &mut [[u8; 16]]) {
        let mut previous = *iv;
        for block in blocks {
            let ciphertext = *block;
            self.decrypt_block(block);
            xor(block, &previous);
            previous = ciphertext;
        }
    }

    /// The cipher key, which begins the key schedule (FIPS 197 5.2).
    #[cfg(test)]
    pub(crate) fn cipher_key(&self) -> &[u8] {
        let words = if self.rounds == 10 { 1 } else { 2 };
        self.keys[..words].as_flattened()
    }
}

/// AddRoundKey (FIPS 197 5.1.4), and the CBC chaining XOR.
fn xor(block: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, key_byte) in block.iter_mut().zip(key) {
        *byte ^= key_byte;
    }
}

fn sub_bytes(bytes: &mut [u8]) {
    for byte in bytes {
        *byte = SBOX[usize::from(*byte)];
    }
}

fn inv_sub_bytes(block: &mut [u8; 16]) {
    for byte in block {
        *byte = INV_SBOX[usize::from(*byte)];
    }
}

fn shift_rows(block: &mut [u8; 16]) {
    let state = *block;
    *block = SHIFT_ROWS.map(|source| state[source]);
}

fn inv_shift_rows(block: &mut [u8; 16]) {
    let state = *block;
    *block = INV_SHIFT_ROWS.map(|source| state[source]);
}

/// FIPS 197 5.1.3, equation 5.7.
fn mix_columns(block: &mut [u8; 16]) {
    for column in block.as_chunks_mut::<4>().0 {
        let [s0, s1, s2, s3] = *column;
        let [i0, i1, i2, i3] = column.map(usize::from);
        *column = [
            MUL2[i0] ^ MUL3[i1] ^ s2 ^ s3,
            s0 ^ MUL2[i1] ^ MUL3[i2] ^ s3,
            s0 ^ s1 ^ MUL2[i2] ^ MUL3[i3],
            MUL3[i0] ^ s1 ^ s2 ^ MUL2[i3],
        ];
    }
}

/// FIPS 197 5.3.3, equation 5.15.
fn inv_mix_columns(block: &mut [u8; 16]) {
    for column in block.as_chunks_mut::<4>().0 {
        let [i0, i1, i2, i3] = column.map(usize::from);
        *column = [
            MUL14[i0] ^ MUL11[i1] ^ MUL13[i2] ^ MUL9[i3],
            MUL9[i0] ^ MUL14[i1] ^ MUL11[i2] ^ MUL13[i3],
            MUL13[i0] ^ MUL9[i1] ^ MUL14[i2] ^ MUL11[i3],
            MUL11[i0] ^ MUL13[i1] ^ MUL9[i2] ^ MUL14[i3],
        ];
    }
}

/// The number of PKCS #7 padding bytes ending an AES plaintext (RFC 5652
/// 6.3), checked in its last block. Positions are relative to that block.
///
/// The check stops at the first wrong byte. The crate makes no constant-time
/// claim, and a padding failure here reports damaged file data to its reader,
/// not to a remote party probing ciphertexts.
///
/// # Errors
/// Returns `Syntax` unless the block ends with `n` bytes of value `n`, 1–16.
pub(crate) fn pkcs7_padding(last: &[u8; 16]) -> Result<usize> {
    let count = usize::from(last[15]);
    if !(1..=16).contains(&count) || last[16 - count..].iter().any(|&byte| byte != last[15]) {
        return Err(Error::syntax("invalid AES PKCS #7 padding").at(Position::Buffer(15)));
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
