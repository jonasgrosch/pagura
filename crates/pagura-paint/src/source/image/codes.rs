//! Area averages, bilinear interpolation and nearest samples of codes
//! (8.9.5.3, ADR 0004), for upright placements of images whose texel lanes
//! each follow one component's code affinely: device colour spaces
//! converted as the identity, through any decode array that stays within
//! the colour range, and every lane of a one-bit image. Quarter turns are
//! sampled upright from the image with rows and columns exchanged.
//!
//! The exact area average of such texels is the same affine function of the
//! exact area average of the codes. The codes of a footprint's interior add
//! up as integers, so a footprint costs one integer addition per sample and
//! a few weighted terms at its edges: a device row first reduces its source
//! rows for every source column it spans, then each pixel reduces its
//! columns. The result differs from adding weighted texels one by one in f64
//! only by the rounding of the few f64 operations at the edges and of the
//! final scaling, about 1e-15 of the value, far below the output
//! quantizer's tie guard (1e-9 of a level). Nearest samples look up the
//! texel of their code, exactly as the general sampler does.

use super::geometry::{Footprint, Geometry, MAX_LANES, Mode};
use crate::SpanColors;
use pagura_core::{Error, Result};
use pagura_image::deferred::{Packed, Planar};
use pagura_image::{DecodedImage, Samples};
use std::sync::{Mutex, TryLockError};

/// The largest deviation from an affine function, relative to the value,
/// that a texel table may show and still count as affine: a few roundings
/// of the conversion's f64 arithmetic.
const AFFINE_TOLERANCE: f64 = 1e-14;

/// Converted rows of planar or packed samples kept for the next spans.
const CACHED_ROWS: usize = 8;

/// A lane's value as an affine function of a code: `zero + code × slope`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Affine {
    zero: f64,
    slope: f64,
    /// The largest code, where the map takes codes onto `0..=1` (`zero` 0,
    /// `slope` `1 / maximum`); then a code that is exact in f64 stays exact
    /// as the value that multiplies back to it.
    unit: Option<f64>,
    /// Levels of 255 per code on a unit map: `255 / maximum`.
    level: f64,
}

impl Affine {
    /// The values `zero + code × slope` for codes `0..=maximum`.
    fn new(zero: f64, slope: f64, maximum: f64) -> Self {
        let unit = (zero == 0.0 && slope * maximum == 1.0).then_some(maximum);
        Self {
            zero,
            slope,
            unit,
            level: unit.map_or(0.0, |maximum| 255.0 / maximum),
        }
    }

    /// The value of `code`, which may be an average. On a unit map it is
    /// the product unless that does not multiply back to the code, and
    /// then the quotient, so that a level of 255 such as `k + 1/2` from an
    /// exact average reaches storage exact, and with it its rounding.
    #[inline]
    fn apply(self, code: f64) -> f64 {
        let value = code * self.slope;
        match self.unit {
            Some(maximum) if value * maximum != code => code / maximum,
            Some(_) => value,
            None => self.zero + value,
        }
    }
}

impl Affine {
    /// The bytes of `codes` on a unit map, as [`Self::byte`] gives them away
    /// from rounding boundaries, into `out`, in one pass the compiler
    /// vectorizes. Returns whether a level came near a boundary; `out` then
    /// needs the colour values rounded.
    fn levels(self, codes: &[f64], out: &mut [u8]) -> bool {
        const LANES: usize = 8;
        let (scale, offset) = (self.level, 0.5 + TIE_GUARD);
        // Levels within 0..256 truncate to their floor; a level whose
        // fraction is within NEAR of a whole number is near a boundary.
        let level = |code: f64| {
            let level = code * scale + offset;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a level of an average code, within 0..256"
            )]
            let floor = level as u32;
            let fraction = level - f64::from(floor);
            let near = u32::from(fraction <= BOUNDARY_NEAR || fraction >= 1.0 - BOUNDARY_NEAR);
            (floor.min(255), near)
        };
        let mut near = 0;
        let (chunks, codes) = codes.as_chunks::<LANES>();
        let (outs, out) = out.as_chunks_mut::<LANES>();
        for (out, chunk) in outs.iter_mut().zip(chunks) {
            let mut flags = [0u32; LANES];
            for ((byte, flag), &code) in out.iter_mut().zip(&mut flags).zip(chunk) {
                let (value, close) = level(code);
                *byte = value.to_le_bytes()[0];
                *flag = close;
            }
            near |= flags.iter().fold(0, |a, &b| a | b);
        }
        for (byte, &code) in out.iter_mut().zip(codes) {
            let (value, close) = level(code);
            *byte = value.to_le_bytes()[0];
            near |= close;
        }
        near != 0
    }

    /// The byte [`pagura_raster::quantize_unit`] gives [`Self::apply`] of
    /// `code`. On a unit map the level is the code scaled by `255 /
    /// maximum`, which is within 3e-13 of the level the colour value
    /// multiplies back to; away from a rounding boundary by more than
    /// 1e-12 the scaled code rounds alike, and near one the colour value
    /// itself is rounded.
    #[inline]
    fn byte(self, code: f64) -> u8 {
        const NEAR: f64 = BOUNDARY_NEAR;
        if self.unit.is_some() {
            let level = code * self.level + 0.5 + TIE_GUARD;
            let fraction = level - level.floor();
            if fraction > NEAR && fraction < 1.0 - NEAR && (0.0..256.0).contains(&level) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a level within 0..256"
                )]
                let byte = level as u8;
                return byte;
            }
        }
        pagura_raster::quantize_unit(self.apply(code))
    }
}

/// The tie guard of [`pagura_raster::quantize_unit`].
const TIE_GUARD: f64 = 1e-9;

/// How near a level computed from a code may come to a rounding boundary
/// and still round as the colour value does: the two differ by less than
/// 3e-13 of a level.
const BOUNDARY_NEAR: f64 = 1e-12;

/// The map of averages already in colour values.
const IDENTITY: Affine = Affine {
    zero: 0.0,
    slope: 1.0,
    unit: None,
    level: 0.0,
};

/// How each texel lane follows the codes.
#[derive(Debug, Clone)]
pub(super) struct Lanes {
    /// Per lane: the component whose code it follows.
    component: Vec<usize>,
    /// Per lane: the texel value of every code.
    table: Vec<[f64; 256]>,
    /// Per lane: its value of a code.
    affine: Vec<Affine>,
    /// Per lane: the values at code zero and at the largest code.
    ends: Vec<(f64, f64)>,
    /// The largest code.
    maximum: u8,
    /// The value of a code shared by every lane of opaque lanes that follow
    /// their own component.
    direct: Option<Affine>,
    /// The same, shared by opaque lanes that all follow the one component.
    replicated: Option<Affine>,
    /// Colour channels: all lanes when opaque, all but alpha and shape
    /// otherwise.
    channels: usize,
    /// Whether every texel has alpha and shape 1; the lanes are then the
    /// colour channels alone.
    opaque: bool,
}

impl Lanes {
    /// The lanes of a one-component image of `codes` codes whose texels are
    /// `table`, `lanes` values per code (colours, alpha and shape), or
    /// `None` when a lane is not affine in the code.
    pub(super) fn single(table: &[f64], lanes: usize, codes: usize, opaque: bool) -> Option<Self> {
        if !(2..=256).contains(&codes) || lanes < 2 || table.len() != codes * lanes {
            return None;
        }
        let kept = if opaque { lanes - 2 } else { lanes };
        let columns: Vec<Vec<f64>> = (0..kept)
            .map(|lane| table.iter().skip(lane).step_by(lanes).copied().collect())
            .collect();
        Self::new(&columns, &vec![0; kept], lanes - 2, opaque)
    }

    /// The lanes of an opaque three-component image of eight-bit codes whose
    /// channel `c` is `tables[c]` of component `c`.
    pub(super) fn rgb(tables: &[[f64; 256]; 3]) -> Option<Self> {
        let columns: Vec<Vec<f64>> = tables.iter().map(|table| table.to_vec()).collect();
        Self::new(&columns, &[0, 1, 2], 3, true)
    }

    fn new(
        columns: &[Vec<f64>],
        component: &[usize],
        channels: usize,
        opaque: bool,
    ) -> Option<Self> {
        let mut table = Vec::with_capacity(columns.len());
        let mut affine = Vec::with_capacity(columns.len());
        let mut ends = Vec::with_capacity(columns.len());
        let maximum = u8::try_from(columns.first()?.len().checked_sub(1)?).ok()?;
        for column in columns {
            let (&zero, &top) = (column.first()?, column.last()?);
            let maximum = f64::from(u32::try_from(column.len().checked_sub(1)?).ok()?);
            let slope = (top - zero) / maximum;
            let straight = column.iter().zip(0u32..).all(|(&value, code)| {
                let expected = zero + f64::from(code) * slope;
                (value - expected).abs() <= AFFINE_TOLERANCE * value.abs().max(1.0)
            });
            if !straight || !slope.is_finite() {
                return None;
            }
            let mut entries = [0.0; 256];
            entries.get_mut(..column.len())?.copy_from_slice(column);
            table.push(entries);
            affine.push(Affine::new(zero, slope, maximum));
            ends.push((zero, top));
        }
        let own = component.iter().enumerate().all(|(lane, &c)| lane == c);
        let single = component.iter().all(|&c| c == 0);
        let shared = affine.windows(2).all(|pair| pair[0] == pair[1]);
        let direct = (opaque && own && shared)
            .then(|| affine.first().copied())
            .flatten();
        let replicated = (opaque && single && shared)
            .then(|| affine.first().copied())
            .flatten();
        Some(Self {
            component: component.to_vec(),
            table,
            affine,
            ends,
            maximum,
            direct,
            replicated,
            channels,
            opaque,
        })
    }

    /// The complements of codes averaged beside them, for lanes that are
    /// not opaque: the largest code. Their averages make each lane the
    /// weighted mean of its values at the two ends, so a lane whose value
    /// is zero wherever the footprint is stays exactly zero, as alpha must
    /// for its colours to be unpremultiplied.
    fn complement(&self) -> Option<u8> {
        (!self.opaque).then_some(self.maximum)
    }

    /// Writes the span's values from its pixels' average codes, `n` per
    /// pixel (and their complements' averages, unless opaque): straight
    /// colours and, unless opaque, alpha and shape.
    fn write_averages(
        &self,
        averages: &[f64],
        n: usize,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        if let Some(map) = self.direct.filter(|_| n == self.table.len()) {
            out.colors.clear();
            out.colors
                .extend(averages.iter().map(|&average| map.apply(average)));
            opaque_shape(averages.len() / n, shape);
            return;
        }
        let fast = self.opaque
            && match (n, self.table.len()) {
                (1, 1) => self.affine_opaque::<1, 1>(averages, &mut out.colors),
                (1, 3) => self.affine_opaque::<1, 3>(averages, &mut out.colors),
                (1, 4) => self.affine_opaque::<1, 4>(averages, &mut out.colors),
                (3, 3) => self.affine_opaque::<3, 3>(averages, &mut out.colors),
                _ => false,
            };
        if fast {
            opaque_shape(averages.len() / n, shape);
            return;
        }
        if !self.opaque {
            let maximum = f64::from(self.maximum);
            let pairs = averages.as_chunks::<2>().0.iter().map(<[f64; 2]>::as_slice);
            self.emit(pairs, out, shape, |averages, values| {
                let (code, complement) = (averages[0] / maximum, averages[1] / maximum);
                for (value, &(zero, top)) in values.iter_mut().zip(&self.ends) {
                    *value = zero * complement + top * code;
                }
            });
            return;
        }
        self.emit(averages.chunks_exact(n), out, shape, |codes, values| {
            for ((value, &component), map) in
                values.iter_mut().zip(&self.component).zip(&self.affine)
            {
                *value = map.apply(codes.get(component).copied().unwrap_or(0.0));
            }
        });
    }

