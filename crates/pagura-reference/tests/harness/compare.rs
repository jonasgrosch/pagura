//! Renderer output against the reference: every channel of every pixel
//! must lie within one level of the exact value (ADR 0004).

use pagura_raster::quantize_unit;
use pagura_reference::Rendering;

/// Slack for the reference's own f64 rounding, in levels.
const SLACK: f64 = 1e-6;

/// How far a rendering is from the reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// Pixels compared.
    pub pixels: usize,
    /// Pixels with a channel more than one level from the exact value.
    pub beyond: usize,
    /// Pixels with a channel that differs from the correctly rounded value.
    pub changed: usize,
    /// Channels equal to the correctly rounded value.
    pub matching: usize,
    /// Channels compared.
    pub channels: usize,
    /// The largest distance from the exact value, in levels.
    pub max_error: f64,
    /// Every channel that differs from the correctly rounded value, with
    /// its exact value (ADR 0004 §3).
    pub changes: Vec<Change>,
}

/// One channel that differs from its correctly rounded value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    /// Column.
    pub x: u32,
    /// Row.
    pub y: u32,
    /// Channel: red, green, blue, alpha.
    pub channel: usize,
    /// The renderer's byte.
    pub byte: u8,
    /// The exact value in levels.
    pub exact: f64,
}

impl Change {
    /// Whether the byte is more than one level from the exact value.
    pub fn beyond(&self) -> bool {
        (f64::from(self.byte) - self.exact).abs() > 1.0 + SLACK
    }
}

impl Comparison {
    /// Compares premultiplied RGBA8 `bytes`, row by row over the
    /// reference's region, with the reference.
    pub fn of(bytes: &[u8], reference: &Rendering) -> Self {
        let mut comparison = Self::default();
        let region = reference.region();
        let coordinates =
            (region.y0..region.y1).flat_map(|y| (region.x0..region.x1).map(move |x| (x, y)));
        for ((pixel, exact), (x, y)) in bytes
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference.values())
            .zip(coordinates)
        {
            comparison.pixels += 1;
            // A byte beyond one level is never the correctly rounded one.
            let (mut beyond, mut changed) = (false, false);
            for (channel, (&byte, &value)) in pixel.iter().zip(exact).enumerate() {
                let level = value * 255.0;
                let error = (f64::from(byte) - level).abs();
                comparison.max_error = comparison.max_error.max(error);
                comparison.channels += 1;
                if byte == quantize_unit(value) {
                    comparison.matching += 1;
                } else {
                    changed = true;
                    let change = Change {
                        x,
                        y,
                        channel,
                        byte,
                        exact: level,
                    };
                    beyond |= change.beyond();
                    comparison.changes.push(change);
                }
            }
            comparison.beyond += usize::from(beyond);
            comparison.changed += usize::from(changed);
        }
        comparison
    }

    /// The share of channels equal to the correctly rounded value.
    pub fn match_rate(&self) -> f64 {
        if self.channels == 0 {
            1.0
        } else {
            self.matching as f64 / self.channels as f64
        }
    }

    /// Adds another comparison's counts.
    pub fn add(&mut self, other: &Self) {
        self.pixels += other.pixels;
        self.beyond += other.beyond;
        self.changed += other.changed;
        self.matching += other.matching;
        self.channels += other.channels;
        self.max_error = self.max_error.max(other.max_error);
    }
}

/// RGB bytes of a premultiplied RGBA rendering composited on white, as the
/// fixture oracles hold them.
pub fn rgb_on_white(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.as_chunks::<4>().0 {
        let transparent = 255 - u32::from(pixel[3]);
        for &c in &pixel[..3] {
            rgb.push(u8::try_from((u32::from(c) + transparent).min(255)).unwrap_or(u8::MAX));
        }
    }
    rgb
}

/// The pixels of a binary PPM: width, height and RGB bytes.
pub fn parse_ppm(data: &[u8]) -> Option<(u32, u32, &[u8])> {
    let mut fields = Vec::new();
    let mut position = 0;
    while fields.len() < 4 {
        while data.get(position)?.is_ascii_whitespace() {
            position += 1;
        }
        if data.get(position) == Some(&b'#') {
            while *data.get(position)? != b'\n' {
                position += 1;
            }
            continue;
        }
        let start = position;
        while !data.get(position)?.is_ascii_whitespace() {
            position += 1;
        }
        fields.push(std::str::from_utf8(&data[start..position]).ok()?);
    }
    let (width, height) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    Some((width, height, data.get(position + 1..)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{Limits, Position, Rect};
    use pagura_interpret::{
        Composite, Group, GroupAttributes, GroupId, GroupRole, MarkInfo, PageSpace, Scene, StateId,
    };
    use pagura_raster::{DeviceRect, Transform};
    use std::sync::Arc;

    /// The reference rendering of an empty 2×1 page: white.
    fn blank() -> Rendering {
        let scene = Scene {
            page: PageSpace {
                crop: Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap(),
                rotation: pagura_document::Rotation::None,
                user_unit: 1.0,
            },
            print_intent: false,
            root: GroupId(0),
            nodes: Vec::new(),
            groups: vec![Group {
                role: GroupRole::Page,
                attributes: GroupAttributes::default(),
                bbox: None,
                view: None,
                children: Vec::new(),
                mark: MarkInfo {
                    state: StateId(0),
                    clip: None,
                    position: Position::Unknown,
                },
            }],
            masks: Vec::new(),
            tilings: Vec::new(),
            clips: Vec::new(),
            states: vec![Arc::new(Composite::default())],
            paints: Vec::new(),
            pens: Vec::new(),
            bytes: 0,
        };
        let region = DeviceRect {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 1,
        };
        let request = pagura_reference::Request::new(Transform::IDENTITY, region);
        pagura_reference::render(&scene, &request, &Limits::default()).unwrap()
    }

    #[test]
    fn comparisons_count_changed_and_beyond_channels() {
        let reference = blank();
        let mut bytes = reference.to_rgba8();
        assert_eq!(bytes, [255; 8]);
        let same = Comparison::of(&bytes, &reference);
        assert_eq!(
            (same.pixels, same.changed, same.beyond, same.match_rate()),
            (2, 0, 0, 1.0)
        );
        bytes[1] = 254;
        bytes[6] = 253;
        let changed = Comparison::of(&bytes, &reference);
        assert_eq!((changed.changed, changed.beyond), (2, 1));
        assert_eq!(changed.matching, 6);
        assert_eq!(
            changed.changes[1],
            Change {
                x: 1,
                y: 0,
                channel: 2,
                byte: 253,
                exact: 255.0
            }
        );
        assert!(changed.changes[1].beyond() && !changed.changes[0].beyond());
    }

    #[test]
    fn ppm_headers_and_white_compositing() {
        let ppm = b"P6\n# comment\n2 1\n255\n\x01\x02\x03\x04\x05\x06";
        assert_eq!(parse_ppm(ppm), Some((2, 1, &ppm[ppm.len() - 6..])));
        assert_eq!(parse_ppm(b"P5\n1 1\n255\n\x00"), None);
        // Premultiplied red at alpha 128 on white.
        assert_eq!(rgb_on_white(&[128, 0, 0, 128]), [255, 127, 127]);
    }
}
