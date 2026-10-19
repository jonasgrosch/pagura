//! Separable area reconstruction in source row order, including quarter
//! turns: horizontal footprints per source row, then up to three cached
//! rows (or a sparse accumulation) per device row.
use super::{
    Placement, filled,
    footprint::{Band, Footprint, Tap},
    lanes::{self, GrayPlan},
};
use crate::{Pixmap, RenderOptions, interpreter::images::Image};
use pagura_core::{Error, Limits, Result};
use pagura_image::deferred::{Packed, Planar};

#[expect(
    clippy::cast_possible_truncation,
    reason = "footprint weights and byte-domain values are finite f32 values"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}
/// Horizontal footprints of the device columns: one band of taps per
/// column, and a structure-of-arrays copy for one-channel rows.
struct Plan {
    bands: Vec<Band>,
    taps: Vec<Tap>,
    gray: Option<GrayPlan>,
    work: usize,
}
impl Plan {
    fn new(
        range: std::ops::Range<usize>,
        scale: f64,
        offset: f64,
        maximum: usize,
        linear: bool,
        gray: bool,
        limits: Limits,
    ) -> Result<Self> {
        let mut work = range
            .len()
            .checked_mul(2)
            .filter(|&n| n <= limits.raster_work)
            .ok_or(Error::limit("image filter plan work exceeded"))?;
        let mut count = 0usize;
        for pixel in range.clone() {
            let span = Footprint::new(pixel, scale, offset, maximum, linear);
            count = count
                .checked_add(span.end - span.first)
                .ok_or(Error::limit("image filter tap overflow"))?;
            if count
                .checked_mul(if gray { 4 } else { 1 })
                .and_then(|n| n.checked_add(work))
                .is_none_or(|n| n > limits.raster_work)
            {
                return Err(Error::limit("image filter plan work exceeded"));
            }
        }
        work += count * if gray { 4 } else { 1 };
        let bytes = count
            .checked_mul(size_of::<Tap>())
            .and_then(|n| {
                range
                    .len()
                    .checked_mul(size_of::<Band>())
                    .and_then(|b| n.checked_add(b))
            })
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("image filter plan exceeded"))?;
        let mut plan = Self {
            bands: Vec::new(),
            taps: Vec::new(),
            gray: None,
            work,
        };
        plan.bands
            .try_reserve_exact(range.len())
            .map_err(|_| Error::limit("image filter allocation failed"))?;
        plan.taps
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("image filter allocation failed"))?;
        for pixel in range {
            let span = Footprint::new(pixel, scale, offset, maximum, linear);
            let start = plan.taps.len();
            for sample in span.first..span.end {
                plan.taps.push(Tap {
                    sample,
                    weight: narrow(span.weight(sample)),
                });
            }
            plan.bands.push(Band {
                start,
                end: plan.taps.len(),
            });
        }
        if gray {
            plan.gray = Some(
                GrayPlan::new(&plan.bands, &plan.taps, limits.decoded_bytes - bytes)
                    .ok_or(Error::limit("image gray plan exceeded"))?,
            );
        }
        Ok(plan)
    }
}
/// Horizontal filtering of source rows in any sample layout, with its work
/// charge. `N` is 1 for gray and 4 for RGB lanes.
struct Filter<'a, const N: usize> {
    samples: &'a [u8],
    planar: Option<&'a Planar>,
    packed: Option<&'a Packed>,
    rgb: Vec<u8>,
    gray: Vec<f32>,
    sparse: Option<super::sparse::Plan>,
    deficit: bool,
    binary: Option<super::binary::Plan>,
    width: usize,
    components: usize,
    plan: Plan,
    tables: Vec<[f32; 256]>,
    identity: bool,
    integer: bool,
    factor: f32,
    input: Vec<[f32; N]>,
    work: usize,
    maximum: usize,
}
impl<'a, const N: usize> Filter<'a, N> {
    fn new(
        image: &'a Image,
        samples: &'a [u8],
        width: usize,
        plan: Plan,
        limits: Limits,
    ) -> Result<Self> {
        let tables = filter_tables(image);
        let identity = tables.iter().all(|table| {
            table
                .iter()
                .enumerate()
                .all(|(i, &value)| value == f32::from(u8::try_from(i).unwrap_or(0)))
        });
        let binary = if matches!(image.decoded.samples, pagura_image::Samples::Packed(_)) {
            super::binary::Plan::new(&plan.bands, &plan.taps, tables[0][0], tables[0][1], limits)?
        } else {
            None
        };
        let initial_work = plan
            .work
            .checked_add(tables.len() * 256)
            .and_then(|n| n.checked_add(binary.as_ref().map_or(0, super::binary::Plan::work)))
            .filter(|&n| n <= limits.raster_work)
            .ok_or(Error::limit("image decode table work exceeded"))?;
        Ok(Self {
            samples,
            planar: if let pagura_image::Samples::Planar(p) = &image.decoded.samples {
                Some(p.as_ref())
            } else {
                None
            },
            packed: if let pagura_image::Samples::Packed(p) = &image.decoded.samples {
                Some(p)
            } else {
                None
            },
            rgb: filled(
                if matches!(
                    image.decoded.samples,
                    pagura_image::Samples::Planar(_) | pagura_image::Samples::Packed(_)
                ) {
                    width * usize::from(image.decoded.components)
                } else {
                    0
                },
                0,
            )?,
            gray: filled(plan.bands.len(), 0.0)?,
            binary,
            deficit: matches!(image.decoded.samples, pagura_image::Samples::Planar(_))
                && integer_samples(image),
            sparse: if matches!(image.decoded.samples, pagura_image::Samples::Planar(_)) {
                super::sparse::Plan::new(width, &plan.bands, &plan.taps, limits)?
            } else {
                None
            },
            width,
            components: usize::from(image.decoded.components),
            plan,
            tables,
            identity,
            integer: integer_samples(image),
            factor: 255.0 / f32::from((1u16 << image.decoded.bits_per_component) - 1),
            input: filled(if integer_samples(image) { 0 } else { width }, [0.0; N])?,
            work: initial_work,
            maximum: limits.raster_work,
        })
    }