    /// The colours of opaque pixels from their average codes, `N` per pixel,
    /// for `L` lanes; false when the lanes are not of that shape.
    fn affine_opaque<const N: usize, const L: usize>(
        &self,
        averages: &[f64],
        colors: &mut Vec<f64>,
    ) -> bool {
        let Some(component) = self.lane_components::<N, L>() else {
            return false;
        };
        let maps: [Affine; L] = std::array::from_fn(|l| self.affine[l]);
        let averages = averages.as_chunks::<N>().0;
        colors.clear();
        colors.resize(averages.len() * L, 0.0);
        for (values, average) in colors.as_chunks_mut::<L>().0.iter_mut().zip(averages) {
            for ((value, &component), map) in values.iter_mut().zip(&component).zip(&maps) {
                *value = map.apply(average[component]);
            }
        }
        true
    }

    /// The colours of opaque pixels from their codes, `N` per pixel, for
    /// `L` lanes; false when the lanes are not of that shape.
    fn table_opaque<const N: usize, const L: usize>(
        &self,
        codes: &[u8],
        colors: &mut Vec<f64>,
    ) -> bool {
        let Some(component) = self.lane_components::<N, L>() else {
            return false;
        };
        let tables: [&[f64; 256]; L] = std::array::from_fn(|l| &self.table[l]);
        let codes = codes.as_chunks::<N>().0;
        colors.clear();
        colors.resize(codes.len() * L, 0.0);
        for (values, sample) in colors.as_chunks_mut::<L>().0.iter_mut().zip(codes) {
            for ((value, &component), table) in values.iter_mut().zip(&component).zip(&tables) {
                *value = table[usize::from(sample[component])];
            }
        }
        true
    }

    /// Each lane's component when there are `L` lanes of components below
    /// `N`.
    fn lane_components<const N: usize, const L: usize>(&self) -> Option<[usize; L]> {
        let component: [usize; L] = self.component.as_slice().try_into().ok()?;
        component.iter().all(|&c| c < N).then_some(component)
    }

    /// Marks the pixels that `covered` excludes as outside the image:
    /// colour, alpha and shape 0, as an empty footprint sums them.
    fn uncover(&self, covered: &[bool], out: &mut SpanColors, shape: Option<&mut Vec<f64>>) {
        if covered.iter().all(|&covered| covered) {
            return;
        }
        let channels = self.channels;
        if self.opaque {
            out.alpha.clear();
            out.alpha
                .extend(covered.iter().map(|&covered| f64::from(u8::from(covered))));
        }
        for (index, _) in covered.iter().enumerate().filter(|(_, covered)| !**covered) {
            let start = index * channels;
            if let Some(colors) = out.colors.get_mut(start..start + channels) {
                colors.fill(0.0);
            }
            if let Some(alpha) = out.alpha.get_mut(index) {
                *alpha = 0.0;
            }
        }
        if let Some(shape) = shape {
            if self.opaque {
                shape.clone_from(&out.alpha);
            } else {
                for (shape, &covered) in shape.iter_mut().zip(covered) {
                    if !covered {
                        *shape = 0.0;
                    }
                }
            }
        }
    }

    /// Writes the span's values from its pixels' codes, `n` per pixel.
    fn write_codes(
        &self,
        codes: &[u8],
        n: usize,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        if let Some(map) = self.direct.filter(|_| n == self.table.len()) {
            // Every code becomes its colour value alike.
            out.colors.clear();
            out.colors
                .extend(codes.iter().map(|&code| map.apply(f64::from(code))));
            opaque_shape(codes.len() / n, shape);
            return;
        }
        let fast = self.opaque
            && match (n, self.table.len()) {
                (1, 1) => self.table_opaque::<1, 1>(codes, &mut out.colors),
                (1, 3) => self.table_opaque::<1, 3>(codes, &mut out.colors),
                (1, 4) => self.table_opaque::<1, 4>(codes, &mut out.colors),
                (3, 3) => self.table_opaque::<3, 3>(codes, &mut out.colors),
                _ => false,
            };
        if fast {
            opaque_shape(codes.len() / n, shape);
            return;
        }
        self.emit(codes.chunks_exact(n), out, shape, |codes, values| {
            for ((value, &component), table) in
                values.iter_mut().zip(&self.component).zip(&self.table)
            {
                *value = table[usize::from(codes.get(component).copied().unwrap_or(0))];
            }
        });
    }

    /// Writes each pixel's lanes, as `texel` gives them, as straight colours
    /// with alpha and shape.
    fn emit<'c, T: 'c>(
        &self,
        pixels: impl Iterator<Item = &'c [T]>,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
        texel: impl Fn(&[T], &mut [f64]),
    ) {
        let channels = self.channels;
        let (lower, _) = pixels.size_hint();
        out.colors.clear();
        out.colors.resize(lower * channels, 0.0);
        if self.opaque {
            for (codes, colors) in pixels.zip(out.colors.chunks_exact_mut(channels)) {
                texel(codes, colors);
            }
            opaque_shape(lower, shape);
            return;
        }
        let mut values = [0.0; MAX_LANES];
        let values = &mut values[..self.table.len()];
        let mut shape = shape;
        if let Some(shape) = shape.as_deref_mut() {
            shape.clear();
        }
        out.alpha.clear();
        for (index, codes) in pixels.enumerate() {
            texel(codes, values);
            let (premultiplied, rest) = values.split_at(channels);
            let (alpha, own) = (rest[0], rest[1]);
            let start = index * channels;
            if let Some(colors) = out.colors.get_mut(start..start + channels) {
                for (color, &value) in colors.iter_mut().zip(premultiplied) {
                    *color = if alpha == 1.0 {
                        value
                    } else if alpha > 0.0 {
                        value / alpha
                    } else {
                        0.0
                    };
                }
            }
            out.alpha.push(alpha);
            if let Some(shape) = shape.as_deref_mut() {
                shape.push(own);
            }
        }
    }
}

/// The bytes of nearest samples' codes, `N` per pixel, through `bytes`.
fn nearest_bytes<const N: usize>(bytes: &ByteLanes, codes: &[u8], out: &mut [[u8; 4]]) {
    let codes = codes.as_chunks::<N>().0;
    if bytes.identity && (N == 1 || bytes.component == [0, 1, 2]) {
        for (pixel, code) in out.iter_mut().zip(codes) {
            *pixel = if N == 1 {
                [code[0], code[0], code[0], 255]
            } else {
                [code[0], code[1 % N], code[2 % N], 255]
            };
        }
        return;
    }
    let [a, b, c] = bytes.component.map(|component| component.min(N - 1));
    let [ta, tb, tc] = &bytes.tables;
    for (pixel, code) in out.iter_mut().zip(codes) {
        *pixel = [
            ta[usize::from(code[a])],
            tb[usize::from(code[b])],
            tc[usize::from(code[c])],
            255,
        ];
    }
}

/// The bytes of covered pixels' average codes, `N` per pixel, through each
/// lane's map; `[0, 0, 0, 0]` where a pixel is not covered. Lanes that
/// share one unit map round every average in one pass, with `levels`
/// holding the bytes.
fn average_bytes<const N: usize>(
    (maps, component): (&[Affine], [usize; 3]),
    averages: &[f64],
    (covered, levels): (&[bool], &mut Vec<u8>),
    out: &mut [[u8; 4]],
) {
    let (Some(&a), Some(&b), Some(&c)) = (maps.first(), maps.get(1), maps.get(2)) else {
        return;
    };
    let [ca, cb, cc] = component.map(|component| component.min(N - 1));
    if a.unit.is_some() && a == b && a == c {
        levels.clear();
        levels.resize(averages.len(), 0);
        if a.levels(averages, levels) {
            // A level next to a rounding boundary: round the colour values.
            for (level, &average) in levels.iter_mut().zip(averages) {
                *level = pagura_raster::quantize_unit(a.apply(average));
            }
        }
        for ((pixel, level), &covered) in out.iter_mut().zip(levels.as_chunks::<N>().0).zip(covered)
        {
            *pixel = if covered {
                [level[ca], level[cb], level[cc], 255]
            } else {
                [0; 4]
            };
        }
        return;
    }
    for ((pixel, average), &covered) in out.iter_mut().zip(averages.as_chunks::<N>().0).zip(covered)
    {
        *pixel = if covered {
            [
                a.byte(average[ca]),
                b.byte(average[cb]),
                c.byte(average[cc]),
                255,
            ]
        } else {
            [0; 4]
        };
    }
}

/// Rounds a span's opaque colours, three per pixel, to bytes; a pixel of
/// alpha 0 becomes `[0, 0, 0, 0]`.
fn round_bytes(span: &SpanColors, out: &mut [[u8; 4]]) {
    let quantize = pagura_raster::quantize_unit;
    let colors = span.colors.as_chunks::<3>().0;
    for (i, (pixel, color)) in out.iter_mut().zip(colors).enumerate() {
        *pixel = if span.alpha.get(i).is_some_and(|&alpha| alpha == 0.0) {
            [0; 4]
        } else {
            [
                quantize(color[0]),
                quantize(color[1]),
                quantize(color[2]),
                255,
            ]
        };
    }
}

/// The shape of `len` opaque pixels: 1 each.
fn opaque_shape(len: usize, shape: Option<&mut Vec<f64>>) {
    if let Some(shape) = shape {
        shape.clear();
        shape.resize(len, 1.0);
    }
}

/// Where device pixels sample: nearest samples or separable footprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Nearest,
    Area,
}

/// How footprints weight their samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taps {
    /// The exact area of the pixel's preimage.
    Area,
    /// Bilinear weights of the two samples around the pixel centre.
    Bilinear,
}

/// The per-column part of the placement, for the columns of the region.
#[derive(Debug)]
enum Plan {
    /// Per device column, the sample along the source axis it steps, and
    /// whether they are consecutive.
    Nearest(Vec<u32>, bool),
    /// Per device column, its footprint along that axis.
    Area(Vec<Footprint>),
}

