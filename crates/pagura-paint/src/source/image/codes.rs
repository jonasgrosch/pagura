//! Area averages, bilinear interpolation and nearest samples of codes
//! (8.9.5.3, ADR 0004), for upright placements of images whose texel lanes
//! each follow one component's code affinely: device colour spaces
//! converted as the identity, through any decode array that stays within
//! the colour range, and every lane of a one-bit image. Quarter turns are
//! sampled upright from the image with rows and columns exchanged.
//!
//! The exact area average of such texels is the same affine function of the
//! exact area average of the codes, which [`Separable`] forms with the
//! codes of a footprint's interior added as integers. The result differs
//! from adding weighted texels one by one in f64 only by the rounding of
//! the few f64 operations at the edges and of the final scaling, about
//! 1e-15 of the value, far below the output quantizer's tie guard (1e-9 of
//! a level). Nearest samples look up the texel of their code, exactly as
//! the general sampler does.

use super::geometry::{Geometry, MAX_LANES};
use super::separable::{RowCache, Rows, Runs, Separable};
use crate::SpanColors;
use pagura_core::Result;
use pagura_image::deferred::{Packed, Planar};
use pagura_image::{DecodedImage, Samples};

/// The largest deviation from an affine function, relative to the value,
/// that a texel table may show and still count as affine: a few roundings
/// of the conversion's f64 arithmetic.
const AFFINE_TOLERANCE: f64 = 1e-14;

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
    fn uncover(
        &self,
        covered: &[bool],
        (colors, alpha): (&mut [f64], &mut Vec<f64>),
        shape: Option<&mut Vec<f64>>,
    ) {
        if covered.iter().all(|&covered| covered) {
            return;
        }
        let channels = self.channels;
        if self.opaque {
            alpha.clear();
            alpha.extend(covered.iter().map(|&covered| f64::from(u8::from(covered))));
        }
        for (index, _) in covered.iter().enumerate().filter(|(_, covered)| !**covered) {
            let start = index * channels;
            if let Some(colors) = colors.get_mut(start..start + channels) {
                colors.fill(0.0);
            }
            if let Some(alpha) = alpha.get_mut(index) {
                *alpha = 0.0;
            }
        }
        if let Some(shape) = shape {
            if self.opaque {
                shape.clone_from(alpha);
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

/// The shape of `len` opaque pixels: 1 each.
fn opaque_shape(len: usize, shape: Option<&mut Vec<f64>>) {
    if let Some(shape) = shape {
        shape.clear();
        shape.resize(len, 1.0);
    }
}

/// The sampler of one image by its codes: its lanes, the separable
/// placement and, for planar samples, their background blocks.
#[derive(Debug)]
pub(super) struct Codes {
    lanes: Lanes,
    components: usize,
    clean: Option<Clean>,
    separable: Separable,
}

impl Codes {
    /// The sampler of `image` by codes, placed by `geometry`, painting
    /// `lanes`, with its plan for device columns `columns`, or `None` when
    /// the placement or the samples do not suit it.
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
        let Some(separable) = Separable::new(geometry, columns)? else {
            return Ok(None);
        };
        let clean = match &image.samples {
            Samples::Planar(planar) => Some(Clean::new(planar)),
            _ => None,
        };
        Ok(Some(Self {
            lanes,
            components,
            clean,
            separable,
        }))
    }

    /// Bytes the plan and lanes hold.
    pub(super) fn bytes(&self) -> usize {
        self.separable.bytes()
            + self.lanes.table.len() * size_of::<[f64; 256]>()
            + self.clean.as_ref().map_or(0, Clean::bytes)
    }

    /// Replaces the lanes, keeping the placement.
    pub(super) fn set_lanes(&mut self, lanes: Lanes) {
        self.lanes = lanes;
        self.separable.forget();
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
        let rows = CodeRows {
            grid,
            components: self.components,
            complement: self.lanes.complement(),
        };
        let n = rows.width();
        if self.separable.nearest() {
            self.separable.gather(&rows, (x, y, len), |codes| {
                self.lanes.write_codes(codes, n, out, shape);
            });
            return;
        }
        let mut shape = shape;
        // Lanes that are their components' codes mapped alike, or one
        // component's code mapped and copied, take their colours as the
        // averages are written.
        let lanes = self.lanes.table.len();
        let mapped = match (self.lanes.direct, self.lanes.replicated) {
            (Some(map), _) if n == lanes => Some(map),
            (_, Some(map)) if n == 1 => Some(map),
            _ => None,
        };
        if let Some(map) = mapped {
            let mut colors = std::mem::take(&mut out.colors);
            colors.clear();
            colors.resize(len * lanes, 0.0);
            let alpha = &mut out.alpha;
            let finish = |colors: &mut [f64], covered: &[bool]| {
                if lanes == 3 && n == 1 {
                    for [a, b, c] in colors.as_chunks_mut::<3>().0 {
                        (*b, *c) = (*a, *a);
                    }
                } else if lanes > n {
                    for pixel in colors.chunks_exact_mut(lanes) {
                        if let Some((&mut first, rest)) = pixel.split_first_mut() {
                            rest.fill(first);
                        }
                    }
                }
                opaque_shape(len, shape.as_deref_mut());
                self.lanes.uncover(covered, (&mut [], alpha), shape);
            };
            let into = Some((colors.as_mut_slice(), lanes));
            let map = |average| map.apply(average);
            self.separable
                .average(&rows, (x, y, len), into, (map, finish));
            out.colors = colors;
            return;
        }
        let finish = |averages: &mut [f64], covered: &[bool]| {
            self.lanes
                .write_averages(averages, n, out, shape.as_deref_mut());
            self.lanes
                .uncover(covered, (&mut out.colors, &mut out.alpha), shape);
        };
        self.separable
            .average(&rows, (x, y, len), None, (|average| average, finish));
    }
}

/// An image's samples as rows of codes, each of a one-component image
/// followed by its complement to `complement` when that is given.
struct CodeRows<'a> {
    grid: Grid<'a>,
    components: usize,
    complement: Option<u8>,
}

impl Rows for CodeRows<'_> {
    type Value = u8;

    fn width(&self) -> usize {
        if self.complement.is_some() {
            2
        } else {
            self.components
        }
    }

    fn row<'c>(
        &'c self,
        y: usize,
        columns: (usize, usize),
        cache: &'c mut RowCache<u8>,
    ) -> &'c [u8] {
        self.grid
            .row(y, (columns, self.components), self.complement, cache)
    }

    fn background(
        &self,
        rows: (usize, usize),
        columns: (usize, usize),
        runs: &mut Runs,
    ) -> Option<&[u8]> {
        match (&self.grid, self.complement) {
            (Grid::Planar(_, Some(clean)), None) => {
                clean.runs(rows, columns, runs);
                Some(&clean.background)
            }
            _ => None,
        }
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
        // Per chroma component: each luma column's chroma columns, and per
        // luma row the chroma columns any chroma row under it marks.
        let chroma: [(Vec<std::ops::RangeInclusive<usize>>, usize); 2] = [1, 2].map(|c| {
            let [h, _, chroma_pitch] = planar.geometry(c);
            let ranges = (0..columns)
                .map(|column| under((column * side, column * side + side - 1), h, max_h))
                .collect();
            (ranges, chroma_pitch / side)
        });
        let mut marked: Vec<bool> = Vec::new();
        for (row, line) in bits.chunks_exact_mut(words.max(1)).enumerate() {
            let samples = (row * side, row * side + side - 1);
            for column in 0..columns {
                if !planar.dirty(0, row, column) {
                    line[column / 64] |= 1 << (column % 64);
                }
            }
            for (c, (ranges, chroma_columns)) in (1..3).zip(&chroma) {
                let [_, v, _] = planar.geometry(c);
                marked.clear();
                marked.resize(*chroma_columns, false);
                for y in under(samples, v, max_v) {
                    for (x, mark) in marked.iter_mut().enumerate() {
                        *mark |= planar.dirty(c, y, x);
                    }
                }
                if !marked.contains(&true) {
                    continue;
                }
                for (column, range) in ranges.iter().enumerate() {
                    let last = (*range.end()).min(marked.len().saturating_sub(1));
                    if marked
                        .get(*range.start()..=last)
                        .is_some_and(|marks| marks.contains(&true))
                    {
                        line[column / 64] &= !(1 << (column % 64));
                    }
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
        cache: &'s mut RowCache<u8>,
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
                Self::Bytes { bytes, width } => cache.get((y, (x0, x1)), 2, |out, _| {
                    pairs(out, &|x| bytes.get(y * width + x).copied().unwrap_or(0))
                }),
                Self::Packed(packed) => cache.get((y, (x0, x1)), 2, |out, _| {
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
            Self::Packed(packed) => cache.get((y, (x0, x1)), 1, |out, _| {
                packed.row(y).is_some_and(|row| {
                    unpack(row, (x0, x1), out);
                    true
                })
            }),
            Self::Planar(planar, clean) => cache.get((y, (x0, x1)), 3, |out, runs| {
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
                        fill_rgb(out, clean.background);
                        true
                    }
                })
            }),
        }
    }
}

