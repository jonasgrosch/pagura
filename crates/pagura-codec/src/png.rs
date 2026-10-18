//! Eight-bit PNG encoding (PNG Second Edition 5, 6.2, 9, 11.2 and 12.8).
//!
//! Rows are tightly packed, top to bottom, without premultiplication. Output
//! contains only IHDR, contiguous IDAT chunks and IEND, with no color profile.
//! The filters are the PNG predictors' arithmetic run forwards, and IDAT
//! holds [`flate::compress`] output; ADR 0020 records why the encoder lives
//! in this crate.
use crate::bounded::{BoundedBytes, ENCODE_BUDGET, encode_limit, filled};
use crate::flate;
use crate::predictor::PngFilter;
use pagura_core::{Error, Limits, Result};

/// The PNG signature (PNG Second Edition 5.2).
const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
/// Length, type and CRC fields around each chunk's data (5.3).
const CHUNK_FIELDS: usize = 12;
/// IHDR data length (11.2.2).
const IHDR_LENGTH: usize = 13;
/// Signature, IHDR, one IDAT and IEND without image data.
const FRAMING: usize = SIGNATURE.len() + 3 * CHUNK_FIELDS + IHDR_LENGTH;
/// The shortest complete zlib stream [`flate::compress`] emits.
const MIN_ZLIB: usize = 8;
/// The largest image dimension and chunk length, 2^31 - 1 (5.3, 11.2.2).
const MAX_FIELD: u32 = 0x7fff_ffff;
/// Sample depth: this encoder writes eight bits per sample.
const BIT_DEPTH: u8 = 8;
/// IDAT data per chunk, an encoder choice below `MAX_FIELD`.
const IDAT_LENGTH: usize = 65536;
/// The reflected CRC-32 polynomial of ISO 3309 (5.5, Annex D).
const CRC_POLYNOMIAL: u32 = 0xedb8_8320;
/// The largest absolute signed value of a filtered byte, which bounds the
/// adaptive heuristic's per-byte score (12.8).
const MAX_SCORE: u64 = 128;

/// The eight-bit sample layout of tightly packed input pixels; the
/// discriminant is the PNG color type (11.2.2, Table 11.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u8)]
pub enum ColorType {
    /// One grayscale byte per pixel (PNG color type 0).
    Gray = 0,
    /// Red, green and blue bytes (PNG color type 2).
    Rgb = 2,
    /// Red, green, blue and straight alpha bytes (PNG color type 6).
    #[default]
    Rgba = 6,
}

impl ColorType {
    const fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

/// PNG encoding settings and per-operation budgets. Start from
/// [`Options::default`] and adjust with the `with_*` methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Options {
    /// Pixel channel layout. Defaults to straight RGBA.
    pub color_type: ColorType,
    /// Compression effort. Defaults to [`flate::Level::Default`].
    pub level: flate::Level,
    /// Pixel/input/output budgets. PNG framing counts toward `stream_bytes`.
    pub limits: Limits,
}

impl Options {
    /// These options with `color_type`.
    #[must_use]
    pub const fn with_color_type(mut self, color_type: ColorType) -> Self {
        self.color_type = color_type;
        self
    }

    /// These options with `level`.
    #[must_use]
    pub const fn with_level(mut self, level: flate::Level) -> Self {
        self.level = level;
        self
    }