/// The fast sampler of one image: its lanes, placement and scratch.
#[derive(Debug)]
pub(super) struct Codes {
    lanes: Lanes,
    kind: Kind,
    taps: Taps,
    geometry: Geometry,
    components: usize,
    /// The first device column of `plan`.
    x0: u32,
    plan: Plan,
    /// Whether every planned footprint weights its samples whole.
    whole: bool,
    /// The longest run of planned footprints that are whole pairs of
    /// samples, each next to the last, with one normalization.
    pairs: std::ops::Range<usize>,

    /// The blocks of planar samples that hold only background.
    clean: Option<Clean>,
    /// The bytes of opaque three-channel colours, for spans filled as
    /// bytes.
    bytes: Option<Box<ByteLanes>>,
    /// Per device column of the region, its bilinear footprint for bytes.
    bilinear: Vec<BilinearTap>,
    scratch: Mutex<Scratch>,
}

/// The bytes that the colours of nearest samples round to, per lane and
/// code, for opaque lanes of three colour channels: what
/// [`pagura_raster::quantize_unit`] gives the values [`Lanes::write_codes`]
/// writes.
#[derive(Debug)]
struct ByteLanes {
    tables: [[u8; 256]; 3],
    /// Each lane's component.
    component: [usize; 3],
    /// Whether every lane is its component's code itself: the bytes are the
    /// codes.
    identity: bool,
}

impl ByteLanes {
    /// The byte tables of opaque `lanes` of three channels whose image has
    /// `n` components, or `None` for other lanes.
    fn new(lanes: &Lanes, n: usize) -> Option<Box<Self>> {
        if !lanes.opaque || lanes.table.len() != 3 || lanes.channels != 3 {
            return None;
        }
        let component: [usize; 3] = lanes.component.as_slice().try_into().ok()?;
        let direct = lanes.direct.filter(|_| n == 3);
        let mut tables = [[0u8; 256]; 3];
        for (lane, table) in tables.iter_mut().enumerate() {
            for (code, byte) in (0..=u8::MAX).zip(table.iter_mut()) {
                let value = match direct {
                    Some(map) => map.apply(f64::from(code)),
                    None => lanes.table[lane][usize::from(code)],
                };
                *byte = pagura_raster::quantize_unit(value);
            }
        }
        let codes = usize::from(lanes.maximum) + 1;
        let identity = tables.iter().all(|table| {
            (0..=u8::MAX)
                .zip(&table[..codes])
                .all(|(code, &byte)| code == byte)
        });
        Some(Box::new(Self {
            tables,
            component,
            identity,
        }))
    }
}

/// Which eight-by-eight luma blocks of planar samples, with the chroma
/// blocks over them, hold only their planes' backgrounds, and the colour
/// those convert to. Their samples need no conversion and their sums no
/// arithmetic beyond one value per device row.
#[derive(Debug)]
struct Clean {
    /// Samples per block side.
    side: usize,
    /// Bit words per block row.
    words: usize,
    /// One bit per luma block, set where it is background.
    bits: Vec<u64>,
    /// The RGB codes of the backgrounds.
    background: [u8; 3],
}

impl Clean {
    fn new(planar: &Planar) -> Self {
        let side = planar.block().max(1);
        let [_, _, pitch] = planar.geometry(0);
        let [max_h, max_v] = planar.maximum();
        let columns = pitch / side;
        let rows = planar.height().div_ceil(side);
        let words = columns.div_ceil(64);
        let mut bits = vec![0u64; words * rows];
        // The chroma blocks under luma samples `a..=b` of a component with
        // sampling factor `factor` of `maximum`.
        let under = |(a, b): (usize, usize), factor: usize, maximum: usize| {
            (a * factor / maximum / side)..=(b * factor / maximum / side)
        };
        for (row, line) in bits.chunks_exact_mut(words.max(1)).enumerate() {
            let samples = (row * side, row * side + side - 1);
            for column in 0..columns {
                let across = (column * side, column * side + side - 1);
                let chroma = (1..3).all(|c| {
                    let [h, v, _] = planar.geometry(c);
                    under(samples, v, max_v)
                        .all(|y| under(across, h, max_h).all(|x| !planar.dirty(c, y, x)))
                });
                if chroma && !planar.dirty(0, row, column) {
                    line[column / 64] |= 1 << (column % 64);
                }
            }
        }
        let [y, cb, cr] = planar.backgrounds();
        let mut background = [0; 3];
        let _ = pagura_image::deferred::ycbcr(&[y], &[cb], &[cr], 1, &mut background);
        Self {
            side,
            words,
            bits,
            background,
        }
    }

    fn bytes(&self) -> usize {
        self.bits.capacity() * size_of::<u64>()
    }

    /// Splits samples `first..end` of source rows `top..=bottom` into runs
    /// that are all background (`false`) or may not be (`true`). Background
    /// shorter than [`Clean::GAP`] samples joins the runs around it.
    fn runs(
        &self,
        (top, bottom): (usize, usize),
        (first, end): (usize, usize),
        runs: &mut Vec<(usize, usize, bool)>,
    ) {
        runs.clear();
        let mut push = |start: usize, stop: usize, dirty: bool| {
            let dirty = dirty || stop - start < Self::GAP && (start != first || stop != end);
            match runs.last_mut() {
                Some(last) if last.2 == dirty => last.1 = stop,
                _ => runs.push((start, stop, dirty)),
            }
        };
        let side = self.side;
        let (mut column, last) = (first / side, (end - 1) / side);
        while column <= last {
            // The background bits of this word's columns from `column` on,
            // over every block row.
            let (word, shift) = (column / 64, column % 64);
            let mut bits = u64::MAX;
            for row in top / side..=bottom / side {
                bits &= self.bits.get(row * self.words + word).copied().unwrap_or(0);
            }
            let bits = bits >> shift;
            let background = bits & 1 == 1;
            let same = if background {
                bits.trailing_ones()
            } else {
                bits.trailing_zeros()
            };
            let available = 64 - shift;
            let count = usize::try_from(same.min(u32::try_from(available).unwrap_or(64)))
                .unwrap_or(1)
                .max(1)
                .min(last + 1 - column);
            let start = (column * side).max(first);
            column += count;
            let stop = (column * side).min(end);
            push(start, stop, !background);
        }
    }

    /// The shortest background run worth reducing apart from its
    /// neighbours.
    const GAP: usize = 64;
}

/// Storage reused between spans.
#[derive(Debug, Default)]
struct Scratch {
    feet: Vec<Footprint>,
    indices: Vec<u32>,
    lines: Vec<f64>,
    sums: Vec<u32>,
    averages: Vec<f64>,
    /// Per pixel of an averaged span: whether its footprint meets the image.
    covered: Vec<bool>,
    /// Runs of background and other samples.
    runs: Vec<(usize, usize, bool)>,
    codes: Vec<u8>,
    rows: RowCache,
    /// The colours of a span filled as bytes, before they are rounded.
    span: SpanColors,
    /// The bytes of a span's average codes.
    levels: Vec<u8>,
    /// Per source column of a span, the columns before it that are not
    /// background.
    dirty: Vec<u32>,
    /// Per source column of a span, its level plus a half under bilinear
    /// weights, in f32.
    levels32: Vec<[f32; 4]>,
    /// The codes of a bilinear span's first source row.
    top: Vec<u8>,
}

/// Rows of codes converted from planar or packed samples, by row and range.
#[derive(Debug, Default)]
struct RowCache {
    entries: Vec<(usize, (usize, usize), Vec<u8>)>,
    next: usize,
    /// Runs of background and other samples of the row being filled.
    runs: Vec<(usize, usize, bool)>,
}

impl RowCache {
    /// The `len` codes of row `y` over `range`, filled by `fill` unless kept
    /// from an earlier span.
    fn get(
        &mut self,
        (y, range): (usize, (usize, usize)),
        len: usize,
        fill: impl FnOnce(&mut [u8], &mut Vec<(usize, usize, bool)>) -> bool,
    ) -> &[u8] {
        let found = self
            .entries
            .iter()
            .position(|(row, kept, _)| (*row, *kept) == (y, range));
        let index = found.unwrap_or_else(|| {
            let index = if self.entries.len() < CACHED_ROWS {
                self.entries.push((usize::MAX, (0, 0), Vec::new()));
                self.entries.len() - 1
            } else {
                let index = self.next;
                self.next = (self.next + 1) % CACHED_ROWS;
                index
            };
            let entry = &mut self.entries[index];
            // Filling writes every code.
            if entry.2.len() != len {
                entry.2.clear();
                entry.2.resize(len, 0);
            }
            if fill(&mut entry.2, &mut self.runs) {
                (entry.0, entry.1) = (y, range);
            } else {
                // A failed conversion reads as zeros and is not kept.
                entry.2.fill(0);
                (entry.0, entry.1) = (usize::MAX, (0, 0));
            }
            index
        });
        &self.entries[index].2
    }
}

/// The decoded samples as rows of codes.
enum Grid<'a> {
    Bytes { bytes: &'a [u8], width: usize },
    Packed(&'a Packed),
    Planar(&'a Planar, Option<&'a Clean>),
}

impl<'a> Grid<'a> {
    fn new(image: &'a DecodedImage, clean: Option<&'a Clean>) -> Option<Self> {
        match &image.samples {
            Samples::Bytes(bytes) => Some(Self::Bytes {
                bytes,
                width: usize::try_from(image.width).ok()?,
            }),
            Samples::Packed(packed) => Some(Self::Packed(packed)),
            Samples::Planar(planar) => Some(Self::Planar(planar, clean)),
            Samples::Words(_) | Samples::Mixed { .. } => None,
        }
    }

    /// The codes of samples `x0..x1` of row `y`, `n` per sample, each of
    /// a one-component image followed by its complement to `complement`
    /// when that is given.
    fn row<'s>(
        &'s self,
        y: usize,
        ((x0, x1), n): ((usize, usize), usize),
        complement: Option<u8>,
        cache: &'s mut RowCache,
    ) -> &'s [u8] {
        if let Some(maximum) = complement {
            let pairs = |out: &mut [u8], code: &dyn Fn(usize) -> u8| {
                for (x, pair) in (x0..x1).zip(out.as_chunks_mut::<2>().0) {
                    let code = code(x);
                    pair.copy_from_slice(&[code, maximum.saturating_sub(code)]);
                }
                true
            };
            return match self {
                Self::Bytes { bytes, width } => {
                    cache.get((y, (x0, x1)), (x1 - x0) * 2, |out, _| {
                        pairs(out, &|x| bytes.get(y * width + x).copied().unwrap_or(0))
                    })
                }
                Self::Packed(packed) => cache.get((y, (x0, x1)), (x1 - x0) * 2, |out, _| {
                    let row = packed.row(y).unwrap_or(&[]);
                    pairs(out, &|x| bit(row, x))
                }),
                Self::Planar(..) => &[],
            };
        }
        match self {
            Self::Bytes { bytes, width } => {
                let start = (y * width + x0) * n;
                bytes.get(start..start + (x1 - x0) * n).unwrap_or(&[])
            }
            Self::Packed(packed) => cache.get((y, (x0, x1)), x1 - x0, |out, _| {
                packed.row(y).is_some_and(|row| {
                    unpack(row, (x0, x1), out);
                    true
                })
            }),
            Self::Planar(planar, clean) => cache.get((y, (x0, x1)), (x1 - x0) * 3, |out, runs| {
                let Some(clean) = clean else {
                    return planar.rgb_range(y, x0, x1, out);
                };
                // Background blocks need no conversion.
                clean.runs((y, y), (x0, x1), runs);
                runs.iter().all(|&(start, stop, dirty)| {
                    let Some(out) = out.get_mut((start - x0) * 3..(stop - x0) * 3) else {
                        return false;
                    };
                    if dirty {
                        planar.rgb_range(y, start, stop, out)
                    } else {
                        for pixel in out.as_chunks_mut::<3>().0 {
                            *pixel = clean.background;
                        }
                        true
                    }
                })
            }),
        }
    }
}