/// Fills `out` with `rgb` repeated, sixteen samples at a copy.
fn fill_rgb(out: &mut [u8], rgb: [u8; 3]) {
    let mut pattern = [0u8; 48];
    for chunk in pattern.as_chunks_mut::<3>().0 {
        *chunk = rgb;
    }
    let (chunks, rest) = out.as_chunks_mut::<48>();
    for chunk in chunks {
        *chunk = pattern;
    }
    for (out, &value) in rest.iter_mut().zip(&pattern) {
        *out = value;
    }
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

#[cfg(test)]
mod tests {
    use super::Clean;
    use pagura_image::deferred::Planar;

    /// Background blocks are those whose luma block and every chroma block
    /// under it hold the background, for every sampling of the chroma.
    #[test]
    fn background_blocks_have_clean_chroma_under_them() {
        let mut state = 0x1234_5678u32;
        let mut random = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 16
        };
        let cases: [([usize; 2], (usize, usize)); 4] = [
            ([2, 2], (37, 29)),
            ([1, 1], (24, 16)),
            ([2, 1], (40, 9)),
            ([4, 1], (70, 15)),
        ];
        for ([h, v], (width, height)) in cases {
            let maximum = [h, v];
            let geometry = [
                [h, v, (width.div_ceil(8 * h) * 8 * h).next_multiple_of(8)],
                [1, 1, width.div_ceil(8 * h) * 8],
                [1, 1, width.div_ceil(8 * h) * 8],
            ];
            let rows = [
                height.div_ceil(8 * v) * 8 * v,
                height.div_ceil(8 * v) * 8,
                height.div_ceil(8 * v) * 8,
            ];
            let backgrounds = [255u8, 128, 128];
            let planes: [Vec<u8>; 3] = std::array::from_fn(|c| {
                let [_, _, pitch] = geometry[c];
                let mut plane = vec![backgrounds[c]; pitch * rows[c]];
                for block in 0..(pitch / 8) * (rows[c] / 8) {
                    if random() % 5 == 0 {
                        let (row, column) = (block / (pitch / 8), block % (pitch / 8));
                        plane[(row * 8 + 3) * pitch + column * 8 + 2] = 7;
                    }
                }
                plane
            });
            let planar = Planar::new((width, height), maximum, geometry, planes).unwrap();
            let clean = Clean::new(&planar);
            let side = 8;
            let under = |(a, b): (usize, usize), factor: usize, maximum: usize| {
                (a * factor / maximum / side)..=(b * factor / maximum / side)
            };
            for row in 0..height.div_ceil(side) {
                for column in 0..geometry[0][2] / side {
                    let samples = (row * side, row * side + side - 1);
                    let across = (column * side, column * side + side - 1);
                    let expected = !planar.dirty(0, row, column)
                        && (1..3).all(|c| {
                            let [h, v, _] = planar.geometry(c);
                            under(samples, v, maximum[1]).all(|y| {
                                under(across, h, maximum[0]).all(|x| !planar.dirty(c, y, x))
                            })
                        });
                    let actual =
                        clean.bits[row * clean.words + column / 64] >> (column % 64) & 1 == 1;
                    assert_eq!(actual, expected, "{maximum:?} block {row} {column}");
                }
            }
        }
    }
}