    /// These options with `limits`.
    #[must_use]
    pub const fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
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
/// chunks; callers choose any external color-management policy.
///
/// # Errors
///
/// Returns `InvalidArgument` for invalid dimensions or an incorrect pixel
/// byte count, and `Limit` for arithmetic overflow, allocation failure or a
/// resource budget exceeded. Neither carries a position: the input is not a
/// byte stream with meaningful offsets. No incomplete PNG is returned.
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
    if !(1..=MAX_FIELD).contains(&width) || !(1..=MAX_FIELD).contains(&height) {
        return Err(Error::invalid_argument("invalid PNG dimensions"));
    }
    let width = usize::try_from(width).map_err(|_| encode_limit())?;
    let height = usize::try_from(height).map_err(|_| encode_limit())?;
    let count = width
        .checked_mul(height)
        .filter(|&n| n <= options.limits.pixels)
        .ok_or(encode_limit())?;
    let channels = options.color_type.channels();
    let size = count
        .checked_mul(channels)
        .filter(|&n| n <= options.limits.decoded_bytes)
        .ok_or(encode_limit())?;
    if pixels.len() != size {
        return Err(Error::invalid_argument("PNG pixel byte count mismatch"));
    }
    if options.limits.stream_bytes < FRAMING + MIN_ZLIB {
        return Err(encode_limit());
    }
    let row_bytes = width.checked_mul(channels).ok_or(encode_limit())?;
    // The signed-difference score of a row must not overflow.
    u64::try_from(row_bytes)
        .ok()
        .and_then(|n| n.checked_mul(MAX_SCORE))
        .ok_or(encode_limit())?;
    let filtered_size = size.checked_add(height).ok_or(encode_limit())?;
    let filtered = filter(pixels, row_bytes, channels, filtered_size)?;
    let mut limits = options.limits;
    limits.decoded_bytes = filtered_size; // tags are framing, not pixel bytes
    limits.stream_bytes -= FRAMING; // reserve the minimum PNG framing before compression
    let compressed = flate::compress(&filtered, options.level, &limits)?;
    // Release the working image before allocating the final chunked output.
    drop(filtered);
    let mut output = BoundedBytes::new(options.limits.stream_bytes, ENCODE_BUDGET);
    output.extend(&SIGNATURE)?;
    let mut ihdr = [0; IHDR_LENGTH];
    ihdr[..4].copy_from_slice(&field(width)?.to_be_bytes());
    ihdr[4..8].copy_from_slice(&field(height)?.to_be_bytes());
    // Compression, filter and interlace methods stay 0 (11.2.2).
    ihdr[8] = BIT_DEPTH;
    ihdr[9] = options.color_type as u8;
    chunk(&mut output, *b"IHDR", &ihdr)?;
    for bytes in compressed.chunks(IDAT_LENGTH) {
        chunk(&mut output, *b"IDAT", bytes)?;
    }
    chunk(&mut output, *b"IEND", &[])?;
    Ok(output.into_vec())
}

fn field(value: usize) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|&value| value <= MAX_FIELD)
        .ok_or(encode_limit())
}

fn filter(pixels: &[u8], row_bytes: usize, channels: usize, cap: usize) -> Result<Vec<u8>> {
    let mut output = BoundedBytes::new(cap, ENCODE_BUDGET);
    let mut candidate = filled(row_bytes, 0)?;
    let mut best = filled(row_bytes, 0)?;
    let mut previous: &[u8] = &[];
    for row in pixels.chunks_exact(row_bytes) {
        let (mut best_score, mut best_filter) = (u64::MAX, PngFilter::None);
        for filter in PngFilter::ALL {
            let mut score = 0;
            for (i, (&byte, target)) in row.iter().zip(&mut candidate).enumerate() {
                let left = i.checked_sub(channels).map_or(0, |p| row[p]);
                let above = previous.get(i).copied().unwrap_or(0);
                let corner = i
                    .checked_sub(channels)
                    .and_then(|p| previous.get(p))
                    .copied()
                    .unwrap_or(0);
                *target = byte.wrapping_sub(filter.predict(left, above, corner));
                let value = u16::from(*target);
                score += u64::from(value.min(256 - value));
            }
            if score < best_score {
                best_score = score;
                best_filter = filter;
                std::mem::swap(&mut candidate, &mut best);
            }
        }
        output.push(best_filter as u8)?;
        output.extend(&best)?;
        previous = row;
    }
    Ok(output.into_vec())
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
            value = (value >> 1) ^ if value & 1 == 0 { 0 } else { CRC_POLYNOMIAL };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC: [u32; 256] = crc_table();

fn chunk(output: &mut BoundedBytes, name: [u8; 4], bytes: &[u8]) -> Result<()> {
    output.extend(&field(bytes.len())?.to_be_bytes())?;
    output.extend(&name)?;
    output.extend(bytes)?;
    let mut crc = u32::MAX;
    for &byte in name.iter().chain(bytes) {
        let index = usize::from((crc ^ u32::from(byte)).to_le_bytes()[0]);
        crc = (crc >> 8) ^ CRC[index];
    }
    output.extend(&(!crc).to_be_bytes())
}
