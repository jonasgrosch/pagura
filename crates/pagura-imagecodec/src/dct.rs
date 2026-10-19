//! Huffman and arithmetic sequential and progressive JPEG (T.81 Annexes B, D,
//! F and G) for `DCTDecode` (7.4.8). Samples use a separable floating-point
//! IDCT; independently encoded fixtures allow an absolute error of one level.
mod arithmetic;
mod entropy;
mod frame;
mod marker;
mod scan;
#[cfg(test)]
mod tests;
mod transform;
use frame::{Frame, Header, Process};
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Recovery, Result};

/// A decoded JPEG image.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DctImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Components per pixel (1, 3 or 4).
    pub components: u8,
    /// Interleaved 8-bit samples after any color transform, top row first.
    pub samples: Vec<u8>,
}

/// The image a JPEG's frame header describes, read without decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DctHeader {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Components per pixel (1, 3 or 4).
    pub components: u8,
}

/// The `Limit` messages of a JPEG decode.
const MESSAGES: crate::budget::Messages = crate::budget::Messages {
    work: "JPEG work budget exceeded",
    memory: "JPEG coefficient memory budget exceeded",
    output: "JPEG sample budget exceeded",
};

struct Decoder {
    frame: Option<Frame>,
    quant: [Option<[u16; 64]>; frame::TABLES],
    huffman: entropy::Tables,
    conditioning: arithmetic::Conditioning,
    restart: usize,
    adobe: Option<u8>,
    budget: crate::budget::Budget,
    padding_reported: bool,
    adobe_reported: bool,
}

