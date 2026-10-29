//! Huffman and arithmetic sequential/progressive JPEG (T.81 B, D, F and G),
//! used by ISO 32000-2 clause 7.4.8. Samples use a separable floating-point
//! IDCT; independently encoded fixtures allow an absolute error of one level.
mod arithmetic;
mod deferred;
mod entropy;
mod frame;
mod scan;
#[cfg(test)]
mod tests;
mod transform;
use entropy::Huffman;
use frame::Frame;
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Recovery, Repair, Result};

/// Stable JPEG repairs (ADR 0003).
pub mod tags {
    use super::Repair;
    /// Ignore non-one unused entropy bits at a scan/restart boundary
    /// (T.81 B.1.1.5 and E.1.4); entropy symbols are never synthesized.
    pub const ENTROPY_PADDING: Repair = Repair::new("jpeg.entropy-padding");
    /// Use the known 12-byte Adobe APP14 prefix and ignore an extension
    /// (ISO 32000-2 7.4.8, Table 13). The extended layout is reader policy;
    /// Adobe Technical Note 5116 remains pending primary-text review.
    pub const ADOBE_MARKER: Repair = Repair::new("jpeg.adobe-marker");
    /// Ignore excess entropy bytes after the prescribed MCUs only when the
    /// next unstuffed marker is EOI (T.81 B.2.1). Never skip another scan.
    pub const ENTROPY_EXCESS: Repair = Repair::new("jpeg.entropy-excess");
    /// Discard a restart marker immediately before EOI after all required
    /// MCUs were decoded (T.81 B.2.1, B.2.4.4); no entropy is synthesized.
    pub const TRAILING_RESTART: Repair = Repair::new("jpeg.trailing-restart");
}

/// A decoded JPEG image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DctImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Components per pixel (1, 3 or 4).
    pub components: u8,
    /// Interleaved 8-bit samples after any colour transform, top row first.
    pub samples: Vec<u8>,
}

pub(crate) use deferred::{Deferred, Native, decode_deferred};

struct Decoder {
    /// Whether baseline frames decode into native planes.
    native: bool,
    frame: Option<Frame>,
    quant: [Option<[u16; 64]>; 4],
    huffman: [Option<Huffman>; 8],
    conditioning: arithmetic::Conditioning,
    restart: usize,
    adobe: Option<u8>,
    work: usize,
    padding_reported: bool,
    adobe_reported: bool,
}

/// Decodes baseline, extended sequential and progressive JPEG, with Huffman
/// or arithmetic entropy coding and eight or twelve bit sample precision.
/// Twelve-bit samples are reconstructed and clamped in 0..4095. Colour
/// transforms retain that precision (chroma centre 2048), then samples are
/// scaled to 0..255 with a single rounding. This normalization is reader policy:
/// PDF 7.4.8 specifies bytes, but no twelve-to-eight bit conversion.
/// APP14 takes precedence over `color_transform` (Table 13). Chroma uses
/// nearest-neighbour expansion; no smoothing is applied to the encoded data.
///
/// # Errors
/// Returns positioned syntax/limit errors, or stable Unsupported tags for
/// lossless/hierarchical coding and precision other than eight or twelve bits.
pub fn decode(data: &[u8], color_transform: Option<i64>, limits: &Limits) -> Result<DctImage> {
    decode_with_diagnostics(
        data,
        color_transform,
        limits,
        &mut Diagnostics::new(Recovery::Strict),
    )
}