    /// Filters source row `key` into `output`, one footprint per column.
    fn row(&mut self, key: usize, output: &mut [[f32; N]]) -> Result<()> {
        self.charge(self.width + self.plan.taps.len())?;
        if self.deferred_row(key, output)? {
            return Ok(());
        }
        let source = if self.planar.is_some() || self.packed.is_some() {
            self.rgb.as_slice()
        } else {
            &self.samples
                [key * self.width * self.components..(key + 1) * self.width * self.components]
        };
        if self.integer {
            if N == 1 && self.factor == 255.0 {
                super::binary::byte_row(
                    source,
                    &self.plan.bands,
                    &self.plan.taps,
                    output.as_flattened_mut().as_chunks_mut::<1>().0,
                );
            } else if N == 1 {
                if !self
                    .plan
                    .gray
                    .as_ref()
                    .is_some_and(|p| p.apply_bytes(source, self.factor, output.as_flattened_mut()))
                {
                    return Err(Error::limit("integer gray plan mismatch"));
                }
            } else {
                super::bytes::integer_row(
                    source,
                    &self.plan.bands,
                    &self.plan.taps,
                    self.factor,
                    output,
                );
            }
            return Ok(());
        }
        if N == 1 {
            if self.identity {
                for (out, &sample) in self.input.iter_mut().zip(source) {
                    out[0] = f32::from(sample);
                }
            } else {
                for (out, &sample) in self.input.iter_mut().zip(source) {
                    out[0] = self.tables[0][usize::from(sample)];
                }
            }
            if !self
                .plan
                .gray
                .as_ref()
                .is_some_and(|p| p.apply(self.input.as_flattened(), output.as_flattened_mut()))
            {
                return Err(Error::limit("image gray plan mismatch"));
            }
        } else {
            if self.components != 3 {
                return Err(Error::syntax("image RGB row mismatch"));
            }
            let pixels = self.input.as_flattened_mut().as_chunks_mut::<4>().0;
            if self.identity {
                for (out, sample) in pixels.iter_mut().zip(source.as_chunks::<3>().0) {
                    *out = [
                        f32::from(sample[0]),
                        f32::from(sample[1]),
                        f32::from(sample[2]),
                        255.0,
                    ];
                }
            } else {
                let [red, green, blue] = self
                    .tables
                    .first_chunk::<3>()
                    .ok_or(Error::syntax("image RGB tables missing"))?;
                for (out, sample) in pixels.iter_mut().zip(source.as_chunks::<3>().0) {
                    *out = [
                        red[usize::from(sample[0])],
                        green[usize::from(sample[1])],
                        blue[usize::from(sample[2])],
                        255.0,
                    ];
                }
            }
            lanes::horizontal(
                self.input.as_flattened().as_chunks::<4>().0,
                &self.plan.bands,
                &self.plan.taps,
                output.as_flattened_mut().as_chunks_mut::<4>().0,
            );
        }
        Ok(())
    }
    /// Filters a packed or planar row without expanding it, where possible;
    /// otherwise converts it into `rgb` for the byte paths.
    fn deferred_row(&mut self, key: usize, output: &mut [[f32; N]]) -> Result<bool> {
        if let Some(p) = self.packed {
            let row = p
                .row(key)
                .ok_or(Error::syntax("packed row outside source"))?;
            if N == 1
                && self
                    .binary
                    .as_ref()
                    .is_some_and(|p| p.row(row, output.as_flattened_mut()))
            {
                return Ok(true);
            }
            if N == 1 {
                super::binary::packed_row(
                    row,
                    &self.plan.bands,
                    &self.plan.taps,
                    self.tables[0][0],
                    self.tables[0][1],
                    output.as_flattened_mut(),
                );
                return Ok(true);
            }
            for (x, out) in self.rgb.iter_mut().enumerate() {
                *out = (row[x / 8] >> (7 - x % 8)) & 1;
            }
        }
        if let Some(p) = self.planar {
            if self.integer
                && N == 4
                && self.sparse.as_mut().is_some_and(|s| {
                    s.row(
                        p,
                        key,
                        &self.plan.bands,
                        &self.plan.taps,
                        output.as_flattened_mut().as_chunks_mut::<4>().0,
                    )
                })
            {
                return Ok(true);
            }
            if let Some(gray) = p.neutral(key).filter(|_| self.integer) {
                let values = &mut self.gray;
                super::bytes::row(gray, &self.plan.bands, &self.plan.taps, 1.0, values);
                for (out, v) in output.iter_mut().zip(values.iter().copied()) {
                    out.copy_from_slice(&[v, v, v, 255.0]);
                }
                return Ok(true);
            }
            if !p.rgb(key, &mut self.rgb) {
                return Err(Error::syntax("planar RGB mismatch"));
            }
        }
        Ok(false)
    }
    // Equal encoded rows have identical Decode-table values and identical
    // ordered horizontal sums. Keep the existing modulo-three destination;
    // sharing slots would overwrite rows still needed by a three-tap footprint.
    fn same_row(&mut self, a: usize, b: usize) -> Result<bool> {
        self.charge(self.width)?;
        if let Some(p) = self.planar {
            return Ok(p.same_row(a, b));
        }
        if let Some(p) = self.packed {
            return Ok(p.row(a) == p.row(b));
        }
        let stride = self
            .width
            .checked_mul(self.components)
            .ok_or(Error::limit("image row overflow"))?;
        let row = |key: usize| -> Result<&[u8]> {
            let start = key
                .checked_mul(stride)
                .ok_or(Error::limit("image row overflow"))?;
            let end = start
                .checked_add(stride)
                .ok_or(Error::limit("image row overflow"))?;
            self.samples
                .get(start..end)
                .ok_or(Error::syntax("image row outside samples"))
        };
        Ok(row(a)? == row(b)?)
    }
    fn charge(&mut self, count: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(count)
            .filter(|&n| n <= self.maximum)
            .ok_or(Error::limit("image filter work exceeded"))?;
        Ok(())
    }
}