/// Decodes baseline, extended sequential and progressive JPEG, with Huffman
/// or arithmetic entropy coding and eight or twelve bit sample precision.
/// Twelve-bit samples are reconstructed and clamped in 0..4095. Color
/// transforms retain that precision (chroma center 2048), then samples are
/// scaled to 0..255 with a single rounding. This normalization is reader policy:
/// 7.4.8 specifies bytes, but no twelve-to-eight bit conversion.
/// APP14 takes precedence over `color_transform` (Table 13). Chroma uses
/// nearest-neighbor expansion; no smoothing is applied to the encoded data.
///
/// Work is bounded by `decode_work` (64 units per 8×8 block in each scan),
/// coefficients and native-precision planes by `decode_bytes`, and the
/// samples by `pixels` and `decoded_bytes`.
///
/// # Errors
/// Returns syntax and limit errors positioned in `data`, or stable
/// Unsupported tags for lossless and hierarchical coding and precision other
/// than eight or twelve bits.
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
/// MCUs and immediately before EOI (T.81 B.2.1; [`crate::repairs::JPEG_ENTROPY_EXCESS`]),
/// as can a restart marker that directly precedes EOI
/// ([`crate::repairs::JPEG_TRAILING_RESTART`]).
///
/// # Errors
/// As [`decode`], including stable repair tags when strict mode refuses a repair.
pub fn decode_with_diagnostics(
    data: &[u8],
    color_transform: Option<i64>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<DctImage> {
    let mut bytes = start(data)?;
    let mut decoder = Decoder {
        frame: None,
        quant: [None; frame::TABLES],
        huffman: entropy::Tables::default(),
        conditioning: arithmetic::Conditioning::default(),
        restart: 0,
        adobe: None,
        budget: crate::budget::Budget::new(limits, MESSAGES),
        padding_reported: false,
        adobe_reported: false,
    };
    loop {
        let at = bytes.position();
        let code = read_marker(&mut bytes)?;
        if code == marker::EOI {
            let frame = decoder
                .frame
                .ok_or(Error::syntax("JPEG has no frame").at(at))?;
            return transform::output(&frame, decoder.adobe, color_transform)
                .map_err(|error| error.at(at));
        }
        if matches!(code, marker::RST0..=marker::SOI | marker::TEM) {
            return Err(Error::syntax("unexpected JPEG standalone marker").at(at));
        }
        let payload = segment(&mut bytes)?;
        let result = match code {
            marker::SOF0..=marker::SOF2 | marker::SOF9 | marker::SOF10 => {
                let Some((process, coding)) = Process::of(code) else {
                    return Err(Error::syntax("invalid JPEG frame marker").at(at));
                };
                if decoder.frame.is_some() {
                    return Err(Error::syntax("multiple JPEG frames").at(at));
                }
                Frame::parse(payload, process, coding, limits, &mut decoder.budget)
                    .map(|frame| decoder.frame = Some(frame))
            }
            marker::SOF11 | marker::SOF13..=marker::SOF15 => Err(Error::unsupported(
                crate::features::JPEG_ARITHMETIC,
                "arithmetic JPEG coding is unsupported",
            )),
            marker::SOF3 | marker::SOF5..=marker::SOF7 | marker::DHP | marker::EXP => {
                Err(Error::unsupported(
                    crate::features::JPEG_PROCESS,
                    "JPEG lossless or hierarchical process is unsupported",
                ))
            }
            marker::DQT => quant_tables(payload, &mut decoder.quant),
            marker::DHT => entropy::tables(payload, &mut decoder.huffman),
            marker::DAC => decoder.conditioning.define(payload),
            marker::DRI => restart_interval(payload).map(|n| decoder.restart = n),
            marker::SOS => decoder
                .scan(payload, bytes.clone(), diagnostics)
                .and_then(|end| bytes.seek(end)),
            marker::APP14 if payload.data().starts_with(b"Adobe") => {
                adobe(payload, &mut decoder, diagnostics)
            }
            marker::APP0..=marker::APP15 | marker::COM => Ok(()),
            _ => Err(Error::syntax("invalid or unsupported JPEG marker")),
        };
        result.map_err(|e| e.at(at))?;
    }
}

/// Reads the frame header of a JPEG without decoding it, so a caller can
/// check the image against its own metadata first.
///
/// # Errors
/// As [`decode`] for the segments before the frame header and the header
/// itself; a JPEG without a frame is a syntax error.
pub fn header(data: &[u8]) -> Result<DctHeader> {
    let mut bytes = start(data)?;
    loop {
        let at = bytes.position();
        let code = read_marker(&mut bytes)?;
        if matches!(code, marker::RST0..=marker::EOI | marker::SOS | marker::TEM) {
            return Err(Error::syntax("JPEG has no frame").at(at));
        }
        let mut payload = segment(&mut bytes)?;
        if let Some((process, _)) = Process::of(code) {
            let header = Header::read(&mut payload, process).map_err(|e| e.at(at))?;
            return Ok(DctHeader {
                width: header.width,
                height: header.height,
                components: header.components,
            });
        }
    }
}

/// A cursor after the SOI marker (T.81 B.2.1).
fn start(data: &[u8]) -> Result<Cursor<'_>> {
    let mut bytes = Cursor::new(data).with_message("truncated JPEG data");
    if bytes.take_array().ok() != Some([marker::PREFIX, marker::SOI]) {
        return Err(Error::syntax("missing JPEG SOI").at(Position::Buffer(0)));
    }
    Ok(bytes)
}

/// The APP14 segment in which Adobe records its color transform (Table 13).
fn adobe(
    mut payload: Cursor<'_>,
    decoder: &mut Decoder,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let at = payload.position();
    // "Adobe", version, flags 0 and 1, then the transform.
    let transform = payload
        .take_array::<12>()
        .ok()
        .map(|known| known[11])
        .filter(|&transform| transform <= 2)
        .ok_or(Error::syntax("invalid Adobe JPEG marker"))?;
    if !payload.at_end() && !decoder.adobe_reported {
        diagnostics.repair(
            Diagnostic::new(
                crate::repairs::JPEG_ADOBE_MARKER,
                "extended Adobe JPEG marker uses its known prefix (7.4.8, Table 13)",
            )
            .at(at),
        )?;
        decoder.adobe_reported = true;
    }
    decoder.adobe = Some(transform);
    Ok(())
}