/// Whether `indices` step by one sample.
fn consecutive(indices: &[u32]) -> bool {
    indices
        .windows(2)
        .all(|pair| pair[0].checked_add(1) == Some(pair[1]))
}

/// A sample coordinate as an index.
fn index(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Samples `x0..x1` of a packed row as one code per byte in `out`.
fn unpack(row: &[u8], (x0, x1): (usize, usize), out: &mut [u8]) {
    let head = x0.next_multiple_of(8).min(x1);
    let whole = (x1 - head) / 8;
    let (start, middle) = out.split_at_mut(head - x0);
    for (x, out) in (x0..).zip(start) {
        *out = bit(row, x);
    }
    let (middle, tail) = middle.split_at_mut(whole * 8);
    let bytes = row.get(head / 8..).unwrap_or(&[]);
    for (out, &byte) in middle.as_chunks_mut::<8>().0.iter_mut().zip(bytes) {
        *out = UNPACKED[usize::from(byte)].to_ne_bytes();
    }
    for (x, out) in (head + whole * 8..).zip(tail) {
        *out = bit(row, x);
    }
}

/// Per byte, its eight bits as eight bytes of 0 or 1, most significant
/// first in memory order.
static UNPACKED: [u64; 256] = unpacked();

#[expect(clippy::cast_possible_truncation, reason = "a bit of a byte is 0 or 1")]
const fn unpacked() -> [u64; 256] {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut bytes = [0u8; 8];
        let mut bit = 0;
        while bit < 8 {
            bytes[bit] = ((byte >> (7 - bit)) & 1) as u8;
            bit += 1;
        }
        table[byte] = u64::from_ne_bytes(bytes);
        byte += 1;
    }
    table
}

/// Sample `x` of a packed row.
fn bit(row: &[u8], x: usize) -> u8 {
    row.get(x / 8).map_or(0, |byte| (byte >> (7 - x % 8)) & 1)
}

/// The weighted sum of a line of reduced values, `N` per sample: the
/// first and last weighted, those between added in order.
#[inline]
fn line_values<const N: usize>(values: &[f64], first: f64, last: f64) -> [f64; N] {
    let mut out = [0.0; N];
    match values.as_chunks::<N>().0 {
        [] => {}
        [a] => {
            for (out, &a) in out.iter_mut().zip(a) {
                *out = first * a;
            }
        }
        [a, b] => {
            for ((out, &a), &b) in out.iter_mut().zip(a).zip(b) {
                *out = first * a + last * b;
            }
        }
        [a, b, c] => {
            for (((out, &a), &b), &c) in out.iter_mut().zip(a).zip(b).zip(c) {
                *out = (first * a + b) + last * c;
            }
        }
        [a, between @ .., z] => {
            for (out, &a) in out.iter_mut().zip(a) {
                *out = first * a;
            }
            for cell in between {
                for (out, &value) in out.iter_mut().zip(cell) {
                    *out += value;
                }
            }
            for (out, &z) in out.iter_mut().zip(z) {
                *out += last * z;
            }
        }
    }
    out
}

/// The footprint of device column `x` along the source rows.
fn column_footprint(geometry: &Geometry, taps: Taps, x: u32) -> Footprint {
    let t = geometry.inverse();
    footprint(taps, x, (t.a, t.e, geometry.size().0))
}

/// The footprint of device row `y` across the source rows.
fn row_footprint(geometry: &Geometry, taps: Taps, y: u32) -> Footprint {
    let t = geometry.inverse();
    footprint(taps, y, (t.d, t.f, geometry.size().1))
}

fn footprint(taps: Taps, pixel: u32, (scale, offset, maximum): (f64, f64, u32)) -> Footprint {
    match taps {
        Taps::Area => Footprint::new(pixel, scale, offset, maximum).trimmed(),
        Taps::Bilinear => Footprint::bilinear(pixel, scale, offset, maximum),
    }
}

/// The samples `first..end` that footprints span.
fn extent(feet: &[Footprint]) -> Option<(usize, usize)> {
    let (first, end) = feet
        .iter()
        .filter(|foot| foot.first < foot.end)
        .fold((u32::MAX, 0), |(first, end), foot| {
            (first.min(foot.first), end.max(foot.end))
        });
    (first < end).then(|| (index(first), index(end)))
}

impl Codes {
    /// The fast sampler of `image` placed by `geometry`, painting `lanes`,
    /// with its plan for device columns `columns`, or `None` when the
    /// placement or the samples do not suit it.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the plan cannot be allocated.
    pub(super) fn new(
        lanes: Lanes,
        image: &DecodedImage,
        geometry: &Geometry,
        columns: (u32, u32),
    ) -> Result<Option<Self>> {
        let t = geometry.inverse();
        if t.b != 0.0 || t.c != 0.0 {
            return Ok(None);
        }
        let (kind, taps) = match geometry.mode() {
            Mode::Nearest => (Kind::Nearest, Taps::Area),
            Mode::Bilinear => (Kind::Area, Taps::Bilinear),
            Mode::Separable { swapped: false } => (Kind::Area, Taps::Area),
            Mode::Separable { swapped: true } | Mode::Area => return Ok(None),
        };
        let components = usize::from(image.components);
        let usable = match &image.samples {
            Samples::Bytes(_) => (1..=4).contains(&components),
            Samples::Packed(_) => components == 1,
            Samples::Planar(_) => components == 3,
            Samples::Words(_) | Samples::Mixed { .. } => false,
        };
        if !usable
            || lanes.component.iter().any(|&c| c >= components)
            || !lanes.opaque && components != 1
        {
            return Ok(None);
        }
        let (x0, x1) = columns;
        let count = index(x1.saturating_sub(x0));
        let plan = match kind {
            Kind::Nearest => {
                let mut indices = Vec::new();
                indices
                    .try_reserve_exact(count)
                    .map_err(|_| Error::limit("image plan allocation failed"))?;
                indices.extend((x0..x1).map(|x| geometry.nearest_cell(x, 0).0));
                let consecutive = consecutive(&indices);
                Plan::Nearest(indices, consecutive)
            }
            Kind::Area => {
                let mut feet = Vec::new();
                feet.try_reserve_exact(count)
                    .map_err(|_| Error::limit("image plan allocation failed"))?;
                feet.extend((x0..x1).map(|x| column_footprint(geometry, taps, x)));
                Plan::Area(feet)
            }
        };
        let clean = match &image.samples {
            Samples::Planar(planar) => Some(Clean::new(planar)),
            _ => None,
        };
        let whole = match &plan {
            Plan::Area(feet) => feet
                .iter()
                .all(|foot| foot.first >= foot.end || foot.whole()),
            Plan::Nearest(..) => false,
        };
        let pairs = match &plan {
            Plan::Area(feet) => pair_run(feet),
            Plan::Nearest(..) => 0..0,
        };
        let bytes = ByteLanes::new(&lanes, components);
        let bilinear = match (&plan, taps, &bytes) {
            (Plan::Area(feet), Taps::Bilinear, Some(_)) => {
                let mut bilinear = Vec::new();
                bilinear
                    .try_reserve_exact(feet.len())
                    .map_err(|_| Error::limit("image plan allocation failed"))?;
                bilinear.extend(feet.iter().map(BilinearTap::new));
                bilinear
            }
            _ => Vec::new(),
        };
        Ok(Some(Self {
            lanes,
            kind,
            taps,
            geometry: geometry.clone(),
            components,
            x0,
            plan,
            whole,
            pairs,
            clean,
            bytes,
            bilinear,
            scratch: Mutex::new(Scratch::default()),
        }))
    }

    /// Bytes the plan and lanes hold.
    pub(super) fn bytes(&self) -> usize {
        let plan = match &self.plan {
            Plan::Nearest(indices, _) => indices.capacity() * size_of::<u32>(),
            Plan::Area(feet) => feet.capacity() * size_of::<Footprint>(),
        };
        plan + self.bilinear.capacity() * size_of::<BilinearTap>()
            + self.lanes.table.len() * size_of::<[f64; 256]>()
            + self.clean.as_ref().map_or(0, Clean::bytes)
    }

    /// Replaces the lanes, keeping the placement.
    pub(super) fn set_lanes(&mut self, lanes: Lanes) {
        self.bytes = ByteLanes::new(&lanes, self.components);
        self.lanes = lanes;
        self.scratch = Mutex::new(Scratch::default());
    }

    /// Whether [`Self::fill_bytes`] fills spans: opaque lanes of three
    /// colour channels.
    pub(super) fn fills_bytes(&self) -> bool {
        self.bytes.is_some()
    }

    /// Fills `out` with device pixels `x .. x + out.len()` of row `y` as
    /// premultiplied RGBA bytes: the colours [`Self::fill`] gives, each
    /// rounded by [`pagura_raster::quantize_unit`], with alpha 255, and
    /// `[0, 0, 0, 0]` where a footprint misses the image (alpha 0). Returns
    /// false, with `out` unspecified, unless [`Self::fills_bytes`].
    pub(super) fn fill_bytes(
        &self,
        image: &DecodedImage,
        (x, y): (u32, u32),
        out: &mut [[u8; 4]],
    ) -> bool {
        let Some(bytes) = self.bytes.as_deref() else {
            return false;
        };
        let len = out.len();
        let Some(grid) = Grid::new(image, self.clean.as_ref()) else {
            out.fill([0, 0, 0, 255]);
            return true;
        };
        let mut guard = match self.scratch.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        let mut local = None;
        let scratch = match guard.as_deref_mut() {
            Some(scratch) => scratch,
            None => local.insert(Scratch::default()),
        };
        let n = self.components;
        if self.kind == Kind::Nearest && matches!(n, 1 | 3) {
            if n == 1 {
                self.nearest::<1>(&grid, (x, y, len), scratch);
                nearest_bytes::<1>(bytes, &scratch.codes, out);
            } else {
                self.nearest::<3>(&grid, (x, y, len), scratch);
                nearest_bytes::<3>(bytes, &scratch.codes, out);
            }
            return true;
        }
        if let Grid::Packed(packed) = &grid
            && self.packed_bytes(&grid, packed, (x, y), scratch, out)
        {
            return true;
        }
        if self.kind == Kind::Area {
            self.area_bytes(&grid, (x, y), (scratch, bytes), out);
            return true;
        }
        let mut span = std::mem::take(&mut scratch.span);
        self.fill_in(&grid, (x, y, len), scratch, &mut span, None);
        round_bytes(&span, out);
        scratch.span = span;
        true
    }

