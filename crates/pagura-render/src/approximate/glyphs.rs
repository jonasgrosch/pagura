//! Glyph coverage from prepared, vertically clipped edge fragments.
//!
//! Each outline is flattened once per linear map. Its edges are clipped to
//! rows once per vertical phase, and those fragments are reused across
//! horizontal phases, with area division done when a fragment is prepared.
//! Origins snap to a 1/128-pixel lattice horizontally and to the
//! resolution's lattice vertically; integer translation is applied when the
//! mask is painted. A mask is a pure function of its key, so retention and
//! eviction change time, never pixels.

use super::Resolution;
use super::coverage::{Coverage, Geometry};
use crate::hash::{FastMap, FastSet};
use pagura_core::{Error, Limits, Result};
use pagura_raster::{Path, Transform};
use std::sync::Arc;

/// Horizontal subpixel origins per device pixel.
const PHASES: u16 = 128;

/// Retained glyph masks, row fragments and geometry, bounded in bytes.
#[derive(Debug, Default)]
pub(crate) struct GlyphCache {
    masks: FastMap<Key, Mask>,
    rows: FastMap<Key, Rows>,
    geometries: FastMap<Key, Prepared>,
    outlines: FastSet<usize>,
    bytes: usize,
    maximum: usize,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Key {
    outline: usize,
    resolution: Resolution,
    matrix: [u64; 6],
}

impl Key {
    fn new(
        outline: &Arc<Path>,
        resolution: Resolution,
        transform: Transform,
        px: f64,
        py: f64,
    ) -> Self {
        Self {
            outline: Arc::as_ptr(outline) as usize,
            resolution,
            matrix: [transform.a, transform.b, transform.c, transform.d, px, py].map(f64::to_bits),
        }
    }
}

#[derive(Debug)]
struct Mask {
    // Retaining the outline prevents its address from being reused as a key.
    _outline: Arc<Path>,
    coverage: Arc<Coverage>,
}

impl GlyphCache {
    /// An empty cache retaining at most `maximum` bytes.
    pub(crate) fn new(maximum: usize) -> Self {
        Self {
            maximum,
            ..Self::default()
        }
    }

    /// The coverage of `outline` under `transform` at `resolution`, and the
    /// integer device offset at which to paint it. `scratch` is reused
    /// working storage; it never carries values between masks.
    pub(super) fn coverage(
        &mut self,
        outline: &Arc<Path>,
        (transform, resolution): (Transform, Resolution),
        limits: Limits,
        scratch: &mut Accumulator,
    ) -> Result<(Arc<Coverage>, (i32, i32))> {
        let (px, x) = phase(transform.e, PHASES)?;
        let (py, y) = phase(transform.f, resolution.vertical_phases())?;
        let key = Key::new(outline, resolution, transform, px, py);
        if let Some(mask) = self.masks.get(&key) {
            return Ok((Arc::clone(&mask.coverage), (x, y)));
        }
        let row_key = Key::new(outline, resolution, transform, 0.0, py);
        let coverage = if let Some(rows) = self.rows.get(&row_key) {
            rows.mask(px, limits, scratch)?
        } else {
            let geometry_key = Key::new(outline, resolution, transform, 0.0, 0.0);
            let rows = if let Some(prepared) = self.geometries.get(&geometry_key) {
                Rows::new(prepared, py, limits)?
            } else {
                let prepared = Prepared::new(outline, transform, resolution, limits)?;
                let rows = Rows::new(&prepared, py, limits)?;
                if self.reserve(outline, prepared.bytes, limits.document_bytes) {
                    self.geometries.insert(geometry_key, prepared);
                }
                rows
            };
            let coverage = rows.mask(px, limits, scratch)?;
            if self.reserve(outline, rows.bytes, limits.document_bytes) {
                self.rows.insert(row_key, rows);
            }
            coverage
        };
        let coverage = Arc::new(coverage);
        if self.reserve(
            outline,
            entry_bytes(outline, coverage.data.len()),
            limits.document_bytes,
        ) {
            self.masks.insert(
                key,
                Mask {
                    _outline: Arc::clone(outline),
                    coverage: Arc::clone(&coverage),
                },
            );
        }
        Ok((coverage, (x, y)))
    }

