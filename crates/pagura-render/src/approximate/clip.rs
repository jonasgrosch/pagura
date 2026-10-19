//! Rectangular clips and approximate coverage intersections, cached per clip.

use super::coverage::{self, Coverage};
use crate::display::Clip;
use pagura_core::{Error, Limits, Result};
use pagura_raster::{Path, Segment, Transform};
use std::{collections::HashMap, sync::Arc};

/// Clip-path flattening tolerance in device pixels.
const TOLERANCE: f64 = 0.25;

#[derive(Debug)]
pub(super) enum ClipMask {
    Rectangle([f64; 4]),
    Coverage(Coverage),
}

impl ClipMask {
    pub(super) fn covers(&self, x: i64, y: i64, width: usize, height: usize) -> bool {
        #[expect(
            clippy::cast_precision_loss,
            reason = "device coordinates are bounded far below 2^52"
        )]
        let (x, y, width, height) = (x as f64, y as f64, width as f64, height as f64);
        matches!(self, Self::Rectangle([x0, y0, x1, y1])
            if *x0 <= x && *y0 <= y && *x1 >= x + width && *y1 >= y + height)
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        reason = "coverage is clamped to 0..255 and indices checked nonnegative"
    )]
    pub(super) fn at(&self, x: i64, y: i64) -> u8 {
        match self {
            Self::Rectangle([x0, y0, x1, y1]) => {
                let dx = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).clamp(0.0, 1.0);
                let dy = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).clamp(0.0, 1.0);
                (dx * dy * 255.0).round() as u8
            }
            Self::Coverage(c) => {
                let x = x - i64::from(c.x);
                let y = y - i64::from(c.y);
                if x < 0 || y < 0 || x >= c.width as i64 {
                    0
                } else {
                    c.data
                        .get(y as usize * c.width + x as usize)
                        .copied()
                        .unwrap_or(0)
                }
            }
        }
    }
}

/// The device bounds of an axis-aligned rectangle path.
pub(super) fn rectangle(path: &Path, device: Transform) -> Option<[f64; 4]> {
    let [
        Segment::MoveTo(a),
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        Segment::Close,
    ] = path.segments()
    else {
        return None;
    };
    let [a, b, c, d] = [*a, *b, *c, *d].map(|p| p * device);
    if !((a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x))
    {
        return None;
    }
    Some([a.x.min(c.x), a.y.min(c.y), a.x.max(c.x), a.y.max(c.y)])
}

/// The mask of a clip chain over the whole page, so every tile shares it.
pub(super) fn clip_mask<'a>(
    clip: Option<&Arc<Clip>>,
    (device, page, limits): (Transform, (u32, u32), Limits),
    cache: &'a mut HashMap<usize, ClipMask>,
) -> Result<Option<&'a ClipMask>> {
    let Some(clip) = clip else {
        return Ok(None);
    };
    let identity = Arc::as_ptr(clip) as usize;
    if !cache.contains_key(&identity) {
        let maximum = limits.depth.min(limits.mask_sources);
        let mut bounds = [
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::INFINITY,
        ];
        let mut node = Some(clip.as_ref());
        let mut rectangles = true;
        let mut depth = 0;
        while let Some(current) = node {
            depth += 1;
            if depth > maximum {
                return Err(Error::limit("clip depth exceeded"));
            }
            let Some(r) = rectangle(&current.path, device) else {
                rectangles = false;
                break;
            };
            bounds = [
                bounds[0].max(r[0]),
                bounds[1].max(r[1]),
                bounds[2].min(r[2]),
                bounds[3].min(r[3]),
            ];
            node = current.parent.as_deref();
        }
        let mask = if rectangles {
            ClipMask::Rectangle(bounds)
        } else {
            let mut coverage =
                coverage::rasterize(&clip.path, device, clip.rule, limits, TOLERANCE, page)?;
            let mut parent = clip.parent.as_deref();
            let mut depth = 0;
            while let Some(node) = parent {
                depth += 1;
                if depth > maximum {
                    return Err(Error::limit("clip depth exceeded"));
                }
                let next =
                    coverage::rasterize(&node.path, device, node.rule, limits, TOLERANCE, page)?;
                intersect(&mut coverage, &next);
                parent = node.parent.as_deref();
            }
            ClipMask::Coverage(coverage)
        };
        let bytes = |c: &ClipMask| match c {
            ClipMask::Coverage(c) => c.data.len() + 64,
            ClipMask::Rectangle(_) => 64,
        };
        let retained: usize = cache
            .values()
            .map(bytes)
            .fold(0_usize, usize::saturating_add);
        if bytes(&mask) > limits.decoded_bytes {
            return Err(Error::limit("clip retention budget exceeded"));
        }
        if bytes(&mask).saturating_add(retained) > limits.decoded_bytes {
            cache.clear();
        }
        cache.insert(identity, mask);
    }
    Ok(cache.get(&identity))
}

#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "mask extents are bounded by rasterize and checked before indexing"
)]
fn intersect(a: &mut Coverage, b: &Coverage) {
    if a.width == 0 {
        return;
    }
    for (row, values) in a.data.chunks_exact_mut(a.width).enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            let x = a.x + column as i32 - b.x;
            let y = a.y + row as i32 - b.y;
            let other = if x >= 0 && y >= 0 && (x as usize) < b.width {
                b.data
                    .get(y as usize * b.width + x as usize)
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            *value =
                u8::try_from((u16::from(*value) * u16::from(other) + 127) / 255).unwrap_or(255);
        }
    }
}