    /// Fills `out` with device pixels `x .. x + len` of row `y`: straight
    /// colours and, unless opaque, alpha and `shape`.
    pub(super) fn fill(
        &self,
        image: &DecodedImage,
        (x, y, len): (u32, u32, usize),
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let Some(grid) = Grid::new(image, self.clean.as_ref()) else {
            out.channels = self.lanes.channels;
            out.alpha.clear();
            out.colors.clear();
            out.colors.resize(len * self.lanes.channels, 0.0);
            return;
        };
        // A concurrent caller works in scratch of its own.
        let mut guard = match self.scratch.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        let mut local = None;
        let scratch = match guard.as_deref_mut() {
            Some(scratch) => scratch,
            None => local.insert(Scratch::default()),
        };
        self.fill_in(&grid, (x, y, len), scratch, out, shape);
    }

    /// The bytes of a one-bit image's span of row `y` from column `x`, by
    /// counts of its bits, unless the span is not one-bit, separable or
    /// planned.
    fn packed_bytes(
        &self,
        grid: &Grid<'_>,
        packed: &Packed,
        (x, y): (u32, u32),
        scratch: &mut Scratch,
        out: &mut [[u8; 4]],
    ) -> bool {
        let Some(range) = self
            .planned(x, out.len())
            .filter(|_| self.components == 1 && self.kind == Kind::Area)
            .filter(|_| self.taps == Taps::Area)
        else {
            return false;
        };
        let (Plan::Area(feet), Some(&[a, b, c])) = (&self.plan, self.lanes.affine.get(..3)) else {
            return false;
        };
        let Some(feet) = feet.get(range.clone()) else {
            return false;
        };
        let across = row_footprint(&self.geometry, self.taps, y);
        let maps = [a, b, c];
        if self.pairs.start <= range.start
            && range.end <= self.pairs.end
            && across.end.wrapping_sub(across.first) == 2
            && across.whole()
        {
            pair_bytes(packed, (&across, feet), maps, out);
            return true;
        }
        let whole = self.whole && across.whole();
        let storage = (&mut scratch.rows, &mut scratch.codes);
        bit_bytes(grid, (&across, feet, whole), maps, storage, out)
    }

    /// The bytes of an averaged span of row `y` from column `x`: bilinear
    /// or separable footprints of lanes that share a unit map in fast
    /// arithmetic, others from their average codes.
    fn area_bytes(
        &self,
        grid: &Grid<'_>,
        (x, y): (u32, u32),
        (scratch, bytes): (&mut Scratch, &ByteLanes),
        out: &mut [[u8; 4]],
    ) {
        let (n, len) = (self.components, out.len());
        let span = (x, y, len);
        let shared = match self.lanes.affine.as_slice() {
            [a, b, c] if a.unit.is_some() && a == b && a == c => Some(*a),
            _ => None,
        };
        if let Some(map) = shared {
            out.fill([0; 4]);
            let maps = (map, bytes.component);
            if self.taps == Taps::Bilinear
                && let Some(range) = self.planned(x, len)
                && let (Some(taps), Plan::Area(feet)) =
                    (self.bilinear.get(range.clone()), &self.plan)
                && let Some(feet) = feet.get(range)
            {
                match n {
                    1 => self.bilinear_bytes::<1>(grid, (y, taps, feet), scratch, (maps, out)),
                    2 => self.bilinear_bytes::<2>(grid, (y, taps, feet), scratch, (maps, out)),
                    3 => self.bilinear_bytes::<3>(grid, (y, taps, feet), scratch, (maps, out)),
                    _ => self.bilinear_bytes::<4>(grid, (y, taps, feet), scratch, (maps, out)),
                }
                return;
            }
            let finish = |reduction: Reduction<'_>, _: &mut [bool], dirty: &mut Vec<u32>| match n {
                1 => column_bytes::<1>(&reduction, maps, dirty, out),
                2 => column_bytes::<2>(&reduction, maps, dirty, out),
                3 => column_bytes::<3>(&reduction, maps, dirty, out),
                _ => column_bytes::<4>(&reduction, maps, dirty, out),
            };
            match n {
                1 => self.separable::<1>(grid, span, scratch, finish),
                2 => self.separable::<2>(grid, span, scratch, finish),
                3 => self.separable::<3>(grid, span, scratch, finish),
                _ => self.separable::<4>(grid, span, scratch, finish),
            }
            return;
        }
        // The average codes, mapped per lane as the colours are.
        scratch.covered.clear();
        scratch.covered.resize(len, false);
        let mut averages = std::mem::take(&mut scratch.averages);
        averages.clear();
        averages.resize(len * n, 0.0);
        let into = (averages.as_mut_slice(), IDENTITY);
        match n {
            1 => self.area::<1, 1>(grid, span, scratch, into),
            2 => self.area::<2, 1>(grid, span, scratch, into),
            3 => self.area::<3, 1>(grid, span, scratch, into),
            _ => self.area::<4, 1>(grid, span, scratch, into),
        }
        let maps = (self.lanes.affine.as_slice(), bytes.component);
        let flags = (scratch.covered.as_slice(), &mut scratch.levels);
        match n {
            1 => average_bytes::<1>(maps, &averages, flags, out),
            2 => average_bytes::<2>(maps, &averages, flags, out),
            3 => average_bytes::<3>(maps, &averages, flags, out),
            _ => average_bytes::<4>(maps, &averages, flags, out),
        }
        scratch.averages = averages;
    }

    /// [`Self::fill`] over `grid` in `scratch`.
    fn fill_in(
        &self,
        grid: &Grid<'_>,
        (x, y, len): (u32, u32, usize),
        scratch: &mut Scratch,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        out.channels = self.lanes.channels;
        out.alpha.clear();
        let n = self.components;
        match self.kind {
            Kind::Nearest => {
                match n {
                    1 => self.nearest::<1>(grid, (x, y, len), scratch),
                    2 => self.nearest::<2>(grid, (x, y, len), scratch),
                    3 => self.nearest::<3>(grid, (x, y, len), scratch),
                    _ => self.nearest::<4>(grid, (x, y, len), scratch),
                }
                self.lanes.write_codes(&scratch.codes, n, out, shape);
            }
            Kind::Area => {
                scratch.covered.clear();
                scratch.covered.resize(len, false);
                // A lane that is not opaque averages each code beside its
                // complement.
                let n = if self.lanes.opaque { n } else { 2 };
                // Lanes that are their components' codes mapped alike, or
                // one component's code copied, take their colours straight
                // from the sums.
                let lanes = self.lanes.table.len();
                let direct = match (self.lanes.direct, self.lanes.replicated) {
                    (Some(map), _) if n == lanes => Some((map, 1)),
                    (_, Some(map)) if n == 1 => Some((map, lanes)),
                    _ => None,
                };
                let (mut target, (map, mut copies)) = match direct {
                    Some(direct) => (std::mem::take(&mut out.colors), direct),
                    None => (std::mem::take(&mut scratch.averages), (IDENTITY, 1)),
                };
                if !matches!(copies, 1 | 3 | 4) {
                    copies = 1;
                }
                target.clear();
                target.resize(len * n * copies, 0.0);
                let into = (target.as_mut_slice(), map);
                let span = (x, y, len);
                match (n, copies) {
                    (1, 3) => self.area::<1, 3>(grid, span, scratch, into),
                    (1, 4) => self.area::<1, 4>(grid, span, scratch, into),
                    (1, _) => self.area::<1, 1>(grid, span, scratch, into),
                    (2, _) => self.area::<2, 1>(grid, span, scratch, into),
                    (3, _) => self.area::<3, 1>(grid, span, scratch, into),
                    _ => self.area::<4, 1>(grid, span, scratch, into),
                }
                let mut shape = shape;
                if direct.is_some() {
                    out.colors = target;
                    opaque_shape(len, shape.as_deref_mut());
                } else {
                    self.lanes
                        .write_averages(&target, n, out, shape.as_deref_mut());
                    scratch.averages = target;
                }
                self.lanes.uncover(&scratch.covered, out, shape);
            }
        }
    }

    /// Fills `out` with the bytes of a bilinear span of row `y`, whose
    /// device columns have the footprints `feet` and `taps`, in f32 with
    /// the three lanes side by side. Each source column's level under the
    /// row's two weights, `g·(w₀·T + w₁·B)` for the row's scale `g` (its
    /// normalization times levels per code), is formed once; a pixel between
    /// columns `c` and `c + 1` has the level `h[c] + t·(h[c + 1] − h[c])`
    /// for its `t` (its normalization times its second weight), its f64
    /// average's level rearranged because the normalized weights sum to
    /// one. The level truncates to the byte the f64 colours round to unless
    /// it lies within the f32 arithmetic's error bound of a rounding
    /// boundary; then the f64 colours of [`Self::fill`] are rounded.
    ///
    /// Levels are at most 256.5 and nonnegative, so each of the ten f32
    /// roundings (the two weights, their products and sum with the offset,
    /// `t`, the difference, product and sum, and the two bound offsets)
    /// moves the level by at most 2^-24 · 257; the f64 colours lie within
    /// 1e-12 of the exact average and the tie guard moves the boundary by
    /// 1e-9. The bound taken, 3e-4, covers twice that.
    fn bilinear_bytes<const N: usize>(
        &self,
        grid: &Grid<'_>,
        (y, taps, feet): (u32, &[BilinearTap], &[Footprint]),
        scratch: &mut Scratch,
        ((map, component), out): ((Affine, [usize; 3]), &mut [[u8; 4]]),
    ) {
        let Scratch {
            rows,
            levels32,
            top,
            ..
        } = scratch;
        let across = row_footprint(&self.geometry, Taps::Bilinear, y);
        let (low, high) = taps.iter().fold((u32::MAX, 0), |(low, high), tap| {
            (low.min(tap.column), high.max(tap.column))
        });
        let width = self.geometry.size().0;
        if low > high || across.first >= across.end {
            return;
        }
        let (first, end) = (index(low), index(high.saturating_add(2).min(width)));
        let codes = grid.row(index(across.first), ((first, end), N), None, rows);
        top.clear();
        top.extend_from_slice(codes);
        let bottom = if across.end - across.first > 1 {
            grid.row(index(across.first) + 1, ((first, end), N), None, rows)
        } else {
            &[]
        };
        let source = BilinearRows {
            top,
            bottom,
            first,
            across,
            lanes: component.map(|component| component.min(N - 1)),
        };
        source.levels::<N>(map, levels32);
        bilinear_pixels::<N>(&source, (levels32, taps, feet), map, out);
    }

    /// The plan's entries for device columns `x .. x + len`, or `None` when
    /// they reach outside it.
    fn planned(&self, x: u32, len: usize) -> Option<std::ops::Range<usize>> {
        let start = index(x.checked_sub(self.x0)?);
        let end = start.checked_add(len)?;
        let available = match &self.plan {
            Plan::Nearest(indices, _) => indices.len(),
            Plan::Area(feet) => feet.len(),
        };
        (end <= available).then_some(start..end)
    }

    /// The codes of the span's nearest samples into `scratch.codes`, `N`
    /// per pixel.
    fn nearest<const N: usize>(
        &self,
        grid: &Grid<'_>,
        (x, y, len): (u32, u32, usize),
        scratch: &mut Scratch,
    ) {
        let Scratch {
            indices,
            codes,
            rows,
            ..
        } = scratch;
        let (indices, consecutive): (&[u32], bool) =
            if let (Plan::Nearest(planned, consecutive), Some(range)) =
                (&self.plan, self.planned(x, len))
            {
                (&planned[range], *consecutive)
            } else {
                indices.clear();
                indices.extend((x..).take(len).map(|x| self.geometry.nearest_cell(x, 0).0));
                (indices.as_slice(), consecutive(indices))
            };
        codes.clear();
        let row = self.geometry.nearest_cell(0, y).1;
        let (first, end) = indices
            .iter()
            .fold((u32::MAX, 0), |(a, b), &i| (a.min(i), b.max(i)));
        if first > end {
            return;
        }
        let (first, end) = (index(first), index(end) + 1);
        let source = grid.row(index(row), ((first, end), N), None, rows);
        codes.resize(indices.len() * N, 0);
        if consecutive {
            // One sample per pixel in order: the row itself.
            if let Some(source) = source.get(..codes.len()) {
                codes.copy_from_slice(source);
            }
            return;
        }
        let samples = source.as_chunks::<N>().0;
        for (out, &column) in codes.as_chunks_mut::<N>().0.iter_mut().zip(indices) {
            if let Some(sample) = samples.get(index(column) - first) {
                *out = *sample;
            }
        }
    }

    /// The average codes of the span's separable footprints, `N` per pixel,
    /// mapped by `map` into `averages`, each value `C` times.
    fn area<const N: usize, const C: usize>(
        &self,
        grid: &Grid<'_>,
        span: (u32, u32, usize),
        scratch: &mut Scratch,
        (averages, map): (&mut [f64], Affine),
    ) {
        self.separable::<N>(grid, span, scratch, |reduction, covered, _| {
            let Reduction {
                feet,
                first,
                reduced,
                scale,
                ..
            } = reduction;
            let map = (scale, map);
            match reduced {
                Reduced::Sums(sums) => columns::<N, C, u32>(
                    (feet, first),
                    sums,
                    (averages, covered),
                    map,
                    |values, _| whole_sum::<N>(values),
                ),
                Reduced::Lines(lines) => columns::<N, C, f64>(
                    (feet, first),
                    lines,
                    (averages, covered),
                    map,
                    |values, foot| line_values::<N>(values, foot.first_weight, foot.last_weight),
                ),
            }
        });
    }

    /// Reduces the span's source rows for every source column its separable
    /// footprints reach, and hands the reduction to `finish` with the
    /// span's coverage flags and quad storage; an empty span leaves both
    /// alone.
    fn separable<const N: usize>(
        &self,
        grid: &Grid<'_>,
        (x, y, len): (u32, u32, usize),
        scratch: &mut Scratch,
        finish: impl FnOnce(Reduction<'_>, &mut [bool], &mut Vec<u32>),
    ) {
        let Scratch {
            feet,
            lines,
            sums,
            covered,
            runs,
            rows,
            dirty,
            ..
        } = scratch;
        let (feet, whole): (&[Footprint], bool) =
            if let (Plan::Area(planned), Some(range)) = (&self.plan, self.planned(x, len)) {
                (&planned[range], self.whole)
            } else {
                feet.clear();
                feet.extend(
                    (x..)
                        .take(len)
                        .map(|x| column_footprint(&self.geometry, self.taps, x)),
                );
                let whole = feet
                    .iter()
                    .all(|foot| foot.first >= foot.end || foot.whole());
                (feet, whole)
            };
        let across = row_footprint(&self.geometry, self.taps, y);
        let Some((first, end)) = extent(feet) else {
            return;
        };
        if across.first >= across.end {
            return;
        }
        let complement = self.lanes.complement();
        let length = (end - first) * N;
        let rows_spanned = (index(across.first), index(across.end) - 1);
        runs.clear();
        match (grid, self.clean.as_ref()) {
            (Grid::Planar(..), Some(clean)) => clean.runs(rows_spanned, (first, end), runs),
            _ => runs.push((first, end, true)),
        }
        let scale = across.normalization;
        if whole && across.whole() {
            // Whole samples sum as integers throughout.
            sum_rows::<N>(
                (grid, complement, self.clean.as_ref()),
                &across,
                (first, end),
                (sums, rows),
                runs,
            );
            let reduction = Reduction {
                feet,
                first,
                reduced: Reduced::Sums(sums),
                scale,
                runs,
                background: self.clean.as_ref().map(|clean| clean.background),
            };
            finish(reduction, covered, dirty);
            return;
        }
        if lines.len() != length {
            lines.clear();
            lines.resize(length, 0.0);
        }
        if let Some(clean) = self.clean.as_ref() {
            let constant: [f64; N] = std::array::from_fn(|k| {
                let code = clean.background.get(k).copied().unwrap_or(0);
                reduce_constant(&across, code)
            });
            for &(start, stop, _) in runs.iter().filter(|run| !run.2) {
                if let Some(lines) = lines.get_mut((start - first) * N..(stop - first) * N) {
                    for line in lines.as_chunks_mut::<N>().0 {
                        *line = constant;
                    }
                }
            }
        }
        if runs.iter().any(|run| run.2) {
            reduce_rows(
                (grid, complement),
                &across,
                ((first, end), N),
                (lines, sums, rows),
                runs,
            );
        }
        let reduction = Reduction {
            feet,
            first,
            reduced: Reduced::Lines(lines),
            scale,
            runs,
            background: self.clean.as_ref().map(|clean| clean.background),
        };
        finish(reduction, covered, dirty);
    }
}

