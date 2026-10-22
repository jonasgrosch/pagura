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

/// How each texel lane follows the codes.
#[derive(Debug, Clone)]
pub(super) struct Lanes {
    /// Per lane: the component whose code it follows.
    component: Vec<usize>,
    /// Per lane: the texel value of every code.
    table: Vec<[f64; 256]>,
    /// Per lane: the value at code zero and the change per code.
    affine: Vec<(f64, f64)>,
    /// Per lane: the values at code zero and at the largest code.
    ends: Vec<(f64, f64)>,
    /// The largest code.
    maximum: u8,
    /// The value at code zero and the change per code shared by every lane
    /// of opaque lanes that follow their own component.
    direct: Option<(f64, f64)>,
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
            affine.push((zero, slope));
            ends.push((zero, top));
        }
        let own = component.iter().enumerate().all(|(lane, &c)| lane == c);
        let shared = affine.windows(2).all(|pair| pair[0] == pair[1]);
        let direct = (opaque && own && shared)
            .then(|| affine.first().copied())
            .flatten();
        Some(Self {
            component: component.to_vec(),
            table,
            affine,
            ends,
            maximum,
            direct,
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
        if let Some((zero, slope)) = self.direct.filter(|_| n == self.table.len()) {
            out.colors.clear();
            out.colors
                .extend(averages.iter().map(|&average| zero + average * slope));
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
            for ((value, &component), &(zero, slope)) in
                values.iter_mut().zip(&self.component).zip(&self.affine)
            {
                *value = zero + codes.get(component).copied().unwrap_or(0.0) * slope;
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
        let zero: [f64; L] = std::array::from_fn(|l| self.affine[l].0);
        let slope: [f64; L] = std::array::from_fn(|l| self.affine[l].1);
        let averages = averages.as_chunks::<N>().0;
        colors.clear();
        colors.resize(averages.len() * L, 0.0);
        for (values, average) in colors.as_chunks_mut::<L>().0.iter_mut().zip(averages) {
            for (((value, &component), &zero), &slope) in
                values.iter_mut().zip(&component).zip(&zero).zip(&slope)
            {
                *value = zero + average[component] * slope;
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
        if let Some((zero, slope)) = self.direct.filter(|_| n == self.table.len()) {
            // Every code becomes its colour value alike.
            out.colors.clear();
            out.colors
                .extend(codes.iter().map(|&code| zero + f64::from(code) * slope));
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
    /// The blocks of planar samples that hold only background.
    clean: Option<Clean>,
    scratch: Mutex<Scratch>,
}

/// Which eight-by-eight luma blocks of planar samples, with the chroma
/// blocks over them, hold only their planes' backgrounds, and the colour
/// those convert to. Their samples need no conversion and their sums no
/// arithmetic beyond one value per device row.
#[derive(Debug)]
struct Clean {
    /// Bit words per block row.
    words: usize,
    /// One bit per luma block, set where it is background.
    bits: Vec<u64>,
    /// The RGB codes of the backgrounds.
    background: [u8; 3],
}

impl Clean {
    fn new(planar: &Planar) -> Self {
        let [_, _, pitch] = planar.geometry(0);
        let [max_h, max_v] = planar.maximum();
        let columns = pitch / 8;
        let rows = planar.height().div_ceil(8);
        let words = columns.div_ceil(64);
        let mut bits = vec![0u64; words * rows];
        for (row, line) in bits.chunks_exact_mut(words.max(1)).enumerate() {
            for column in 0..columns {
                let chroma = (1..3).all(|c| {
                    let [h, v, _] = planar.geometry(c);
                    let (y0, y1) = (row * 8 * v / max_v / 8, (row * 8 + 7) * v / max_v / 8);
                    let (x0, x1) = (column * 8 * h / max_h / 8, (column * 8 + 7) * h / max_h / 8);
                    (y0..=y1).all(|y| (x0..=x1).all(|x| !planar.dirty(c, y, x)))
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
        let (mut column, last) = (first / 8, (end - 1) / 8);
        while column <= last {
            // The background bits of this word's columns from `column` on,
            // over every block row.
            let (word, shift) = (column / 64, column % 64);
            let mut bits = u64::MAX;
            for row in top / 8..=bottom / 8 {
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
            let start = (column * 8).max(first);
            column += count;
            let stop = (column * 8).min(end);
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
        *out = [7, 6, 5, 4, 3, 2, 1, 0].map(|shift| (byte >> shift) & 1);
    }
    for (x, out) in (head + whole * 8..).zip(tail) {
        *out = bit(row, x);
    }
}

/// Sample `x` of a packed row.
fn bit(row: &[u8], x: usize) -> u8 {
    row.get(x / 8).map_or(0, |byte| (byte >> (7 - x % 8)) & 1)
}

/// The weighted sum of a line of reduced values, `N` per sample.
#[inline]
fn line_values<const N: usize>(values: &[f64], first: f64, last: f64) -> [f64; N] {
    let mut out = [0.0; N];
    let cells = values.len() / N;
    if cells == 0 {
        return out;
    }
    for (out, &value) in out.iter_mut().zip(values) {
        *out = first * value;
    }
    if cells > 1 {
        for cell in values[N..(cells - 1) * N].as_chunks::<N>().0 {
            for (out, &value) in out.iter_mut().zip(cell) {
                *out += value;
            }
        }
        for (out, &value) in out.iter_mut().zip(&values[(cells - 1) * N..cells * N]) {
            *out += last * value;
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
        Ok(Some(Self {
            lanes,
            kind,
            taps,
            geometry: geometry.clone(),
            components,
            x0,
            plan,
            clean,
            scratch: Mutex::new(Scratch::default()),
        }))
    }

    /// Bytes the plan and lanes hold.
    pub(super) fn bytes(&self) -> usize {
        let plan = match &self.plan {
            Plan::Nearest(indices, _) => indices.capacity() * size_of::<u32>(),
            Plan::Area(feet) => feet.capacity() * size_of::<Footprint>(),
        };
        plan + self.lanes.table.len() * size_of::<[f64; 256]>()
            + self.clean.as_ref().map_or(0, Clean::bytes)
    }

    /// Replaces the lanes, keeping the placement.
    pub(super) fn set_lanes(&mut self, lanes: Lanes) {
        self.lanes = lanes;
        self.scratch = Mutex::new(Scratch::default());
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
        out.channels = self.lanes.channels;
        out.alpha.clear();
        let Some(grid) = Grid::new(image, self.clean.as_ref()) else {
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
        let n = self.components;
        match self.kind {
            Kind::Nearest => {
                match n {
                    1 => self.nearest::<1>(&grid, (x, y, len), scratch),
                    2 => self.nearest::<2>(&grid, (x, y, len), scratch),
                    3 => self.nearest::<3>(&grid, (x, y, len), scratch),
                    _ => self.nearest::<4>(&grid, (x, y, len), scratch),
                }
                self.lanes.write_codes(&scratch.codes, n, out, shape);
            }
            Kind::Area => {
                scratch.averages.clear();
                scratch.covered.clear();
                scratch.covered.resize(len, false);
                // A lane that is not opaque averages each code beside its
                // complement.
                let n = if self.lanes.opaque { n } else { 2 };
                scratch.averages.resize(len * n, 0.0);
                match n {
                    1 => self.area::<1>(&grid, (x, y, len), scratch),
                    2 => self.area::<2>(&grid, (x, y, len), scratch),
                    3 => self.area::<3>(&grid, (x, y, len), scratch),
                    _ => self.area::<4>(&grid, (x, y, len), scratch),
                }
                let mut shape = shape;
                self.lanes
                    .write_averages(&scratch.averages, n, out, shape.as_deref_mut());
                self.lanes.uncover(&scratch.covered, out, shape);
            }
        }
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

    /// The average codes of the span's separable footprints into
    /// `scratch.averages`, `N` per pixel.
    fn area<const N: usize>(
        &self,
        grid: &Grid<'_>,
        (x, y, len): (u32, u32, usize),
        scratch: &mut Scratch,
    ) {
        let Scratch {
            feet,
            lines,
            sums,
            averages,
            covered,
            runs,
            rows,
            ..
        } = scratch;
        let feet: &[Footprint] =
            if let (Plan::Area(planned), Some(range)) = (&self.plan, self.planned(x, len)) {
                &planned[range]
            } else {
                feet.clear();
                feet.extend(
                    (x..)
                        .take(len)
                        .map(|x| column_footprint(&self.geometry, self.taps, x)),
                );
                feet
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
        if lines.len() != length {
            lines.clear();
            lines.resize(length, 0.0);
        }
        let rows_spanned = (index(across.first), index(across.end) - 1);
        runs.clear();
        match (grid, self.clean.as_ref()) {
            (Grid::Planar(..), Some(clean)) => clean.runs(rows_spanned, (first, end), runs),
            _ => runs.push((first, end, true)),
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
        let scale = across.normalization;
        for ((foot, average), covered) in feet
            .iter()
            .zip(averages.as_chunks_mut::<N>().0)
            .zip(covered.iter_mut())
        {
            if foot.first >= foot.end {
                continue;
            }
            *covered = true;
            let start = (index(foot.first) - first) * N;
            let stop = (index(foot.end) - first) * N;
            let Some(values) = lines.get(start..stop) else {
                continue;
            };
            let sum = line_values::<N>(values, foot.first_weight, foot.last_weight);
            for (average, sum) in average.iter_mut().zip(sum) {
                *average = sum * foot.normalization * scale;
            }
        }
    }
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