/// Up to three filtered source rows, keyed by source row, and the device row
/// they combine into.
struct Rows<const N: usize> {
    cache: [Vec<[f32; N]>; 3],
    ranges: [Vec<std::ops::Range<usize>>; 3],
    keys: [Option<usize>; 3],
    sums: Vec<[f32; N]>,
    packed: Vec<[u8; 4]>,
}
impl<const N: usize> Rows<N> {
    fn new(length: usize) -> Result<Self> {
        Ok(Self {
            cache: [
                filled(length, [0.0; N])?,
                filled(length, [0.0; N])?,
                filled(length, [0.0; N])?,
            ],
            ranges: std::array::from_fn(|_| Vec::new()),
            keys: [None; 3],
            sums: filled(length, [0.0; N])?,
            packed: filled(length, [0; 4])?,
        })
    }
    fn reconstruct(
        &mut self,
        filter: &mut Filter<'_, N>,
        span: Footprint,
        reversed: bool,
    ) -> Result<()> {
        let length = self.packed.len();
        let count = span.end - span.first;
        let small = count <= 3 && !filter.deficit;
        if !small {
            self.sums.fill([0.0; N]);
        }
        let mut slots = [0; 3];
        let mut weights = [0.0; 3];
        for step in span.first..span.end {
            let key = if reversed {
                span.end - 1 - (step - span.first)
            } else {
                step
            };
            let slot = self
                .keys
                .iter()
                .position(|&k| k == Some(key))
                .unwrap_or(key % 3);
            if self.keys[slot] != Some(key) {
                let same = match self.keys[slot] {
                    Some(previous) => filter.same_row(key, previous)?,
                    None => false,
                };
                if !same {
                    filter.row(key, &mut self.cache[slot])?;
                    if filter.deficit {
                        let ranges = filter
                            .sparse
                            .as_ref()
                            .ok_or(Error::syntax("sparse row plan missing"))?
                            .ranges();
                        self.ranges[slot].clear();
                        self.ranges[slot]
                            .try_reserve_exact(ranges.len())
                            .map_err(|_| Error::limit("sparse range cache allocation failed"))?;
                        self.ranges[slot].extend_from_slice(ranges);
                    }
                }
                self.keys[slot] = Some(key);
            }
            filter.charge(length)?;
            if small {
                let i = step - span.first;
                slots[i] = slot;
                weights[i] = narrow(span.weight(key));
            } else if filter.deficit {
                for range in &self.ranges[slot] {
                    lanes::accumulate(
                        &self.cache[slot][range.clone()],
                        narrow(span.weight(key)),
                        &mut self.sums[range.clone()],
                    );
                }
            } else {
                lanes::accumulate(&self.cache[slot], narrow(span.weight(key)), &mut self.sums);
            }
        }
        if small {
            let selected = slots.map(|slot| self.cache[slot].as_slice());
            let success = if N == 1 {
                lanes::combine_gray(
                    selected.map(<[[f32; N]]>::as_flattened),
                    weights,
                    count,
                    &mut self.packed,
                )
            } else {
                lanes::combine_pack(
                    selected.map(|r| r.as_flattened().as_chunks::<4>().0),
                    weights,
                    count,
                    &mut self.packed,
                )
            };
            if !success {
                return Err(Error::syntax("image row reconstruction mismatch"));
            }
        } else if filter.deficit {
            for (out, pixel) in self.packed.iter_mut().zip(&self.sums) {
                *out = std::array::from_fn(|c| {
                    let v = 255.0 - pixel[c];
                    rounded_byte(v)
                });
            }
        } else if N == 1 {
            lanes::pack_gray(self.sums.as_flattened(), &mut self.packed);
        } else {
            lanes::pack(
                self.sums.as_flattened().as_chunks::<4>().0,
                &mut self.packed,
            );
        }
        Ok(())
    }
}

