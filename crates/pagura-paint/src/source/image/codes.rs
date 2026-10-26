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
use crate::source::{Pixels, SpanColors};
use pagura_core::{Meter, Result};
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
    /// Averages take either about equally often, so both are computed and
    /// one selected, rather than branching.
    #[inline]
    fn apply(self, code: f64) -> f64 {
        let value = code * self.slope;
        match self.unit {
            Some(maximum) => {
                let quotient = code / maximum;
                std::hint::select_unpredictable(value * maximum != code, quotient, value)
            }
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
    /// their own component, and that value of every code.
    direct: Option<Affine>,
    codes: Option<Box<[f64; 256]>>,
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
    pub(super) fn single(
        table: &[f64],
        lanes: usize,
        codes: usize,
        opaque: bool,
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        if !(2..=256).contains(&codes) || lanes < 2 || table.len() != codes * lanes {
            return Ok(None);
        }
        let kept = if opaque { lanes - 2 } else { lanes };
        let components = [0; MAX_LANES];
        let Some(component) = components.get(..kept) else {
            return Ok(None);
        };
        Self::new(
            (kept, codes),
            component,
            (lanes - 2, opaque),
            |lane, code| table[code * lanes + lane],
            bytes,
        )
    }

    /// The lanes of an opaque three-component image of eight-bit codes whose
    /// channel `c` is `tables[c]` of component `c`.
    pub(super) fn rgb(tables: &[[f64; 256]; 3], bytes: &mut Meter) -> Result<Option<Self>> {
        Self::new(
            (3, 256),
            &[0, 1, 2],
            (3, true),
            |lane, code| tables[lane][code],
            bytes,
        )
    }

    fn new(
        (kept, count): (usize, usize),
        component: &[usize],
        (channels, opaque): (usize, bool),
        value: impl Fn(usize, usize) -> f64,
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        let Some(maximum) = count.checked_sub(1).and_then(|n| u8::try_from(n).ok()) else {
            return Ok(None);
        };
        if kept > MAX_LANES {
            return Ok(None);
        }
        let mut maps = [Affine::new(0.0, 0.0, 1.0); MAX_LANES];
        let mut ends = [(0.0, 0.0); MAX_LANES];
        for lane in 0..kept {
            let (zero, top) = (value(lane, 0), value(lane, count - 1));
            let maximum = f64::from(maximum);
            let slope = (top - zero) / maximum;
            let straight = (0..count).zip(0u32..).all(|(index, code)| {
                let value = value(lane, index);
                let expected = zero + f64::from(code) * slope;
                (value - expected).abs() <= AFFINE_TOLERANCE * value.abs().max(1.0)
            });
            if !straight || !slope.is_finite() {
                return Ok(None);
            }
            maps[lane] = Affine::new(zero, slope, maximum);
            ends[lane] = (zero, top);
        }
        let affine = super::copied(&maps[..kept], bytes)?;
        let ends = super::copied(&ends[..kept], bytes)?;
        let component = super::copied(component, bytes)?;
        let mut table = super::reserved(kept, bytes)?;
        for lane in 0..kept {
            let mut entries = [0.0; 256];
            for (code, entry) in entries[..count].iter_mut().enumerate() {
                *entry = value(lane, code);
            }
            table.push(entries);
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
        let codes = if let Some(map) = direct {
            bytes.charge_items::<[f64; 256]>(1)?;
            let mut codes = Box::new([0.0; 256]);
            for (code, value) in (0..=u8::MAX).zip(codes.iter_mut()) {
                *value = map.apply(f64::from(code));
            }
            Some(codes)
        } else {
            None
        };
        Ok(Some(Self {
            component,
            table,
            affine,
            ends,
            maximum,
            direct,
            codes,
            replicated,
            channels,
            opaque,
        }))
    }

    pub(super) fn bytes(&self) -> usize {
        self.component.capacity() * size_of::<usize>()
            + self.table.capacity() * size_of::<[f64; 256]>()
            + self.affine.capacity() * size_of::<Affine>()
            + self.ends.capacity() * size_of::<(f64, f64)>()
            + self.codes.as_ref().map_or(0, |_| size_of::<[f64; 256]>())
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
        if let Some(values) = self.codes.as_deref().filter(|_| n == self.table.len()) {
            // Every code becomes its colour value alike.
            out.colors.clear();
            out.colors
                .extend(codes.iter().map(|&code| values[usize::from(code)]));
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

/// The sampler of one image by its codes: its lanes and the separable
/// placement.
#[derive(Debug)]
pub(super) struct Codes {
    lanes: Lanes,
    components: usize,
    separable: Separable,
    /// Per colour lane, the N16 word of each code's colour, where three
    /// opaque lanes take nearest samples.
    words: Option<Box<[[u16; 256]; 3]>>,
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
        bytes: &mut Meter,
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
            let released = lanes.bytes();
            drop(lanes);
            bytes.refund(released);
            return Ok(None);
        }
        let Some(separable) = Separable::new(geometry, columns, bytes)? else {
            let released = lanes.bytes();
            drop(lanes);
            bytes.refund(released);
            return Ok(None);
        };
        let words = word_tables(&lanes, components, &separable, bytes)?;
        Ok(Some(Self {
            lanes,
            components,
            separable,
            words,
        }))
    }

    /// The caller-owned storage needed by the code sampler.
    pub(super) fn layout(&self) -> super::separable::Layout {
        let values = if self.lanes.complement().is_some() {
            2
        } else {
            self.components
        };
        self.separable
            .layout(values, super::separable::Values::Codes)
    }

    /// The lone texel's colour, without constructing execution storage.
    pub(super) fn constant(&self, image: &DecodedImage, out: &mut SpanColors) {
        let mut codes = [0u8; 4];
        for (i, code) in codes.iter_mut().enumerate().take(self.components) {
            *code = u8::try_from(image.samples.get(i).unwrap_or(0)).unwrap_or(0);
        }
        out.channels = self.lanes.channels;
        self.lanes
            .write_codes(&codes[..self.components], self.components, out, None);
    }

    /// Writes the N16 words of the opaque colours of device pixels
    /// `x .. x + len` of row `y` into `out`, as a store at full opacity
    /// writes them; false, with nothing written, unless the image's three
    /// lanes are opaque and its pixels take nearest samples, or their
    /// colours are mapped averages (see [`Self::mapped`]) whose footprints
    /// all meet the image.
    pub(super) fn words(
        &self,
        image: &DecodedImage,
        pixels: Pixels<'_>,
        scratch: &mut super::separable::Scratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        let Some(grid) = Grid::new(image) else {
            return false;
        };
        let rows = CodeRows {
            grid,
            components: self.components,
            complement: None,
        };
        if !self.separable.nearest() {
            return match self.mapped(rows.width()) {
                Some(map) if self.lanes.table.len() == 3 => {
                    let map = |average| map.apply(average);
                    self.separable.words(&rows, pixels, scratch, map, out)
                }
                _ => false,
            };
        }
        let Some(words) = self.words.as_deref() else {
            return false;
        };
        let n = rows.width().max(1);
        let &[a, b, c] = self.lanes.component.as_slice() else {
            return false;
        };
        let code = |sample: &[u8], k: usize| usize::from(sample.get(k).copied().unwrap_or(0));
        self.separable.gather(&rows, pixels, scratch, |codes| {
            for (pixel, sample) in out.iter_mut().zip(codes.chunks_exact(n)) {
                *pixel = [
                    words[0][code(sample, a)],
                    words[1][code(sample, b)],
                    words[2][code(sample, c)],
                    u16::MAX,
                ];
            }
        });
        true
    }

    /// Bytes the plan and lanes hold.
    pub(super) fn bytes(&self) -> usize {
        self.separable.bytes()
            + self.lanes.bytes()
            + self
                .words
                .as_ref()
                .map_or(0, |_| size_of::<[[u16; 256]; 3]>())
    }

    /// Replaces the lanes, keeping the placement.
    pub(super) fn set_lanes(&mut self, lanes: Lanes, bytes: &mut Meter) -> Result<()> {
        let old = std::mem::replace(&mut self.lanes, lanes);
        let released = old.bytes();
        drop(old);
        bytes.refund(released);
        if let Some(old) = self.words.take() {
            drop(old);
            bytes.refund(size_of::<[[u16; 256]; 3]>());
        }
        self.words = word_tables(&self.lanes, self.components, &self.separable, bytes)?;
        Ok(())
    }

    /// The placement for a stencil whose colour no longer follows codes.
    pub(super) fn into_separable(self, bytes: &mut Meter) -> Separable {
        let released = self.lanes.bytes()
            + self
                .words
                .as_ref()
                .map_or(0, |_| size_of::<[[u16; 256]; 3]>());
        let Self {
            lanes,
            words,
            separable,
            ..
        } = self;
        drop((lanes, words));
        bytes.refund(released);
        separable
    }

    /// Fills `out` with device pixels `x .. x + len` of row `y`: straight
    /// colours and, unless opaque, alpha and `shape`.
    pub(super) fn fill(
        &self,
        image: &DecodedImage,
        pixels: Pixels<'_>,
        scratch: &mut super::separable::Scratch,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let len = pixels.len;
        out.channels = self.lanes.channels;
        out.alpha.clear();
        let Some(grid) = Grid::new(image) else {
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
            self.separable.gather(&rows, pixels, scratch, |codes| {
                self.lanes.write_codes(codes, n, out, shape);
            });
            return;
        }
        let mut shape = shape;
        let lanes = self.lanes.table.len();
        if let Some(map) = self.mapped(n) {
            let mut colors = std::mem::take(&mut out.colors);
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
                .average(&rows, pixels, scratch, into, (map, finish));
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
            .average(&rows, pixels, scratch, None, (|average| average, finish));
    }

    /// The map of averages of `n` codes per pixel to colours, for lanes that
    /// are their components' codes mapped alike, or one component's code
    /// mapped and copied: they take their colours as the averages are
    /// written.
    fn mapped(&self, n: usize) -> Option<Affine> {
        match (self.lanes.direct, self.lanes.replicated) {
            (Some(map), _) if n == self.lanes.table.len() => Some(map),
            (_, Some(map)) if n == 1 => Some(map),
            _ => None,
        }
    }
}

/// Per colour lane, the N16 word of each code's colour as
/// [`Lanes::write_codes`] gives it, for three opaque lanes sampled nearest.
fn word_tables(
    lanes: &Lanes,
    components: usize,
    separable: &Separable,
    bytes: &mut Meter,
) -> Result<Option<Box<[[u16; 256]; 3]>>> {
    if !separable.nearest() || !lanes.opaque || lanes.table.len() != 3 {
        return Ok(None);
    }
    bytes.charge_items::<[[u16; 256]; 3]>(1)?;
    let direct = lanes.codes.as_deref().filter(|_| components == 3);
    let mut words = Box::new([[0u16; 256]; 3]);
    for (words, table) in words.iter_mut().zip(&lanes.table) {
        for (code, word) in words.iter_mut().enumerate() {
            let value = direct.map_or(table[code], |direct| direct[code]);
            *word = crate::kernel::opaque_word(value);
        }
    }
    Ok(Some(words))
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
            (Grid::Planar(planar), None) => {
                runs.clear();
                planar.background_runs(rows, columns, |start, stop, background| {
                    runs.push((start, stop, !background));
                });
                Some(planar.background_rgb())
            }
            _ => None,
        }
    }
}

/// The decoded samples as rows of codes.
enum Grid<'a> {
    Bytes { bytes: &'a [u8], width: usize },
    Packed(&'a Packed),
    Planar(&'a Planar),
}

impl<'a> Grid<'a> {
    fn new(image: &'a DecodedImage) -> Option<Self> {
        match &image.samples {
            Samples::Bytes(bytes) => Some(Self::Bytes {
                bytes,
                width: usize::try_from(image.width).ok()?,
            }),
            Samples::Packed(packed) => Some(Self::Packed(packed)),
            Samples::Planar(planar) => Some(Self::Planar(planar)),
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
            Self::Planar(planar) => {
                cache.get((y, (x0, x1)), 3, |out, _| planar.rgb_range(y, x0, x1, out))
            }
        }
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