    fn clear(&mut self) {
        *self = Self::new(self.maximum);
    }

    // Each retained outline is charged once; a full cache starts again.
    fn reserve(&mut self, path: &Arc<Path>, bytes: usize, document_bytes: usize) -> bool {
        let maximum = self.maximum.min(document_bytes);
        let outline = path
            .segments()
            .len()
            .saturating_mul(size_of::<pagura_raster::Segment>());
        let storage = bytes.saturating_sub(outline);
        let source = Arc::as_ptr(path) as usize;
        let full = storage.saturating_add(outline).saturating_add(64);
        let mut charge = if self.outlines.contains(&source) {
            storage
        } else {
            full
        };
        if self.bytes.saturating_add(charge) > maximum {
            if full > maximum {
                return false;
            }
            self.clear();
            charge = full;
        }
        self.outlines.insert(source);
        self.bytes += charge;
        true
    }

    /// Bytes charged for retained masks, fragments, geometry and outlines.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

fn entry_bytes(path: &Path, storage: usize) -> usize {
    storage
        .saturating_add(
            path.segments()
                .len()
                .saturating_mul(size_of::<pagura_raster::Segment>()),
        )
        .saturating_add(256)
}

/// Splits a device coordinate into its fraction, snapped to `phases` per
/// pixel, and integer part.
fn phase(value: f64, phases: u16) -> Result<(f64, i32)> {
    let value = (value * f64::from(phases)).round() / f64::from(phases);
    if !value.is_finite() || value.abs() > 1e8 {
        return Err(Error::limit("glyph origin exceeds working range"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite coordinate range checked above"
    )]
    let integer = value.floor() as i32;
    Ok((value - f64::from(integer), integer))
}

/// Flattened edges in f32, relative to the glyph's integer extent.
#[derive(Debug)]
struct Prepared {
    outline: Arc<Path>,
    edges: Vec<[f32; 4]>,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    bytes: usize,
}

impl Prepared {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated geometry is normalized before f32 storage and extents are budgeted"
    )]
    fn new(
        path: &Arc<Path>,
        transform: Transform,
        resolution: Resolution,
        limits: Limits,
    ) -> Result<Self> {
        let geometry = Geometry::new(
            path,
            transform,
            resolution.glyph_flattening(),
            Limits {
                decoded_bytes: limits.decoded_bytes / 2,
                ..limits
            },
        )?;
        if geometry.min.x >= geometry.max.x || geometry.min.y >= geometry.max.y {
            return Ok(Self {
                outline: Arc::clone(path),
                edges: Vec::new(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                bytes: entry_bytes(path, 0),
            });
        }
        let x = geometry.min.x.floor() as i32;
        let y = geometry.min.y.floor() as i32;
        // One extra column and row hold the largest phase offset.
        let width = (geometry.max.x.ceil() - f64::from(x)) as usize + 1;
        let height = (geometry.max.y.ceil() - f64::from(y)) as usize + 1;
        width
            .checked_mul(height)
            .filter(|&n| n <= limits.pixels && n <= limits.raster_work)
            .ok_or(Error::limit("glyph extent budget exceeded"))?;
        let edges: Vec<_> = geometry
            .edges
            .iter()
            .map(|(a, b)| {
                [
                    (a.x - f64::from(x)) as f32,
                    (a.y - f64::from(y)) as f32,
                    (b.x - f64::from(x)) as f32,
                    (b.y - f64::from(y)) as f32,
                ]
            })
            .collect();
        let bytes = entry_bytes(path, edges.capacity().saturating_mul(size_of::<[f32; 4]>()));
        Ok(Self {
            outline: Arc::clone(path),
            edges,
            x,
            y,
            width,
            height,
            bytes,
        })
    }
}

/// One edge clipped to one row, with its area scale.
#[derive(Debug)]
struct Fragment {
    row: usize,
    left: f32,
    right: f32,
    scale: f32,
    height: f32,
}

/// The row fragments of a prepared glyph at one vertical phase.
#[derive(Debug)]
struct Rows {
    _outline: Arc<Path>,
    fragments: Vec<Fragment>,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    bytes: usize,
}

impl Rows {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "normalized finite geometry and checked raster extents bound row indices"
    )]
    fn new(glyph: &Prepared, py: f64, limits: Limits) -> Result<Self> {
        let mut fragments = Vec::new();
        let maximum = (limits.decoded_bytes / 2 / size_of::<Fragment>()).min(limits.raster_work);
        for &[mut ax, ay, mut bx, by] in &glyph.edges {
            let (mut ay, mut by) = (ay + py as f32, by + py as f32);
            let sign = if ay < by {
                1.0
            } else {
                std::mem::swap(&mut ax, &mut bx);
                std::mem::swap(&mut ay, &mut by);
                -1.0
            };
            if ay == by {
                continue;
            }
            let slope = (bx - ax) / (by - ay);
            for row in ay.floor().max(0.0) as usize..(by.ceil() as usize).min(glyph.height) {
                let low = ay.max(row as f32);
                let high = by.min(row as f32 + 1.0);
                let x0 = ax + slope * (low - ay);
                let x1 = ax + slope * (high - ay);
                if !x0.is_finite() || !x1.is_finite() {
                    return Err(Error::limit("glyph row arithmetic exceeded"));
                }
                if fragments.len() >= maximum {
                    return Err(Error::limit("glyph row fragment budget exceeded"));
                }
                if fragments.len() == fragments.capacity() {
                    let extra = fragments.len().max(16).min(maximum - fragments.len());
                    fragments
                        .try_reserve_exact(extra)
                        .map_err(|_| Error::limit("glyph row allocation failed"))?;
                }
                let left = x0.min(x1);
                let right = x0.max(x1);
                let height = (high - low) * sign;
                let scale = if right - left < 1e-6 {
                    0.0
                } else {
                    height / (right - left)
                };
                fragments.push(Fragment {
                    row,
                    left,
                    right,
                    scale,
                    height,
                });
            }
        }
        let bytes = entry_bytes(
            &glyph.outline,
            fragments.capacity().saturating_mul(size_of::<Fragment>()),
        );
        Ok(Self {
            _outline: Arc::clone(&glyph.outline),
            fragments,
            x: glyph.x,
            y: glyph.y,
            width: glyph.width,
            height: glyph.height,
            bytes,
        })
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "canonical phases lie within [0,1)"
    )]
    fn mask(&self, px: f64, limits: Limits, accumulator: &mut Accumulator) -> Result<Coverage> {
        if self.width == 0 {
            return Ok(Coverage::empty());
        }
        accumulator.reset(self.width, self.height, limits)?;
        let px = px as f32;
        for fragment in &self.fragments {
            accumulator.segment(
                fragment.row,
                fragment.left + px,
                fragment.right + px,
                fragment.height,
                fragment.scale,
            )?;
        }
        Ok(Coverage {
            x: self.x,
            y: self.y,
            width: self.width,
            data: accumulator.finish(),
        })
    }
}