/// Decodes JPEG with reported padding, extended APP14 and final entropy-suffix
/// repairs. The default [`decode`] entry point remains strict. Reports at most
/// one padding repair per image, at its first scan/restart boundary (T.81
/// B.1.1.5, E.1.4). Extra entropy bytes can be discarded only after the required
/// MCUs and immediately before EOI (T.81 B.2.1; [`tags::ENTROPY_EXCESS`]).
///
/// # Errors
/// As [`decode`], including stable repair tags when strict mode refuses a repair.
pub fn decode_with_diagnostics(
    data: &[u8],
    color_transform: Option<i64>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<DctImage> {
    if data.len() > limits.stream_bytes {
        return Err(Error::limit("JPEG input budget exceeded").at(Position::Buffer(0)));
    }
    decode_inner(data, color_transform, limits, diagnostics).map_err(|e| e.at(Position::Buffer(0)))
}

fn decode_inner(
    data: &[u8],
    color_transform: Option<i64>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<DctImage> {
    let mut decoder = run(data, color_transform, limits, diagnostics, None)?;
    let mut frame = decoder
        .frame
        .take()
        .ok_or(Error::syntax("JPEG has no frame"))?;
    transform::output(&mut frame, decoder.adobe, color_transform, limits)
}

/// Parses and decodes every segment through EOI. With `native`, baseline
/// frames decode into native planes (see [`deferred`]) as its options say.
fn run(
    data: &[u8],
    color_transform: Option<i64>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    native: Option<Native>,
) -> Result<Decoder> {
    if data.get(..2) != Some(&[255, 216]) {
        return Err(Error::syntax("missing JPEG SOI"));
    }
    let mut decoder = Decoder {
        native: native.is_some(),
        frame: None,
        quant: [None; 4],
        huffman: std::array::from_fn(|_| None),
        conditioning: arithmetic::Conditioning::default(),
        restart: 0,
        adobe: None,
        work: 0,
        padding_reported: false,
        adobe_reported: false,
    };
    let mut offset = 2;
    loop {
        let at = offset;
        let marker = marker(data, &mut offset)?;
        if marker == 0xd9 {
            if decoder.frame.is_none() {
                return Err(Error::syntax("JPEG has no frame"));
            }
            return Ok(decoder);
        }
        if matches!(marker, 0xd0..=0xd8 | 0x01) {
            return Err(Error::syntax("unexpected JPEG standalone marker").at(Position::Buffer(at)));
        }
        let payload = segment(data, &mut offset)?;
        let result = match marker {
            0xc0..=0xc2 | 0xc9 | 0xca => {
                if decoder.frame.is_some() {
                    return Err(Error::syntax("multiple JPEG frames").at(Position::Buffer(at)));
                }
                let frame = if let Some(native) = native {
                    let mut frame = Frame::header(payload, marker, limits)?;
                    deferred::prepare(
                        &mut frame,
                        (decoder.adobe, color_transform),
                        limits,
                        native,
                    )?;
                    frame
                } else {
                    Frame::parse(payload, marker, limits)?
                };
                decoder.frame = Some(frame);
                Ok(())
            }
            0xcb | 0xcd..=0xcf => Err(Error::unsupported(
                crate::features::JPEG_ARITHMETIC,
                "arithmetic JPEG coding is unsupported",
            )),
            0xc3 | 0xc5..=0xc7 | 0xde | 0xdf => Err(Error::unsupported(
                crate::features::JPEG_PROCESS,
                "JPEG lossless or hierarchical process is unsupported",
            )),
            0xdb => quant_tables(payload, &mut decoder.quant),
            0xc4 => {
                let pairs = deferred::paired(decoder.frame.as_ref());
                entropy::tables(payload, &mut decoder.huffman, (decoder.native, pairs))
            }
            0xcc => decoder.conditioning.define(payload),
            0xdd if payload.len() == 2 => {
                decoder.restart = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
                Ok(())
            }
            0xda => {
                offset = decoder.scan(payload, data, offset, limits, diagnostics)?;
                Ok(())
            }
            0xee if payload.starts_with(b"Adobe") => {
                if payload.len() < 12 || payload[11] > 2 {
                    return Err(Error::syntax("invalid Adobe JPEG marker").at(Position::Buffer(at)));
                }
                if payload.len() > 12 && !decoder.adobe_reported {
                    diagnostics.repair(
                        Diagnostic::new(
                            tags::ADOBE_MARKER,
                            "extended Adobe JPEG marker uses its known prefix (7.4.8, Table 13)",
                        )
                        .at(Position::Buffer(at)),
                    )?;
                    decoder.adobe_reported = true;
                }
                decoder.adobe = Some(payload[11]);
                Ok(())
            }
            0xe0..=0xef | 0xfe => Ok(()),
            _ => Err(Error::syntax("invalid or unsupported JPEG marker")),
        };
        result.map_err(|e| e.at(Position::Buffer(at)))?;
    }
}

fn quant_tables(mut payload: &[u8], tables: &mut [Option<[u16; 64]>; 4]) -> Result<()> {
    while let Some((&info, rest)) = payload.split_first() {
        let precision = info >> 4;
        let table = tables
            .get_mut(usize::from(info & 15))
            .ok_or(Error::syntax("invalid JPEG quantization table index"))?;
        if precision > 1 {
            return Err(Error::syntax("invalid JPEG quantization precision"));
        }
        let length = 64 * (usize::from(precision) + 1);
        let values = rest
            .get(..length)
            .ok_or(Error::syntax("truncated JPEG quantization table"))?;
        let mut quant = [0; 64];
        for (i, q) in quant.iter_mut().enumerate() {
            *q = if precision == 0 {
                u16::from(values[i])
            } else {
                u16::from_be_bytes([values[2 * i], values[2 * i + 1]])
            };
            if *q == 0 {
                return Err(Error::syntax("zero JPEG quantizer"));
            }
        }
        *table = Some(quant);
        payload = &rest[length..];
    }
    Ok(())
}

fn marker(data: &[u8], offset: &mut usize) -> Result<u8> {
    if data.get(*offset) != Some(&255) {
        return Err(Error::syntax("expected JPEG marker").at(Position::Buffer(*offset)));
    }
    while data.get(*offset) == Some(&255) {
        *offset += 1;
    }
    let value = *data
        .get(*offset)
        .ok_or(Error::syntax("truncated JPEG marker").at(Position::Buffer(*offset)))?;
    *offset += 1;
    if value == 0 {
        return Err(
            Error::syntax("stuffed byte outside JPEG scan").at(Position::Buffer(*offset - 1))
        );
    }
    Ok(value)
}

fn segment<'a>(data: &'a [u8], offset: &mut usize) -> Result<&'a [u8]> {
    let length_bytes = data
        .get(*offset..)
        .and_then(|d| d.get(..2))
        .ok_or(Error::syntax("truncated JPEG segment"))?;
    let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    let end = offset
        .checked_add(length)
        .filter(|&v| v <= data.len())
        .ok_or(Error::syntax("truncated JPEG segment"))?;
    if length < 2 {
        return Err(Error::syntax("invalid JPEG segment length"));
    }
    let payload = &data[*offset + 2..end];
    *offset = end;
    Ok(payload)
}

/// Locates the JPEG EOI without interpreting pixels; segment lengths protect
/// marker-like bytes in APP payloads, and scan stuffing protects entropy data.
pub(crate) fn data_length(data: &[u8]) -> Result<usize> {
    if data.get(..2) != Some(&[255, 216]) {
        return Err(Error::syntax("missing JPEG SOI"));
    }
    let mut offset = 2;
    let mut scan = false;
    loop {
        if scan {
            loop {
                let byte = *data
                    .get(offset)
                    .ok_or(Error::syntax("truncated inline JPEG scan"))?;
                if byte != 255 {
                    offset += 1;
                    continue;
                }
                if matches!(data.get(offset + 1), Some(0 | 0xd0..=0xd7)) {
                    offset += 2;
                    continue;
                }
                break;
            }
        }
        let code = marker(data, &mut offset)?;
        if code == 0xd9 {
            return Ok(offset);
        }
        if scan && matches!(code, 0xd0..=0xd7) {
            continue;
        }
        if matches!(code, 0xd0..=0xd8 | 0x01) {
            return Err(Error::syntax("unexpected inline JPEG marker"));
        }
        segment(data, &mut offset)?;
        scan = code == 0xda;
    }
}
