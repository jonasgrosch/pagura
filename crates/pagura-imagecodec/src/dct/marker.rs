//! JPEG marker codes (T.81 Table B.1): the byte after `0xFF`.

/// Start of frame, baseline DCT (Huffman).
pub(super) const SOF0: u8 = 0xc0;
/// Start of frame, extended sequential DCT (Huffman).
pub(super) const SOF1: u8 = 0xc1;
/// Start of frame, progressive DCT (Huffman).
pub(super) const SOF2: u8 = 0xc2;
/// Start of frame, lossless (Huffman).
pub(super) const SOF3: u8 = 0xc3;
/// Define Huffman tables.
pub(super) const DHT: u8 = 0xc4;
/// Start of frame, differential sequential DCT (Huffman).
pub(super) const SOF5: u8 = 0xc5;
/// Start of frame, differential lossless (Huffman).
pub(super) const SOF7: u8 = 0xc7;
/// Start of frame, extended sequential DCT (arithmetic).
pub(super) const SOF9: u8 = 0xc9;
/// Start of frame, progressive DCT (arithmetic).
pub(super) const SOF10: u8 = 0xca;
/// Start of frame, lossless (arithmetic).
pub(super) const SOF11: u8 = 0xcb;
/// Define arithmetic coding conditioning.
pub(super) const DAC: u8 = 0xcc;
/// Start of frame, differential sequential DCT (arithmetic).
pub(super) const SOF13: u8 = 0xcd;
/// Start of frame, differential lossless (arithmetic).
pub(super) const SOF15: u8 = 0xcf;
/// Restart with modulo-8 count 0.
pub(super) const RST0: u8 = 0xd0;
/// Restart with modulo-8 count 7.
pub(super) const RST7: u8 = 0xd7;
/// Start of image.
pub(super) const SOI: u8 = 0xd8;
/// End of image.
pub(super) const EOI: u8 = 0xd9;
/// Start of scan.
pub(super) const SOS: u8 = 0xda;
/// Define quantization tables.
pub(super) const DQT: u8 = 0xdb;
/// Define restart interval.
pub(super) const DRI: u8 = 0xdd;
/// Define hierarchical progression.
pub(super) const DHP: u8 = 0xde;
/// Expand reference components.
pub(super) const EXP: u8 = 0xdf;
/// First application segment.
pub(super) const APP0: u8 = 0xe0;
/// Application segment 14, which Adobe uses for its color transform.
pub(super) const APP14: u8 = 0xee;
/// Last application segment.
pub(super) const APP15: u8 = 0xef;
/// Comment.
pub(super) const COM: u8 = 0xfe;
/// For temporary private use in arithmetic coding.
pub(super) const TEM: u8 = 0x01;
/// The prefix byte of every marker.
pub(super) const PREFIX: u8 = 0xff;
/// The byte that follows a stuffed data byte `0xFF` in entropy-coded data.
pub(super) const STUFFED: u8 = 0x00;