/// A DRI segment (T.81 B.2.4.4): the restart interval in MCUs.
fn restart_interval(mut payload: Cursor<'_>) -> Result<usize> {
    let interval = payload.u16();
    match interval {
        Ok(interval) if payload.at_end() => Ok(usize::from(interval)),
        _ => Err(Error::syntax("invalid JPEG restart interval length")),
    }
}

/// A DQT segment (T.81 B.2.4.1).
fn quant_tables(
    mut payload: Cursor<'_>,
    tables: &mut [Option<[u16; 64]>; frame::TABLES],
) -> Result<()> {
    while !payload.at_end() {
        let at = payload.position();
        let info = payload.u8()?;
        let table = tables
            .get_mut(usize::from(info & 15))
            .ok_or(Error::syntax("invalid JPEG quantization table index").at(at))?;
        let wide = match info >> 4 {
            0 => false,
            1 => true,
            _ => return Err(Error::syntax("invalid JPEG quantization precision").at(at)),
        };
        let mut quant = [0; 64];
        for q in &mut quant {
            *q = if wide {
                payload.u16()?
            } else {
                u16::from(payload.u8()?)
            };
            if *q == 0 {
                return Err(Error::syntax("zero JPEG quantizer").at(at));
            }
        }
        *table = Some(quant);
    }
    Ok(())
}

/// Reads a marker: a `0xFF` byte, optional `0xFF` fill and the marker code
/// (T.81 B.1.1.2).
fn read_marker(bytes: &mut Cursor<'_>) -> Result<u8> {
    let at = bytes.position();
    if bytes.u8()? != marker::PREFIX {
        return Err(Error::syntax("expected JPEG marker").at(at));
    }
    loop {
        let at = bytes.position();
        match bytes.u8()? {
            marker::PREFIX => {}
            marker::STUFFED => return Err(Error::syntax("stuffed byte outside JPEG scan").at(at)),
            code => return Ok(code),
        }
    }
}

/// Reads a marker segment's length and returns a cursor over its payload,
/// positioned in the whole input (T.81 B.1.1.4).
fn segment<'a>(bytes: &mut Cursor<'a>) -> Result<Cursor<'a>> {
    let at = bytes.position();
    let length = usize::from(bytes.u16()?);
    let payload = length
        .checked_sub(2)
        .ok_or(Error::syntax("invalid JPEG segment length").at(at))?;
    bytes.take_cursor(payload)
}

/// Locates the JPEG EOI without interpreting pixels; segment lengths protect
/// marker-like bytes in APP payloads, and scan stuffing protects entropy data.
///
/// # Errors
/// A syntax error when the data ends before EOI or has a marker that is not
/// valid at its place.
pub fn data_length(data: &[u8]) -> Result<usize> {
    let mut bytes = start(data)?;
    let mut scan = false;
    loop {
        if scan {
            // Entropy-coded data: skip data bytes, stuffed bytes and restarts.
            loop {
                let rest = bytes.rest();
                match rest {
                    [
                        marker::PREFIX,
                        marker::STUFFED | marker::RST0..=marker::RST7,
                        ..,
                    ] => {
                        bytes.skip(2)?;
                    }
                    [marker::PREFIX, ..] => break,
                    [_, ..] => bytes.skip(1)?,
                    [] => return Err(bytes.error("truncated inline JPEG scan")),
                }
            }
        }
        let at = bytes.position();
        let code = read_marker(&mut bytes)?;
        if code == marker::EOI {
            return Ok(bytes.offset());
        }
        if matches!(code, marker::RST0..=marker::SOI | marker::TEM) {
            return Err(Error::syntax("unexpected inline JPEG marker").at(at));
        }
        segment(&mut bytes)?;
        scan = code == marker::SOS;
    }
}