impl<const N: usize> Rows<N> {
    /// Mixes partly covered pixels of a deficit row with the background from
    /// their unrounded sums.
    fn cover_float(&mut self, row: usize, p: &Placement, bg: pagura_raster::Rgba8) {
        let [x0, x1, y0, y1] = p.extent;
        let (left, right, low, high, fixed) = if p.rotated {
            (p.range[2], p.range[3], y0, y1, super::overlap(row, x0, x1))
        } else {
            (p.range[0], p.range[1], x0, x1, super::overlap(row, y0, y1))
        };
        let apply = |this: &mut Self, index: usize| {
            let coverage = fixed * super::overlap(left + index, low, high);
            if coverage < 1.0 {
                for (c, under) in [bg.r, bg.g, bg.b].into_iter().enumerate() {
                    let value = 255.0 - f64::from(this.sums[index][c]);
                    this.packed[index][c] =
                        super::byte(value * coverage + f64::from(under) * (1.0 - coverage));
                }
            }
        };
        if fixed < 1.0 {
            for index in 0..right - left {
                apply(self, index);
            }
        } else if right > left {
            apply(self, 0);
            if right - left > 1 {
                apply(self, right - left - 1);
            }
        }
    }
}

fn integer_samples(image: &Image) -> bool {
    image.decode.iter().all(|&r| r == (0.0, 1.0))
        && (image.decoded.bits_per_component == 8 || image.decoded.components == 1)
}
fn filter_tables(image: &Image) -> Vec<[f32; 256]> {
    let maximum = f64::from((1u16 << image.decoded.bits_per_component) - 1);
    image
        .decode
        .iter()
        .map(|&(lo, hi)| {
            std::array::from_fn(|v| {
                narrow((lo + v as f64 / maximum * (hi - lo)).clamp(0.0, 1.0) * 255.0)
            })
        })
        .collect()
}