/// Signed-area cells in f32, one sentinel per row; reusable working storage
/// for one mask at a time.
#[derive(Debug, Default)]
pub(super) struct Accumulator {
    area: Vec<f32>,
    width: usize,
    height: usize,
    work: usize,
    maximum: usize,
}

impl Accumulator {
    /// Zeroes `width` by `height` cells and the work count. The cells and the
    /// finished mask are charged together to the render's allocation budget,
    /// and capacity retained from a larger earlier mask is released when that
    /// budget no longer covers it.
    fn reset(&mut self, width: usize, height: usize, limits: Limits) -> Result<()> {
        let count = width
            .checked_mul(height)
            .filter(|&n| n <= limits.pixels && n <= limits.raster_work)
            .ok_or(Error::limit("glyph coverage extent budget exceeded"))?;
        let cells = count
            .checked_add(height)
            .ok_or(Error::limit("glyph coverage allocation budget exceeded"))?;
        let available = limits
            .decoded_bytes
            .checked_sub(count)
            .ok_or(Error::limit("glyph coverage allocation budget exceeded"))?
            / size_of::<f32>();
        if cells > available {
            return Err(Error::limit("glyph coverage allocation budget exceeded"));
        }
        if self.area.capacity() > available {
            self.area = Vec::new();
        }
        self.area.clear();
        self.area
            .try_reserve_exact(cells)
            .map_err(|_| Error::limit("glyph coverage allocation failed"))?;
        self.area.resize(cells, 0.0);
        self.width = width;
        self.height = height;
        self.work = 0;
        self.maximum = limits.raster_work;
        Ok(())
    }