/// A span's source rows reduced for every source column its footprints
/// reach, `N` values per column from `first`, and the row footprint's
/// normalization.
struct Reduction<'a> {
    feet: &'a [Footprint],
    first: usize,
    reduced: Reduced<'a>,
    scale: f64,
    /// Runs of background (`false`) and other source columns.
    runs: &'a [(usize, usize, bool)],
    /// The background's codes, where the runs mark background.
    background: Option<[u8; 3]>,
}

/// How the rows reduced: integer sums where every weight is whole, else
/// weighted lines.
#[derive(Clone, Copy)]
enum Reduced<'a> {
    Sums(&'a [u32]),
    Lines(&'a [f64]),
}

/// The sum of a footprint's whole cells of integer sums, `N` per cell.
#[inline]
fn whole_sum<const N: usize>(values: &[u32]) -> [f64; N] {
    let mut sum = [0u32; N];
    for cell in values.as_chunks::<N>().0 {
        for (sum, &value) in sum.iter_mut().zip(cell) {
            *sum = sum.wrapping_add(value);
        }
    }
    sum.map(f64::from)
}

/// Writes each pixel's bytes from its footprint over `reduction`: the f64
/// average [`columns`] gives, rounded as [`Affine::byte`] rounds it. A pixel
/// whose footprint lies in background columns takes the background's bytes;
/// one whose footprint is empty is `[0, 0, 0, 0]`.
fn column_bytes<const N: usize>(
    reduction: &Reduction<'_>,
    (map, component): (Affine, [usize; 3]),
    dirty: &mut Vec<u32>,
    out: &mut [[u8; 4]],
) {
    let [a, b, c] = component.map(|component| component.min(N - 1));
    let columns = match reduction.reduced {
        Reduced::Sums(sums) => sums.len() / N,
        Reduced::Lines(lines) => lines.len() / N,
    };
    let background = background_bytes(reduction, columns, dirty, (map, [a, b, c]));
    let first = reduction.first;
    for (pixel, foot) in out.iter_mut().zip(reduction.feet) {
        if foot.first >= foot.end {
            *pixel = [0; 4];
            continue;
        }
        let (start, stop) = (index(foot.first) - first, index(foot.end) - first);
        if let Some(background) = background
            && dirty.get(start) == dirty.get(stop)
        {
            *pixel = background;
            continue;
        }
        let sum = match reduction.reduced {
            Reduced::Sums(sums) => sums.get(start * N..stop * N).map(whole_sum::<N>),
            Reduced::Lines(lines) => lines
                .get(start * N..stop * N)
                .map(|values| line_values::<N>(values, foot.first_weight, foot.last_weight)),
        };
        let Some(sum) = sum else {
            *pixel = [0; 4];
            continue;
        };
        let factor = foot.normalization * reduction.scale;
        *pixel = [
            map.byte(sum[a] * factor),
            map.byte(sum[b] * factor),
            map.byte(sum[c] * factor),
            255,
        ];
    }
}

/// The two source rows of a bilinear span from column `first`, `N` codes
/// per column, under the row footprint `across`, for lanes of `lanes`.
struct BilinearRows<'a> {
    top: &'a [u8],
    /// Empty when the footprint has one row.
    bottom: &'a [u8],
    first: usize,
    across: Footprint,
    lanes: [usize; 3],
}

impl BilinearRows<'_> {
    /// Each column's level under the row's weights, plus a half, in f32,
    /// and the last column's again.
    fn levels<const N: usize>(&self, map: Affine, levels: &mut Vec<[f32; 4]>) {
        let scale = self.across.normalization * map.level;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "weights narrowed to f32 within the error bound"
        )]
        let (upper, lower) = (
            (self.across.first_weight * scale) as f32,
            (self.across.last_weight * scale) as f32,
        );
        let lanes = self.lanes;
        levels.clear();
        let top = self.top.as_chunks::<N>().0;
        if self.bottom.is_empty() {
            levels.extend(top.iter().map(|t| {
                let at = |lane: usize| f32::from(t[lanes[lane]]) * upper + 0.5;
                [at(0), at(1), at(2), 0.5]
            }));
        } else {
            levels.extend(
                top.iter()
                    .zip(self.bottom.as_chunks::<N>().0)
                    .map(|(t, b)| {
                        let at = |lane: usize| {
                            f32::from(t[lanes[lane]]) * upper
                                + f32::from(b[lanes[lane]]) * lower
                                + 0.5
                        };
                        [at(0), at(1), at(2), 0.5]
                    }),
            );
        }
        if let Some(&last) = levels.last() {
            levels.push(last);
        }
    }

    /// The bytes of the f64 colours [`Codes::fill`] gives the pixel of
    /// footprint `foot`: each column's line as `reduce_rows` forms it, the
    /// line's weighted sum as `columns` forms it, rounded by `map`.
    fn exact<const N: usize>(&self, foot: &Footprint, map: Affine) -> [u8; 4] {
        let across = &self.across;
        let column = |c: u32, lane: usize| -> f64 {
            let at = (index(c) - self.first) * N + self.lanes[lane];
            let line = across.first_weight * f64::from(self.top.get(at).copied().unwrap_or(0));
            if self.bottom.is_empty() {
                line
            } else {
                line + across.last_weight * f64::from(self.bottom.get(at).copied().unwrap_or(0))
            }
        };
        let factor = foot.normalization * across.normalization;
        let byte = |lane: usize| {
            let mut sum = foot.first_weight * column(foot.first, lane);
            if foot.end - foot.first > 1 {
                sum += foot.last_weight * column(foot.first + 1, lane);
            }
            pagura_raster::quantize_unit(map.apply(sum * factor))
        };
        [byte(0), byte(1), byte(2), 255]
    }
}

