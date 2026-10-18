//! The zlib wrapper (RFC 1950 2.2): a two-byte header and an Adler-32
//! trailer around DEFLATE data.
use super::Level;

/// CM = 8: the data is DEFLATE.
const DEFLATE: u8 = 8;
/// The largest CINFO, the base-2 logarithm of the window minus eight.
const MAX_WINDOW_LOG: u8 = 7;
/// CMF and FLG, read as a big-endian 16-bit number, are a multiple of this.
const CHECK_MODULUS: u16 = 31;
/// FLG bit: a preset dictionary identifier follows the header.
pub(super) const PRESET_DICTIONARY: u8 = 0x20;
/// The header length.
pub(super) const HEADER: usize = 2;
/// The Adler-32 trailer length.
pub(super) const TRAILER: usize = 4;

/// Whether `cmf` declares DEFLATE data, whatever its window.
pub(super) const fn is_deflate(cmf: u8) -> bool {
    cmf & 0x0f == DEFLATE
}

/// The window a valid header declares, or `None` if `cmf` and `flg` do not
/// form a valid header. Preset dictionaries are checked separately.
pub(super) fn window(cmf: u8, flg: u8) -> Option<usize> {
    let log = cmf >> 4;
    (is_deflate(cmf)
        && log <= MAX_WINDOW_LOG
        && u16::from_be_bytes([cmf, flg]).is_multiple_of(CHECK_MODULUS))
    .then_some(1 << (log + 8))
}

/// The header [`super::compress`] writes: a 32 KiB window, no preset
/// dictionary, and FLEVEL 0 (fastest) or 2 (default) as advice only.
pub(super) const fn header(level: Level) -> [u8; HEADER] {
    let cmf = (MAX_WINDOW_LOG << 4) | DEFLATE;
    let level = match level {
        Level::Fast => 0,
        Level::Default => 2,
    };
    let flg = level << 6;
    let remainder = u16::from_be_bytes([cmf, flg]) % CHECK_MODULUS;
    // FCHECK is below 31, so it fits the low five bits.
    let check = ((CHECK_MODULUS - remainder) % CHECK_MODULUS) as u8;
    [cmf, flg | check]
}

/// The Adler-32 checksum of the uncompressed data (RFC 1950 2.2, 9). The
/// sums are reduced once per `RUN` bytes, however the data arrives.
#[derive(Debug, Clone, Copy)]
pub(super) struct Adler32 {
    a: u32,
    b: u32,
    /// Bytes summed since the last reduction.
    pending: usize,
}

impl Adler32 {
    const MODULUS: u32 = 65521;
    /// The most bytes summed before `b` could overflow 32 bits, even when
    /// every byte is 255.
    const RUN: usize = 5552;

    pub(super) const fn new() -> Self {
        Self {
            a: 1,
            b: 0,
            pending: 0,
        }
    }

    pub(super) fn update(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let count = bytes.len().min(Self::RUN - self.pending);
            let (head, tail) = bytes.split_at(count);
            for &byte in head {
                self.a += u32::from(byte);
                self.b += self.a;
            }
            self.pending += count;
            if self.pending == Self::RUN {
                self.a %= Self::MODULUS;
                self.b %= Self::MODULUS;
                self.pending = 0;
            }
            bytes = tail;
        }
    }

    pub(super) const fn finish(self) -> u32 {
        ((self.b % Self::MODULUS) << 16) | (self.a % Self::MODULUS)
    }
}

#[cfg(test)]
mod tests {
    use super::{Adler32, header, window};
    use crate::flate::Level;

    #[test]
    fn headers_and_checksums_match_rfc_1950() {
        // The two headers zlib writes for levels 1 and 6.
        assert_eq!(header(Level::Fast), [0x78, 0x01]);
        assert_eq!(header(Level::Default), [0x78, 0x9c]);
        assert_eq!(window(0x78, 0x9c), Some(32 * 1024));
        assert_eq!(window(0x08, 0x1d), Some(256));
        assert_eq!(window(0x78, 0x9d), None);
        assert_eq!(window(0x88, 0x98), None);
        // A widely published value: Adler-32 of "Wikipedia" is 0x11E60398.
        let mut adler = Adler32::new();
        adler.update(b"Wiki");
        adler.update(b"pedia");
        assert_eq!(adler.finish(), 0x11e6_0398);
        let mut long = Adler32::new();
        long.update(&vec![0xff; 100_000]);
        let (mut a, mut b) = (1_u64, 0_u64);
        for _ in 0..100_000 {
            a = (a + 0xff) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(u64::from(long.finish()), (b << 16) | a);
    }

    #[test]
    fn arbitrary_emission_boundaries_preserve_the_checksum() {
        for size in [0, 1, 32, 5551, 5552, 5553, 16384] {
            let bytes = vec![255; size];
            let mut expected = (1_u32, 0_u32);
            for &v in &bytes {
                expected.0 = (expected.0 + u32::from(v)) % 65521;
                expected.1 = (expected.1 + expected.0) % 65521;
            }
            for chunk in [1, 7, 258, 5552, 8192] {
                let mut adler = Adler32::new();
                for bytes in bytes.chunks(chunk) {
                    adler.update(bytes);
                }
                assert_eq!(adler.finish(), (expected.1 << 16) | expected.0);
            }
        }
    }
}