    // Inlining lets the placement loop keep the row stride and work limit in
    // registers across fragments.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::inline_always,
        reason = "finite fragment endpoints saturate to the checked local raster extent; \
                  one caller per mask"
    )]
    #[inline(always)]
    fn segment(
        &mut self,
        row: usize,
        left: f32,
        right: f32,
        height: f32,
        scale: f32,
    ) -> Result<()> {
        let lo = (left as usize).min(self.width);
        let hi = (right.ceil() as usize).min(self.width);
        self.work = self.work.saturating_add(hi - lo + 1);
        if self.work > self.maximum {
            return Err(Error::limit("glyph coverage work budget exceeded"));
        }
        let row = &mut self.area[row * (self.width + 1)..(row + 1) * (self.width + 1)];
        if hi == lo + 1 && left >= lo as f32 && right <= hi as f32 {
            let value = (hi as f32 - left.midpoint(right)) * height;
            row[lo] += value;
            row[hi] += height - value;
            return Ok(());
        }
        let mut previous = 0.0;
        for (offset, cell) in row[lo..hi].iter_mut().enumerate() {
            let boundary = (lo + offset + 1) as f32;
            let value = if scale == 0.0 {
                (boundary - left.midpoint(right)).clamp(0.0, 1.0) * height
            } else {
                (primitive(boundary - left) - primitive(boundary - right)) * scale
            };
            *cell += value - previous;
            previous = value;
        }
        row[hi] += height - previous;
        Ok(())
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "coverage is clamped to [0,255] before positive nearest rounding"
    )]
    fn finish(&self) -> Vec<u8> {
        let level = |value: f32| (value.abs().min(1.0) * 255.0 + 0.5) as u8;
        let mut data = vec![0_u8; self.width * self.height];
        for (row, output) in self
            .area
            .chunks_exact(self.width + 1)
            .zip(data.chunks_exact_mut(self.width))
        {
            // The prefix sums stay serial, in the same order; only their
            // conversions to levels are independent, four at a time.
            let mut value = 0.0_f32;
            let (blocks, tail) = output.as_chunks_mut::<4>();
            let (deltas, rest) = row.split_at(blocks.len() * 4);
            for (out, delta) in blocks.iter_mut().zip(deltas.as_chunks::<4>().0) {
                let a = value + delta[0];
                let b = a + delta[1];
                let c = b + delta[2];
                value = c + delta[3];
                *out = [a, b, c, value].map(level);
            }
            for (out, &delta) in tail.iter_mut().zip(rest) {
                value += delta;
                *out = level(value);
            }
        }
        data
    }
}