/// The pixels of [`Codes::bilinear_bytes`] from the columns' `levels`.
fn bilinear_pixels<const N: usize>(
    source: &BilinearRows<'_>,
    (levels, taps, feet): (&[[f32; 4]], &[BilinearTap], &[Footprint]),
    map: Affine,
    out: &mut [[u8; 4]],
) {
    const BOUND: f32 = 3e-4;
    let base = u32::try_from(source.first).unwrap_or(u32::MAX);
    // Magnified pixels share their columns: the level and its difference
    // stay while the column does.
    let mut current = u32::MAX;
    let (mut here, mut difference) = ([0.0f32; 4], [0.0f32; 4]);
    for ((pixel, tap), foot) in out.iter_mut().zip(taps).zip(feet) {
        if tap.column != current {
            let column = index(tap.column.wrapping_sub(base));
            let Some(&[level, next]) = levels.get(column..column.wrapping_add(2)) else {
                *pixel = [0; 4];
                continue;
            };
            current = tap.column;
            here = level;
            difference = [
                next[0] - level[0],
                next[1] - level[1],
                next[2] - level[2],
                next[3] - level[3],
            ];
        }
        let level = [
            here[0] + tap.t * difference[0],
            here[1] + tap.t * difference[1],
            here[2] + tap.t * difference[2],
            here[3] + tap.t * difference[3],
        ];
        let low = level.map(|level| floor(level - BOUND));
        let high = level.map(|level| floor(level + BOUND));
        *pixel = if low == high {
            (u32::from(byte(low[0]))
                | u32::from(byte(low[1])) << 8
                | u32::from(byte(low[2])) << 16
                | 0xff00_0000)
                .to_le_bytes()
        } else {
            source.exact::<N>(foot, map)
        };
    }
}

/// One device column's bilinear footprint for [`Codes::bilinear_bytes`]:
/// its first source column and its normalization times its second weight.
#[derive(Debug, Clone, Copy)]
struct BilinearTap {
    column: u32,
    t: f32,
}

impl BilinearTap {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a weight narrowed to f32 within the error bound"
    )]
    fn new(foot: &Footprint) -> Self {
        let t = if foot.end.saturating_sub(foot.first) > 1 {
            (foot.normalization * foot.last_weight) as f32
        } else {
            0.0
        };
        Self {
            column: foot.first,
            t,
        }
    }
}

/// The floor of a nonnegative level below 2^32; zero below zero.
#[inline]
fn floor(level: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "levels within 0..257 truncate to their floors; below zero to zero"
    )]
    let floor = level as u32;
    floor
}

/// A level's floor as a byte.
#[inline]
fn byte(floor: u32) -> u8 {
    floor.min(255).to_le_bytes()[0]
}

/// Bytes of a one-bit image whose device pixels each cover a whole pair of
/// samples in a whole pair of rows, `across` and `feet`, with one
/// normalization: a pixel's average is its count of one bits, 0 to 4,
/// times that normalization, so the bytes of the five counts, computed as
/// [`Codes::fill`] computes the colours and rounded, serve every pixel. Two
/// rows' bits count pairwise eight at a time.
fn pair_bytes(
    packed: &Packed,
    (across, feet): (&Footprint, &[Footprint]),
    maps: [Affine; 3],
    out: &mut [[u8; 4]],
) {
    let Some(start) = feet.first() else {
        return;
    };
    let factor = start.normalization * across.normalization;
    let table: [[u8; 4]; 5] = std::array::from_fn(|count| {
        let average = f64::from(u32::try_from(count).unwrap_or(0)) * factor;
        let byte = |map: Affine| pagura_raster::quantize_unit(map.apply(average));
        [byte(maps[0]), byte(maps[1]), byte(maps[2]), 255]
    });
    let rows = (
        packed.row(index(across.first)).unwrap_or(&[]),
        packed.row(index(across.first) + 1).unwrap_or(&[]),
    );
    // Seven bytes of both rows at a time: 28 pixels.
    let mut at = index(start.first);
    for pixels in out.chunks_mut(28) {
        let (byte, shift) = (at / 8, at % 8);
        let words = (
            word_at(rows.0, byte) << shift,
            word_at(rows.1, byte) << shift,
        );
        for (group, pixels) in pixels.chunks_mut(4).enumerate() {
            let at = 56 - 8 * group;
            let pair = |word: u64| PAIRS[usize::from((word >> at).to_le_bytes()[0])];
            let counts = pair(words.0) + pair(words.1);
            for (pixel, shift) in pixels.iter_mut().zip([12, 8, 4, 0]) {
                *pixel = table[usize::from((counts >> shift) & 15).min(4)];
            }
        }
        at += 56;
    }
}

/// The eight bytes of a packed row from byte `start` on, most significant
/// first; bytes past the row are zero.
#[inline]
fn word_at(row: &[u8], start: usize) -> u64 {
    if let Some(bytes) = row
        .get(start..start.wrapping_add(8))
        .and_then(|bytes| bytes.first_chunk::<8>())
    {
        return u64::from_be_bytes(*bytes);
    }
    let mut word = [0u8; 8];
    if let Some(bytes) = row.get(start..) {
        for (word, &byte) in word.iter_mut().zip(bytes) {
            *word = byte;
        }
    }
    u64::from_be_bytes(word)
}

/// The longest run of `feet` that are whole pairs of samples, each next to
/// the last, with one normalization.
fn pair_run(feet: &[Footprint]) -> std::ops::Range<usize> {
    let pair = |foot: &Footprint| foot.end.wrapping_sub(foot.first) == 2 && foot.whole();
    let (mut best, mut start) = (0..0, 0);
    for (i, foot) in feet.iter().enumerate() {
        let continues = i > start
            && pair(foot)
            && feet.get(i - 1).is_some_and(|previous| {
                foot.first == previous.first.wrapping_add(2)
                    && foot.normalization == previous.normalization
            });
        if !continues {
            start = if pair(foot) { i } else { i + 1 };
        }
        if pair(foot) && i + 1 - start > best.len() {
            best = start..i + 1;
        }
    }
    best
}

/// Per byte, the counts of one bits in each of its four pairs of bits,
/// most significant pair in the highest nibble.
static PAIRS: [u16; 256] = pair_counts();

#[expect(
    clippy::cast_possible_truncation,
    reason = "a pair holds at most two one bits"
)]
const fn pair_counts() -> [u16; 256] {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut counts = 0u16;
        let mut pair = 0;
        while pair < 4 {
            let bits = (byte >> (6 - 2 * pair)) & 3;
            let count = (bits & 1) + (bits >> 1);
            counts |= (count as u16) << (12 - 4 * pair);
            pair += 1;
        }
        table[byte] = counts;
        byte += 1;
    }
    table
}

/// Bytes of a one-bit image under separable footprints `across` and
/// `feet`, through `rows`: each source column's bits in the footprint's
/// rows make a class, the first row's bit plus twice the last row's plus
/// four times the count of those between; each class's line is formed as
/// `reduce_rows` forms it (or summed whole, as `sum_rows` sums it, when
/// `whole`), once per row; each pixel then adds its columns' lines as
/// `columns` does. The colours are those of [`Codes::fill`], operation for
/// operation.
fn bit_bytes(
    grid: &Grid<'_>,
    (across, feet, whole): (&Footprint, &[Footprint], bool),
    maps: [Affine; 3],
    (rows, classes): (&mut RowCache, &mut Vec<u8>),
    out: &mut [[u8; 4]],
) -> bool {
    let Some((first, end)) = extent(feet) else {
        out.fill([0; 4]);
        return true;
    };
    let (top, bottom) = (index(across.first), index(across.end));
    let count = bottom.saturating_sub(top);
    if count == 0 || count > 65 {
        return false;
    }
    classes.clear();
    classes.extend_from_slice(grid.row(top, ((first, end), 1), None, rows));
    if count > 1 {
        let last = grid.row(bottom - 1, ((first, end), 1), None, rows);
        for (class, &bit) in classes.iter_mut().zip(last) {
            *class = class.wrapping_add(bit.wrapping_mul(2));
        }
        for y in top + 1..bottom - 1 {
            let row = grid.row(y, ((first, end), 1), None, rows);
            for (class, &bit) in classes.iter_mut().zip(row) {
                *class = class.wrapping_add(bit.wrapping_mul(4));
            }
        }
    }
    if classes.len() != end - first {
        return false;
    }
    // Each class's line; whole footprints weight their cells by one, so
    // the weighted sum of their integer lines is the integer sum.
    let mut lines = [0.0f64; 256];
    // Classes reach four times the rows between, plus three.
    let used = 4 * count.saturating_sub(2) + 4;
    for (class, line) in (0..=u8::MAX).zip(lines.iter_mut()).take(used) {
        let (first_bit, last_bit, between) = (
            f64::from(class & 1),
            f64::from((class >> 1) & 1),
            f64::from(class >> 2),
        );
        *line = if whole {
            first_bit + last_bit + between
        } else if count == 1 {
            across.first_weight * first_bit
        } else if count == 2 {
            across.first_weight * first_bit + across.last_weight * last_bit
        } else {
            (across.first_weight * first_bit + between) + across.last_weight * last_bit
        };
    }
    let shared = maps[0] == maps[1] && maps[0] == maps[2];
    // Padding classes past the last column, whose lines a zero weight
    // drops.
    let widest = feet
        .iter()
        .map(|foot| index(foot.end.saturating_sub(foot.first)))
        .max()
        .unwrap_or(0);
    classes.resize(classes.len() + widest.max(8), 0);
    let span = BitSpan {
        classes,
        first,
        lines: &lines,
        scale: across.normalization,
        maps: (maps, shared),
    };
    match widest {
        0..=4 => span.fill::<4>(feet, out),
        5..=6 => span.fill::<6>(feet, out),
        7..=8 => span.fill::<8>(feet, out),
        _ => span.fill_any(feet, out),
    }
    true
}

/// A device row of [`bit_bytes`]: each source column's class from `first`,
/// padded past the end, the classes' lines, the row's normalization and
/// the lanes' maps, and whether they share one.
struct BitSpan<'a> {
    classes: &'a [u8],
    first: usize,
    lines: &'a [f64; 256],
    scale: f64,
    maps: ([Affine; 3], bool),
}

