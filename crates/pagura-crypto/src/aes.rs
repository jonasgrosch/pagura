//! AES-128/256 (FIPS 197-upd1) with CBC chaining, for the security handler.
//!
//! AES uses forward and inverse S-box tables, including in key expansion.
//! Their indices depend on secret data; cache access and finite-field
//! arithmetic can have data-dependent timing. This implementation is not
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

const SBOX: [u8; 256] = sbox();
const INV_SBOX: [u8; 256] = invert(&SBOX);
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
                rcon = multiply(rcon, 2);
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

/// FIPS 197 5.1.2: row `r` of the column-major state rotates left by `r`.
fn shift_rows(block: &mut [u8; 16]) {
    rotate_rows(block, 1);
}

/// FIPS 197 5.3.1: rotating left by `3r` rotates row `r` right by `r`.
fn inv_shift_rows(block: &mut [u8; 16]) {
    rotate_rows(block, 3);
}

fn rotate_rows(block: &mut [u8; 16], step: usize) {
    let original = *block;
    for row in 0..4 {
        for column in 0..4 {
            block[4 * column + row] = original[4 * ((column + step * row) % 4) + row];
        }
    }
}

/// FIPS 197 5.1.3, equation 5.7.
fn mix_columns(block: &mut [u8; 16]) {
    mix(block, [2, 3, 1, 1]);
}

/// FIPS 197 5.3.3, equation 5.15.
fn inv_mix_columns(block: &mut [u8; 16]) {
    mix(block, [14, 11, 13, 9]);
}

/// Multiplies each column by the circulant matrix with first row `coefficients`.
fn mix(block: &mut [u8; 16], coefficients: [u8; 4]) {
    for column in block.as_chunks_mut::<4>().0 {
        let mut result = [0; 4];
        for (row, out) in result.iter_mut().enumerate() {
            for (i, byte) in column.iter().enumerate() {
                *out ^= multiply(*byte, coefficients[(i + 4 - row) % 4]);
            }
        }
        *column = result;
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