/// Paints `image` through its footprints, or through nearest samples when it
/// is aligned to the device grid or magnified without interpolation.
pub(super) fn paint(
    image: &Image,
    samples: &[u8],
    placement: &Placement,
    options: &RenderOptions,
    limits: Limits,
) -> Result<Pixmap> {
    let t = super::source_transform(image, placement)?;
    let aligned = if placement.rotated {
        (t.b.abs() - 1.0).abs() < 1e-9 && (t.c.abs() - 1.0).abs() < 1e-9
    } else {
        (t.a.abs() - 1.0).abs() < 1e-9 && (t.d.abs() - 1.0).abs() < 1e-9
    } && (t.e - t.e.round()).abs() < 1e-9
        && (t.f - t.f.round()).abs() < 1e-9;
    if aligned {
        if let pagura_image::Samples::Packed(p) = &image.decoded.samples {
            return packed_nearest(image, p, placement, options, limits);
        }
        if let pagura_image::Samples::Planar(p) = &image.decoded.samples {
            return planar_nearest(image, p, placement, options, limits);
        }
        return super::nearest(image, samples, placement, options, limits);
    }
    let minifying = t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9;
    if !minifying && !image.decoded.interpolate {
        if let pagura_image::Samples::Packed(p) = &image.decoded.samples {
            return packed_nearest(image, p, placement, options, limits);
        }
        if let pagura_image::Samples::Planar(p) = &image.decoded.samples {
            return planar_nearest(image, p, placement, options, limits);
        }
        return super::nearest(image, samples, placement, options, limits);
    }
    if image.decoded.components == 1 {
        render::<1>(image, samples, placement, options, limits, t, minifying)
    } else {
        render::<4>(image, samples, placement, options, limits, t, minifying)
    }
}

fn render<const N: usize>(
    image: &Image,
    samples: &[u8],
    placement: &Placement,
    options: &RenderOptions,
    limits: Limits,
    t: pagura_raster::Transform,
    minifying: bool,
) -> Result<Pixmap> {
    let [left, right, top, bottom] = placement.range;
    let (columns, rows, xs, xo, ys, yo) = if placement.rotated {
        (top..bottom, left..right, t.c, t.e, t.b, t.f)
    } else {
        (left..right, top..bottom, t.a, t.e, t.d, t.f)
    };
    let (sw, sh) = (image.decoded.width as usize, image.decoded.height as usize);
    let length = columns.len();
    let mut target = Pixmap::new_with_limits(
        placement.width,
        placement.height,
        options.background,
        limits,
    )?;
    let tile_length = if placement.rotated {
        length
            .checked_mul(16)
            .ok_or(Error::limit("image transpose storage overflow"))?
    } else {
        0
    };
    let remaining =
        temporary_budget::<N>(image, length, tile_length, sw, target.data().len(), limits)?;
    let plan = Plan::new(
        columns.clone(),
        xs,
        xo,
        sw,
        !minifying,
        N == 1,
        Limits {
            decoded_bytes: remaining,
            ..limits
        },
    )?;
    let mut filter = Filter::<N>::new(image, samples, sw, plan, limits)?;
    let mut output = Rows::<N>::new(length)?;
    let mut tile = filled(tile_length, [0u8; 4])?;
    for row in rows {
        output.reconstruct(
            &mut filter,
            Footprint::new(row, ys, yo, sh, !minifying),
            ys < 0.0,
        )?;
        if filter.deficit {
            output.cover_float(row, placement, options.background);
        }
        let packed = &output.packed;
        if !placement.rotated {
            let start = (row * placement.width as usize + left) * 4;
            let out = &mut target.data_mut()[start..start + length * 4];
            out.copy_from_slice(packed.as_flattened());
            if !filter.deficit {
                super::cover_row(out, left, row, placement.extent, options.background);
            }
            continue;
        }
        let slot = (row - left) % 16;
        tile[slot * length..(slot + 1) * length].copy_from_slice(packed);
        if slot == 15 || row + 1 == right {
            let start = row - slot;
            for (column, y) in columns.clone().enumerate() {
                let offset = (y * placement.width as usize + start) * 4;
                let out = &mut target.data_mut()[offset..offset + (slot + 1) * 4];
                for (i, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    *pixel = tile[i * length + column];
                }
                if !filter.deficit {
                    super::cover_row(out, start, y, placement.extent, options.background);
                }
            }
        }
    }
    Ok(target)
}