impl BitSpan<'_> {
    /// The pixels of `feet`, at most `W` samples wide, each summing exactly
    /// `W` lines: the cells past its last weigh zero, which adds nothing,
    /// so the sum is `columns`' sum without a branch on the width.
    fn fill<const W: usize>(&self, feet: &[Footprint], out: &mut [[u8; 4]]) {
        for (pixel, foot) in out.iter_mut().zip(feet) {
            let cells = index(foot.end.saturating_sub(foot.first));
            let start = index(foot.first).wrapping_sub(self.first);
            let Some(window) = self
                .classes
                .get(start..start.wrapping_add(W))
                .and_then(|window| window.first_chunk::<W>())
                .filter(|_| cells > 0)
            else {
                *pixel = [0; 4];
                continue;
            };
            let mut sum = foot.first_weight * self.lines[usize::from(window[0])];
            for (j, &class) in window.iter().enumerate().skip(1) {
                let line = self.lines[usize::from(class)];
                let weight = match (j + 1).cmp(&cells) {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => foot.last_weight,
                    std::cmp::Ordering::Greater => 0.0,
                };
                sum += weight * line;
            }
            *pixel = self.bytes(sum * (foot.normalization * self.scale));
        }
    }

    /// The pixels of `feet` of any width.
    fn fill_any(&self, feet: &[Footprint], out: &mut [[u8; 4]]) {
        for (pixel, foot) in out.iter_mut().zip(feet) {
            let start = index(foot.first).wrapping_sub(self.first);
            let cells = index(foot.end.saturating_sub(foot.first));
            let window = self
                .classes
                .get(start..start.wrapping_add(cells))
                .unwrap_or(&[]);
            let Some((&a, rest)) = window.split_first() else {
                *pixel = [0; 4];
                continue;
            };
            let mut sum = foot.first_weight * self.lines[usize::from(a)];
            if let Some((&z, between)) = rest.split_last() {
                for &class in between {
                    sum += self.lines[usize::from(class)];
                }
                sum += foot.last_weight * self.lines[usize::from(z)];
            }
            *pixel = self.bytes(sum * (foot.normalization * self.scale));
        }
    }

    #[inline]
    fn bytes(&self, average: f64) -> [u8; 4] {
        let ([a, b, c], shared) = self.maps;
        if shared {
            let byte = a.byte(average);
            [byte, byte, byte, 255]
        } else {
            [a.byte(average), b.byte(average), c.byte(average), 255]
        }
    }
}

/// The bytes of a pixel whose footprint lies in background columns, unless
/// the background's level is within 1e-6 of a rounding boundary or there is
/// none; and in `dirty`, the count of columns before each that are not
/// background.
fn background_bytes(
    reduction: &Reduction<'_>,
    columns: usize,
    dirty: &mut Vec<u32>,
    (map, lanes): (Affine, [usize; 3]),
) -> Option<[u8; 4]> {
    let background = reduction.background?;
    dirty.clear();
    dirty.resize(columns + 1, 0);
    let mut count = 0u32;
    for &(start, stop, is_dirty) in reduction.runs {
        let range = (start.max(reduction.first) - reduction.first + 1)
            ..(stop.max(reduction.first) - reduction.first + 1).min(columns + 1);
        let Some(counts) = dirty.get_mut(range) else {
            continue;
        };
        if is_dirty {
            for value in counts.iter_mut() {
                count = count.wrapping_add(1);
                *value = count;
            }
        } else {
            counts.fill(count);
        }
    }
    // A footprint of background cells averages to the background code
    // within a few roundings of its weights.
    let byte = |lane: usize| {
        let code = f64::from(background.get(lane).copied().unwrap_or(0));
        let level = code * map.level + 0.5 + TIE_GUARD;
        let fraction = level - level.floor();
        (fraction > 1e-6 && fraction < 1.0 - 1e-6)
            .then(|| pagura_raster::quantize_unit(map.apply(code)))
    };
    Some([byte(lanes[0])?, byte(lanes[1])?, byte(lanes[2])?, 255])
}

/// Reduces the source rows of `rows` for the source columns of the `true`
/// runs of `first..end` into `lines`, `n` values per column: the first and
/// last rows weighted, those between added as integers in `sums`.
fn reduce_rows(
    (grid, complement): (&Grid<'_>, Option<u8>),
    rows: &Footprint,
    ((first, end), n): ((usize, usize), usize),
    (lines, sums, cache): (&mut [f64], &mut Vec<u32>, &mut RowCache),
    runs: &[(usize, usize, bool)],
) {
    let (top, bottom) = (index(rows.first), index(rows.end) - 1);
    let mut between = false;
    for y in top..=bottom {
        let codes = grid.row(y, ((first, end), n), complement, cache);
        if codes.len() != lines.len() {
            // Conversion failed; the samples read as zeros.
            lines.fill(0.0);
            return;
        }
        let first_between = y != top && y != bottom && !between;
        if first_between {
            // The first row between assigns the sums, the others add.
            if sums.len() != lines.len() {
                sums.clear();
                sums.resize(lines.len(), 0);
            }
            between = true;
        }
        for &(start, stop, _) in runs.iter().filter(|run| run.2) {
            let range = (start - first) * n..(stop - first) * n;
            let (Some(lines), Some(codes)) =
                (lines.get_mut(range.clone()), codes.get(range.clone()))
            else {
                continue;
            };
            if y == top {
                for (line, &code) in lines.iter_mut().zip(codes) {
                    *line = rows.first_weight * f64::from(code);
                }
            } else if y == bottom {
                // Without rows between, the sum of none adds exactly nothing.
                if let Some(sums) = sums.get(range).filter(|_| between) {
                    for ((line, &sum), &code) in lines.iter_mut().zip(sums).zip(codes) {
                        *line = (*line + f64::from(sum)) + rows.last_weight * f64::from(code);
                    }
                } else {
                    for (line, &code) in lines.iter_mut().zip(codes) {
                        *line += rows.last_weight * f64::from(code);
                    }
                }
            } else if let Some(sums) = sums.get_mut(range) {
                if first_between {
                    for (sum, &code) in sums.iter_mut().zip(codes) {
                        *sum = u32::from(code);
                    }
                } else {
                    for (sum, &code) in sums.iter_mut().zip(codes) {
                        *sum = sum.wrapping_add(u32::from(code));
                    }
                }
            }
        }
    }
}

/// Writes each covered pixel's `N` averages, its footprint's sums of
/// `values` (`N` per source column from `first`) as `sum` gives them, scaled
/// by the footprint's and the row's normalization and mapped by
/// `map`, `C` times each, and marks it covered.
fn columns<const N: usize, const C: usize, T>(
    (feet, first): (&[Footprint], usize),
    values: &[T],
    (averages, covered): (&mut [f64], &mut [bool]),
    (scale, map): (f64, Affine),
    sum: impl Fn(&[T], &Footprint) -> [f64; N],
) {
    for ((foot, average), covered) in feet
        .iter()
        .zip(averages.as_chunks_mut::<C>().0.as_chunks_mut::<N>().0)
        .zip(covered.iter_mut())
    {
        if foot.first >= foot.end {
            continue;
        }
        *covered = true;
        let start = (index(foot.first) - first) * N;
        let stop = (index(foot.end) - first) * N;
        let Some(values) = values.get(start..stop) else {
            continue;
        };
        let factor = foot.normalization * scale;
        for (lanes, sum) in average.iter_mut().zip(sum(values, foot)) {
            *lanes = [map.apply(sum * factor); C];
        }
    }
}

/// The integer sums of the source rows of `rows` for every source column
/// `first..end` into `sums`, `n` per column: background runs from their
/// codes, the others from the rows.
fn sum_rows<const N: usize>(
    (grid, complement, clean): (&Grid<'_>, Option<u8>, Option<&Clean>),
    rows: &Footprint,
    (first, end): (usize, usize),
    (sums, cache): (&mut Vec<u32>, &mut RowCache),
    runs: &[(usize, usize, bool)],
) {
    let n = N;
    let (top, bottom) = (index(rows.first), index(rows.end) - 1);
    let length = (end - first) * n;
    if sums.len() != length {
        sums.clear();
        sums.resize(length, 0);
    }
    let count = u32::try_from(bottom + 1 - top).unwrap_or(u32::MAX);
    if let Some(clean) = clean {
        let constant: [u32; N] = std::array::from_fn(|k| {
            u32::from(clean.background.get(k).copied().unwrap_or(0)).wrapping_mul(count)
        });
        for &(start, stop, _) in runs.iter().filter(|run| !run.2) {
            if let Some(sums) = sums.get_mut((start - first) * n..(stop - first) * n) {
                for sum in sums.as_chunks_mut::<N>().0 {
                    *sum = constant;
                }
            }
        }
    }
    for y in top..=bottom {
        let codes = grid.row(y, ((first, end), n), complement, cache);
        if codes.len() != length {
            sums.fill(0);
            return;
        }
        for &(start, stop, _) in runs.iter().filter(|run| run.2) {
            let range = (start - first) * n..(stop - first) * n;
            let (Some(sums), Some(codes)) = (sums.get_mut(range.clone()), codes.get(range)) else {
                continue;
            };
            if y == top {
                for (sum, &code) in sums.iter_mut().zip(codes) {
                    *sum = u32::from(code);
                }
            } else {
                for (sum, &code) in sums.iter_mut().zip(codes) {
                    *sum = sum.wrapping_add(u32::from(code));
                }
            }
        }
    }
}

/// What [`reduce_rows`] gives a column whose rows all hold `code`.
fn reduce_constant(rows: &Footprint, code: u8) -> f64 {
    let code32 = u32::from(code);
    let count = rows.end.saturating_sub(rows.first);
    let mut line = rows.first_weight * f64::from(code);
    if count > 1 {
        let between = code32.wrapping_mul(count - 2);
        if count > 2 {
            line = (line + f64::from(between)) + rows.last_weight * f64::from(code);
        } else {
            line += rows.last_weight * f64::from(code);
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::Affine;

    /// Bytes of mapped codes equal the rounding of the colour values, at
    /// exact half levels, next to them and at random averages, for unit
    /// maps of every precision and other affine maps.
    #[test]
    fn bytes_of_averages_round_their_colour_values() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            f64::from(u32::try_from(state >> 40).unwrap()) / f64::from(1u32 << 24)
        };
        for maximum in [1.0, 3.0, 15.0, 255.0] {
            let maps = [
                Affine::new(0.0, 1.0 / maximum, maximum),
                Affine::new(0.25, 0.5 / maximum, maximum),
                Affine::new(1.0, -1.0 / maximum, maximum),
            ];
            for map in maps {
                let mut codes = Vec::new();
                for level in 0..=510u32 {
                    // Half levels of 255 as codes, and their neighbours.
                    let code = f64::from(level) / 2.0 * maximum / 255.0;
                    codes.extend([code, code.next_up(), code.next_down()]);
                    codes.extend([code + 1e-12, code - 1e-12, code + 1e-9, code - 1e-9]);
                }
                codes.extend((0..20_000).map(|_| random() * maximum));
                for code in codes.into_iter().filter(|c| (0.0..=maximum).contains(c)) {
                    assert_eq!(
                        map.byte(code),
                        pagura_raster::quantize_unit(map.apply(code)),
                        "{map:?} {code}"
                    );
                }
            }
        }
    }
}
