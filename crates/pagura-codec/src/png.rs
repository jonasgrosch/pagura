//! Eight-bit PNG encoding (PNG Second Edition 5, 6.2, 9, 11.2 and 12.8).
//!
//! Rows are tightly packed, top to bottom, without premultiplication. Output
//! contains only IHDR, contiguous IDAT chunks and IEND, with no colour profile.
use crate::{
    bounded::{Bytes, filled, limit},
    flate,
};
use pagura_core::{Error, Limits, Position, Result};

/// The eight-bit sample layout of tightly packed input pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ColorType {
    /// One grayscale byte per pixel (PNG colour type 0).
    Gray,
    /// Red, green and blue bytes (PNG colour type 2).
    Rgb,
    /// Red, green, blue and straight alpha bytes (PNG colour type 6).
    #[default]
    Rgba,
}

impl ColorType {
    const fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
    const fn code(self) -> u8 {
        match self {
            Self::Gray => 0,
            Self::Rgb => 2,
            Self::Rgba => 6,
        }
    }
}

/// PNG encoding settings and per-operation budgets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Options {
    /// Pixel channel layout. Defaults to straight RGBA.
    pub color_type: ColorType,
    /// Compression effort. Defaults to [`flate::Level::Default`].
    pub level: flate::Level,
    /// Pixel/input/output budgets. PNG framing counts toward `stream_bytes`.
    pub limits: Limits,
}

/// Encodes tightly packed eight-bit Gray, RGB or straight RGBA pixels.
///
/// PNG Second Edition 12.8's adaptive heuristic minimizes the sum of absolute
/// signed filtered bytes; ties choose the smaller filter tag. All five filters
/// are considered, including on the first row. Alpha and hidden RGB samples
/// are preserved exactly (6.2); premultiplied inputs must be converted first.
/// Dimensions must be nonzero and at most 2^31-1 (11.2.2).
///
/// Input is bounded by `limits.decoded_bytes` and `limits.pixels`, final PNG
/// bytes by `limits.stream_bytes`. Scratch comprises a filtered image (one
/// extra tag per row), two row buffers and bounded Flate scratch. The input is
/// borrowed. Output is deterministic, non-interlaced, and uses no ancillary
/// chunks; callers choose any external colour-management policy.
///
/// # Errors
///
/// Returns positioned `InvalidArgument` for invalid dimensions or an incorrect pixel
/// byte count, and `Limit` for arithmetic overflow, allocation failure or a
/// resource budget exceeded. No incomplete PNG is returned.
///
/// # Examples
///
/// ```
/// use pagura_codec::png;
/// let image = png::encode(&[255, 0, 0, 128], 1, 1, &png::Options::default())?;
/// assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
/// # Ok::<(), pagura_core::Error>(())
/// ```
pub fn encode(pixels: &[u8], width: u32, height: u32, options: &Options) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > 0x7fff_ffff || height > 0x7fff_ffff {
        return Err(Error::invalid_argument("invalid PNG dimensions").at(Position::Buffer(0)));
    }
    let width = usize::try_from(width).map_err(|_| limit(0))?;
    let height = usize::try_from(height).map_err(|_| limit(0))?;
    let count = width
        .checked_mul(height)
        .filter(|&n| n <= options.limits.pixels)
        .ok_or_else(|| limit(0))?;
    let channels = options.color_type.channels();
    let size = count
        .checked_mul(channels)
        .filter(|&n| n <= options.limits.decoded_bytes)
        .ok_or_else(|| limit(0))?;
    if pixels.len() != size {
        return Err(Error::invalid_argument("PNG pixel byte count mismatch")
            .at(Position::Buffer(pixels.len())));
    }
    // Signature/chunks need 57 bytes, plus at least eight bytes of zlib.
    if options.limits.stream_bytes < 65 {
        return Err(limit(0));
    }
    let row_bytes = width.checked_mul(channels).ok_or_else(|| limit(0))?;
    // At most 128 per byte in the signed-difference score, without overflow.
    u64::try_from(row_bytes)
        .ok()
        .and_then(|n| n.checked_mul(128))
        .ok_or_else(|| limit(0))?;
    let filtered_size = size.checked_add(height).ok_or_else(|| limit(0))?;
    let filtered = filter(pixels, row_bytes, channels, filtered_size)?;
    let mut limits = options.limits;
    limits.decoded_bytes = filtered_size; // tags are framing, not pixel bytes
    limits.stream_bytes -= 57; // reserve the minimum PNG framing before compression
    let compressed = flate::compress(&filtered, options.level, &limits)?;
    // Release the working image before allocating the final chunked output.
    drop(filtered);
    let mut output = Bytes::new(options.limits.stream_bytes);
    output.extend(b"\x89PNG\r\n\x1a\n")?;
    let mut ihdr = [0; 13];
    ihdr[..4].copy_from_slice(&dimension(width)?.to_be_bytes());
    ihdr[4..8].copy_from_slice(&dimension(height)?.to_be_bytes());
    ihdr[8] = 8;
    ihdr[9] = options.color_type.code();
    chunk(&mut output, *b"IHDR", &ihdr)?;
    for bytes in compressed.chunks(65536) {
        chunk(&mut output, *b"IDAT", bytes)?;
    }
    chunk(&mut output, *b"IEND", &[])?;
    Ok(output.data)
}

fn dimension(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| limit(0))
}

fn filter(pixels: &[u8], row_bytes: usize, channels: usize, cap: usize) -> Result<Vec<u8>> {
    let mut output = Bytes::new(cap);
    let mut candidate = filled(row_bytes, 0)?;
    let mut best = filled(row_bytes, 0)?;
    let mut previous: &[u8] = &[];
    for row in pixels.chunks_exact(row_bytes) {
        let (mut best_score, mut best_tag) = (u64::MAX, 0);
        for tag in 0..5 {
            let mut score = 0;
            for (i, (&byte, target)) in row.iter().zip(&mut candidate).enumerate() {
                let left = i.checked_sub(channels).map_or(0, |p| row[p]);
                let above = previous.get(i).copied().unwrap_or(0);
                let corner = i
                    .checked_sub(channels)
                    .and_then(|p| previous.get(p))
                    .copied()
                    .unwrap_or(0);
                let prediction = match tag {
                    1 => left,
                    2 => above,
                    3 => mean(left, above),
                    4 => crate::output::paeth(left, above, corner),
                    _ => 0,
                };
                *target = byte.wrapping_sub(prediction);
                let value = u16::from(*target);
                score += u64::from(value.min(256 - value));
            }
            if score < best_score {
                best_score = score;
                best_tag = tag;
                std::mem::swap(&mut candidate, &mut best);
            }
        }
        output.extend(&[best_tag])?;
        output.extend(&best)?;
        previous = row;
    }
    Ok(output.data)
}

fn mean(a: u8, b: u8) -> u8 {
    (a & b) + ((a ^ b) >> 1)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the CRC table has only 256 entries"
)]
const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 == 0 { 0 } else { 0xedb8_8320 };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC: [u32; 256] = crc_table();

fn chunk(output: &mut Bytes, name: [u8; 4], bytes: &[u8]) -> Result<()> {
    output.extend(&dimension(bytes.len())?.to_be_bytes())?;
    output.extend(&name)?;
    output.extend(bytes)?;
    let mut crc = u32::MAX;
    for &byte in name.iter().chain(bytes) {
        let index = usize::from((crc ^ u32::from(byte)).to_le_bytes()[0]);
        crc = (crc >> 8) ^ CRC[index];
    }
    output.extend(&(!crc).to_be_bytes())
}