#[cfg(test)]
mod content_tests {
    use super::*;
    #[test]
    fn repeated_row_content_keeps_ordered_pixels_and_avoids_reconstruction() -> Result<()> {
        let mut samples = [40u8; 144];
        samples[24] = 51;
        samples[96] = 51;
        let limits = Limits::default();
        let plan = Plan::new(0..3, 8.0 / 3.0, 0.0, 8, false, false, limits)?;
        let tables = (0..3)
            .map(|c| {
                std::array::from_fn(|i| {
                    let value = f32::from(u8::try_from(i).unwrap_or(0));
                    match c {
                        0 => value * 0.5,
                        1 => 100.0 + value,
                        _ => 255.0 - value,
                    }
                })
            })
            .collect();
        let mut filter = Filter::<4> {
            samples: &samples,
            planar: None,
            packed: None,
            rgb: Vec::new(),
            gray: Vec::new(),
            sparse: None,
            deficit: false,
            binary: None,
            width: 8,
            components: 3,
            plan,
            tables,
            identity: false,
            integer: false,
            factor: 1.0,
            input: filled(8, [0.0; 4])?,
            work: 0,
            maximum: limits.raster_work,
        };
        let mut rows = Rows::<4>::new(3)?;
        let mut expected = Vec::new();
        for key in 0..3 {
            rows.reconstruct(&mut filter, Footprint::new(key, 1.0, 0.0, 6, false), false)?;
            expected.push(rows.packed.clone());
        }
        let first_work = filter.work;
        assert_eq!(expected[0], [[20, 140, 215, 255]; 3]);
        assert_ne!(expected[0], expected[1]);
        for key in 3..6 {
            rows.reconstruct(&mut filter, Footprint::new(key, 1.0, 0.0, 6, false), false)?;
            assert_eq!(rows.packed, expected[key - 3]);
        }
        assert!(filter.work - first_work < first_work);
        filter.maximum = filter.work;
        assert!(filter.same_row(0, 3).is_err());
        Ok(())
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn byte_decode_specialization_matches_exact_identity_tables() -> Result<()> {
        let limits = Limits::default();
        let samples: Vec<u8> = (0..201u16)
            .map(|i| u8::try_from((i * 73) % 256).unwrap_or(0))
            .collect();
        let make = |identity| -> Result<Filter<'_, 4>> {
            Ok(Filter {
                samples: &samples,
                planar: None,
                packed: None,
                rgb: Vec::new(),
                gray: Vec::new(),
                sparse: None,
                deficit: false,
                binary: None,
                width: 67,
                components: 3,
                plan: Plan::new(0..37, 67.0 / 37.0, 0.0, 67, false, false, limits)?,
                tables: vec![std::array::from_fn(|i| f32::from(u8::try_from(i).unwrap_or(0))); 3],
                identity,
                integer: false,
                factor: 1.0,
                input: filled(67, [0.0; 4])?,
                work: 0,
                maximum: limits.raster_work,
            })
        };
        let mut a = make(false)?;
        let mut b = make(true)?;
        let mut lookup = [[0.0; 4]; 37];
        let mut direct = [[0.0; 4]; 37];
        a.row(0, &mut lookup)?;
        b.row(0, &mut direct)?;
        assert_eq!(lookup, direct);
        Ok(())
    }
}

fn planar_nearest(
    image: &Image,
    p: &Planar,
    placement: &Placement,
    options: &RenderOptions,
    limits: Limits,
) -> Result<Pixmap> {
    if !placement.rotated {
        return super::nearest_rows(image, placement, options, limits, |y, row| p.rgb(y, row));
    }
    let limits = materialization_limits(image, placement, limits)?;
    let length = p
        .width()
        .checked_mul(p.height())
        .and_then(|n| n.checked_mul(3))
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or(Error::limit("planar materialization budget exceeded"))?;
    let mut samples = filled(length, 0u8)?;
    for (y, row) in samples.chunks_exact_mut(p.width() * 3).enumerate() {
        if !p.rgb(y, row) {
            return Err(Error::syntax("planar RGB mismatch"));
        }
    }
    super::nearest(image, &samples, placement, options, limits)
}

