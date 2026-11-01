//! The previous renderer's output (the baseline goldens), for the report
//! ADR 0004 §3 asks of an output-changing change: every pixel that changed
//! against the golden, with its exact reference value, and the share of
//! changed pixels equal to the correctly rounded value before and after.
//!
//! `PAGURA_REFERENCE_GOLDENS` names the directory of exact fixture goldens,
//! `<area>/<name>-p0.ppm.gz` (gzip of a binary PPM on white).

use crate::compare::{parse_ppm, rgb_on_white};
use pagura_codec::{Predictor, decode_raw_deflate};
use pagura_core::{Diagnostics, Recovery};
use pagura_raster::quantize_unit;
use pagura_reference::Rendering;
use std::path::PathBuf;

/// The environment variable naming the goldens directory.
const GOLDENS: &str = "PAGURA_REFERENCE_GOLDENS";

/// The goldens directory, if one is named.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os(GOLDENS).map(PathBuf::from)
}

/// The golden RGB bytes of case `id` (`area/name`), if present and valid.
pub fn load(directory: &std::path::Path, id: &str) -> Option<(u32, u32, Vec<u8>)> {
    load_path(&directory.join(format!("{id}-p0.ppm.gz")))
}

/// The RGB bytes of a gzipped binary PPM, if present and valid.
pub fn load_path(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let ppm = gunzip(&data)?;
    let (width, height, rgb) = parse_ppm(&ppm)?;
    Some((width, height, rgb.to_vec()))
}

/// The members of a gzip file (RFC 1952) inflated.
fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    const FEXTRA: u8 = 4;
    const FNAME: u8 = 8;
    const FCOMMENT: u8 = 16;
    const FHCRC: u8 = 2;
    if data.get(..3)? != [0x1f, 0x8b, 8] {
        return None;
    }
    let flags = *data.get(3)?;
    let mut position = 10;
    if flags & FEXTRA != 0 {
        let length = usize::from(u16::from_le_bytes([
            *data.get(position)?,
            *data.get(position + 1)?,
        ]));
        position += 2 + length;
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            position += data.get(position..)?.iter().position(|&b| b == 0)? + 1;
        }
    }
    if flags & FHCRC != 0 {
        position += 2;
    }
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    decode_raw_deflate(
        data.get(position..)?,
        1 << 30,
        Predictor::default(),
        &mut diagnostics,
    )
    .ok()
}

/// How the renderer's output changed against the golden.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    /// Channels that differ from the golden: x, y, channel, golden,
    /// renderer, exact reference value in levels.
    pub changed: Vec<(u32, u32, usize, u8, u8, f64)>,
    /// Changed channels whose golden byte is correctly rounded.
    pub golden_matching: usize,
    /// Changed channels whose new byte is correctly rounded.
    pub new_matching: usize,
    /// Changed channels whose new byte is more than one level from exact.
    pub beyond: usize,
}

impl Changes {
    /// Compares the renderer's RGBA (on white) with the golden RGB, both
    /// against the reference.
    pub fn of(rendered: &[u8], golden: &[u8], reference: &Rendering) -> Self {
        let mut changes = Self::default();
        let region = reference.region();
        let new = rgb_on_white(rendered);
        let coordinates =
            (region.y0..region.y1).flat_map(|y| (region.x0..region.x1).map(move |x| (x, y)));
        for (((new, old), exact), (x, y)) in new
            .as_chunks::<3>()
            .0
            .iter()
            .zip(golden.as_chunks::<3>().0)
            .zip(reference.values())
            .zip(coordinates)
        {
            for channel in 0..3 {
                if new[channel] == old[channel] {
                    continue;
                }
                // On white the RGB byte is the premultiplied value plus the
                // background's share.
                let value = exact[channel] + (1.0 - exact[3]);
                let rounded = quantize_unit(value);
                changes.golden_matching += usize::from(old[channel] == rounded);
                changes.new_matching += usize::from(new[channel] == rounded);
                let level = value * 255.0;
                changes.beyond += usize::from((f64::from(new[channel]) - level).abs() > 1.0 + 1e-6);
                changes
                    .changed
                    .push((x, y, channel, old[channel], new[channel], level));
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_members_inflate() {
        // A gzip member holding one stored DEFLATE block of "P6".
        let mut data = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
        data.extend_from_slice(b"name\0");
        data.extend_from_slice(&[1, 2, 0, 0xfd, 0xff]);
        data.extend_from_slice(b"P6");
        data.extend_from_slice(&[0; 8]);
        assert_eq!(gunzip(&data).as_deref(), Some(&b"P6"[..]));
        assert_eq!(gunzip(b"not gzip"), None);
    }
}