fn primitive(value: f32) -> f32 {
    if value <= 0.0 {
        0.0
    } else if value < 1.0 {
        value * value * 0.5
    } else {
        value - 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::PathBuilder;

    fn accumulator(width: usize, height: usize) -> Accumulator {
        let mut raster = Accumulator::default();
        raster.reset(width, height, Limits::default()).unwrap();
        raster
    }

    #[test]
    fn phases_carry_across_integer_and_negative_origins() {
        assert_eq!(phase(3.999, 128).unwrap(), (0.0, 4));
        assert_eq!(phase(-0.25, 128).unwrap(), (0.75, -1));
        assert_eq!(phase(0.125, 128).unwrap(), (0.125, 0));
        assert_eq!(phase(0.97, 16).unwrap(), (0.0, 1));
        assert_eq!(phase(0.1, 16).unwrap(), (0.125, 0));
        assert!(phase(f64::INFINITY, 128).is_err());
    }

    #[test]
    fn block_and_tail_levels_match_independent_integer_prefix_areas() {
        let deltas = [513_i16, -1025, 511, 1024, -255, 1537, -2048, 1791];
        for width in 1..=33 {
            let mut raster = accumulator(width, 3);
            let mut expected = Vec::new();
            for row in 0..3 {
                let mut winding = 0_i32;
                for column in 0..width {
                    let delta = deltas[(row + column) % deltas.len()];
                    raster.area[row * (width + 1) + column] = f32::from(delta) / 1024.0;
                    winding += i32::from(delta);
                    let level = (winding.abs().min(1024) * 255 + 512) / 1024;
                    expected.push(u8::try_from(level).unwrap());
                }
            }
            assert_eq!(raster.finish(), expected);
        }
    }

    #[test]
    fn reused_scratch_is_zeroed_and_released_beyond_the_allocation_budget() {
        let mut raster = accumulator(32, 16);
        raster.area.fill(1.0);
        let limits = Limits {
            decoded_bytes: 32,
            ..Limits::default()
        };
        raster.reset(2, 2, limits).unwrap();
        assert!(raster.area.capacity() * size_of::<f32>() + 4 <= limits.decoded_bytes);
        assert_eq!(raster.finish(), [0; 4]);
        raster.segment(0, 0.5, 0.5, 1.0, 0.0).unwrap();
        raster.reset(2, 2, limits).unwrap();
        assert_eq!((raster.work, raster.finish()), (0, vec![0; 4]));
        let smaller = Limits {
            decoded_bytes: 27,
            ..limits
        };
        assert!(raster.reset(2, 2, smaller).is_err());
    }

    #[test]
    fn clipped_rectangle_phases_have_independent_rational_area() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let glyph = Prepared::new(
            &Arc::new(path.finish()),
            Transform::IDENTITY,
            Resolution::Low,
            Limits::default(),
        )
        .unwrap();
        let mut scratch = Accumulator::default();
        for py in [0_u16, 1, 16, 127] {
            let rows = Rows::new(&glyph, f64::from(py) / 128.0, Limits::default()).unwrap();
            for px in [0_u16, 1, 16, 127] {
                let mask = rows
                    .mask(f64::from(px) / 128.0, Limits::default(), &mut scratch)
                    .unwrap();
                for (index, a, b) in [
                    (0, 128 - px, 128 - py),
                    (1, px, 128 - py),
                    (2, 128 - px, py),
                    (3, px, py),
                ] {
                    assert_eq!(
                        mask.data[index],
                        u8::try_from((u32::from(a) * u32::from(b) * 255 + 8192) / 16384).unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn last_bit_and_half_coverage_have_independent_rectangle_areas() {
        let mut raster = accumulator(64, 3);
        for [ax, ay, bx, by] in [[0.25, 0.5, 0.25, 1.5], [63.75, 1.5, 63.75, 0.5_f32]] {
            // Each vertical edge crosses rows 0 and 1 by half a pixel.
            let sign = if ay < by { 1.0 } else { -1.0 };
            raster.segment(0, ax, bx, 0.5 * sign, 0.0).unwrap();
            raster.segment(1, ax, bx, 0.5 * sign, 0.0).unwrap();
        }
        let output = raster.finish();
        for row in output.as_chunks::<64>().0.iter().take(2) {
            assert_eq!(row[0], 96);
            assert_eq!(row[63], 96);
            assert!(row[1..63].iter().all(|&v| v == 128));
        }
        assert!(output[128..].iter().all(|&v| v == 0));
    }

    #[test]
    fn cache_hits_return_the_same_mask_and_eviction_keeps_pixels() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 3.0, 2.0);
        let path = Arc::new(path.finish());
        let transform = Transform::new(1.5, 0.0, 0.0, 1.5, 10.3, 4.7);
        let limits = Limits::default();
        let mut scratch = Accumulator::default();
        let mut cached = GlyphCache::new(1 << 20);
        let mut uncached = GlyphCache::new(0);
        for resolution in [Resolution::Low, Resolution::High] {
            let placement = (transform, resolution);
            let (first, offset) = cached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            let (again, _) = cached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            assert!(Arc::ptr_eq(&first, &again));
            let (fresh, fresh_offset) = uncached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            assert_eq!(
                (first.data.as_slice(), offset),
                (fresh.data.as_slice(), fresh_offset)
            );
        }
        // Resolutions snap 4.7 to different vertical lattices: a key per resolution.
        let low = cached.coverage(&path, (transform, Resolution::Low), limits, &mut scratch);
        let high = cached.coverage(&path, (transform, Resolution::High), limits, &mut scratch);
        assert_ne!(low.unwrap().0.data, high.unwrap().0.data);
        assert_eq!(uncached.retained_bytes(), 0);
        assert!(cached.retained_bytes() <= 1 << 20);
    }
}