fn packed_nearest(
    image: &Image,
    p: &Packed,
    placement: &Placement,
    options: &RenderOptions,
    limits: Limits,
) -> Result<Pixmap> {
    if !placement.rotated {
        return super::nearest_rows(image, placement, options, limits, |y, out| {
            let Some(source) = p.row(y) else {
                return false;
            };
            for (x, v) in out.iter_mut().enumerate() {
                *v = (source[x / 8] >> (7 - x % 8)) & 1;
            }
            true
        });
    }
    let limits = materialization_limits(image, placement, limits)?;
    let length = p
        .width()
        .checked_mul(p.height())
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or(Error::limit("packed expansion budget exceeded"))?;
    let mut values = filled(length, 0u8)?;
    for (y, out) in values.chunks_exact_mut(p.width()).enumerate() {
        let source = p.row(y).ok_or(Error::syntax("packed row outside source"))?;
        for (x, v) in out.iter_mut().enumerate() {
            *v = (source[x / 8] >> (7 - x % 8)) & 1;
        }
    }
    super::nearest(image, &values, placement, options, limits)
}

fn materialization_limits(image: &Image, p: &Placement, limits: Limits) -> Result<Limits> {
    let remaining = limits
        .decoded_bytes
        .checked_sub(pagura_image::deferred::storage_bytes(
            &image.decoded.samples,
        ))
        .ok_or(Error::limit("deferred image storage exceeded"))?;
    let width = p.range[1] - p.range[0];
    let storage = (image.decoded.width as usize)
        .checked_mul(image.decoded.height as usize)
        .and_then(|n| n.checked_mul(usize::from(image.decoded.components)))
        .and_then(|n| {
            (p.width as usize)
                .checked_mul(p.height as usize)
                .and_then(|v| v.checked_mul(4))
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| {
            width
                .checked_mul(size_of::<usize>() + 4)
                .and_then(|v| n.checked_add(v))
        })
        .and_then(|n| n.checked_add(4096))
        .filter(|&n| n <= remaining);
    if storage.is_none() {
        return Err(Error::limit("deferred image materialization exceeded"));
    }
    Ok(Limits {
        decoded_bytes: remaining,
        ..limits
    })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "finite rounded channel is clamped to the byte domain"
)]
fn rounded_byte(value: f32) -> u8 {
    ((value + 0.5).clamp(0.0, 255.0) as i32).to_le_bytes()[0]
}

fn temporary_budget<const N: usize>(
    image: &Image,
    length: usize,
    tile_length: usize,
    sw: usize,
    target_bytes: usize,
    limits: Limits,
) -> Result<usize> {
    let temporary = length
        .checked_mul(4)
        .and_then(|n| n.checked_add(sw))
        .and_then(|n| n.checked_mul(N * size_of::<f32>()))
        .and_then(|n| {
            length
                .checked_add(tile_length)
                .and_then(|r| r.checked_mul(4))
                .and_then(|r| n.checked_add(r))
        })
        .and_then(|n| {
            image
                .decode
                .len()
                .checked_mul(size_of::<[f32; 256]>())
                .and_then(|b| n.checked_add(b))
        })
        .and_then(|n| {
            n.checked_add(pagura_image::deferred::storage_bytes(
                &image.decoded.samples,
            ))
        })
        .and_then(|n| {
            // Component conversion, sparse runs, row-range caches and at most
            // 64 packed phase tables coexist with the separable plan.
            if matches!(
                image.decoded.samples,
                pagura_image::Samples::Planar(_) | pagura_image::Samples::Packed(_)
            ) {
                sw.checked_mul(64)
                    .and_then(|a| length.checked_mul(64).and_then(|b| a.checked_add(b)))
                    .and_then(|b| b.checked_add(131_072))
                    .and_then(|b| n.checked_add(b))
            } else {
                Some(n)
            }
        })
        .and_then(|n| n.checked_add(target_bytes))
        .ok_or(Error::limit("image filter storage overflow"))?;
    limits
        .decoded_bytes
        .checked_sub(temporary)
        .ok_or(Error::limit("image filter storage exceeded"))
}
