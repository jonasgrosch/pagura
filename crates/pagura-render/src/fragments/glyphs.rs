//! Glyph coverage from prepared, vertically clipped edge fragments.
//!
//! Each outline is flattened once per linear map. Its edges are clipped to
//! rows once per vertical phase, and those fragments are reused across
//! horizontal phases, with area division done when a fragment is prepared.
//! Fragments are dealt from the rows in turn, four to a vector of lanes, so
//! that their areas form side by side and their additions never wait on one
//! another. Each cell still receives its additions in clipping order, but
//! for the +0.0 that a one-cell fragment adds two cells on, which can lie in
//! the next row and changes no level.
//! Origins snap to a 1/128-pixel lattice horizontally and to the
//! resolution's lattice vertically; integer translation is applied when the
//! mask is painted. A mask is a pure function of its key, so retention and
//! eviction change time, never pixels.

use super::Resolution;
use super::geometry::{Coverage, Geometry, View};
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
    /// Every retained mask's levels, one after another, within the bound.
    store: Vec<u8>,
    /// The mask built last; its levels are copied to the store when the
    /// cache retains it.
    built: Coverage,
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
    place: Place,
}

/// A retained mask's extent and its levels' range in the store.
#[derive(Debug, Clone, Copy)]
struct Place {
    x: i32,
    y: i32,
    width: usize,
    start: usize,
    end: usize,
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
    pub(crate) fn coverage(
        &mut self,
        outline: &Arc<Path>,
        (transform, resolution): (Transform, Resolution),
        limits: Limits,
        scratch: &mut Accumulator,
    ) -> Result<(View<'_>, (i32, i32))> {
        let (px, x) = phase(transform.e, PHASES)?;
        let (py, y) = phase(transform.f, resolution.vertical_phases())?;
        let key = Key::new(outline, resolution, transform, px, py);
        if let Some(place) = self.masks.get(&key).map(|mask| mask.place) {
            return Ok((self.retained(place), (x, y)));
        }
        let row_key = Key::new(outline, resolution, transform, 0.0, py);
        if let Some(rows) = self.rows.get(&row_key) {
            rows.mask(px, limits, scratch, &mut self.built)?;
        } else {
            let geometry_key = Key::new(outline, resolution, transform, 0.0, 0.0);
            let rows = if let Some(prepared) = self.geometries.get(&geometry_key) {
                Rows::new(prepared, py, limits, &mut scratch.clipping)?
            } else {
                let prepared = Prepared::new(outline, transform, resolution, limits)?;
                let rows = Rows::new(&prepared, py, limits, &mut scratch.clipping)?;
                if self.reserve(outline, prepared.bytes, limits.document_bytes) {
                    self.geometries.insert(geometry_key, prepared);
                }
                rows
            };
            rows.mask(px, limits, scratch, &mut self.built)?;
            if self.reserve(outline, rows.bytes, limits.document_bytes) {
                self.rows.insert(row_key, rows);
            }
        }
        let bytes = entry_bytes(outline, self.built.data.len());
        if self.reserve(outline, bytes, limits.document_bytes)
            && let Some(place) = self.store(limits.document_bytes)
        {
            self.masks.insert(
                key,
                Mask {
                    _outline: Arc::clone(outline),
                    place,
                },
            );
            return Ok((self.retained(place), (x, y)));
        }
        Ok((self.built.view(), (x, y)))
    }

    /// Makes room for the masks of a page's `placements` glyph placements, so
    /// that they are not rehashed as the cache fills: most placements need a
    /// mask of their own. The hint stops at the entries the bound can hold.
    pub(crate) fn make_room(&mut self, placements: usize, document_bytes: usize) {
        let room = self.maximum.min(document_bytes).saturating_sub(self.bytes) / ENTRY_BYTES;
        // A hint only: a failed reservation leaves the map to grow as it fills.
        self.masks.try_reserve(placements.min(room)).ok();
    }

    fn retained(&self, place: Place) -> View<'_> {
        Coverage {
            x: place.x,
            y: place.y,
            width: place.width,
            data: &self.store[place.start..place.end],
        }
    }

    /// Copies the mask built last to the store, whose capacity grows by
    /// doubling within the cache's bound.
    fn store(&mut self, document_bytes: usize) -> Option<Place> {
        let start = self.store.len();
        let end = start.checked_add(self.built.data.len())?;
        if end > self.store.capacity() {
            let bound = self.maximum.min(document_bytes).max(end);
            let target = end.max(self.store.capacity().saturating_mul(2)).min(bound);
            self.store.try_reserve_exact(target - start).ok()?;
        }
        self.store.extend_from_slice(&self.built.data);
        Some(Place {
            x: self.built.x,
            y: self.built.y,
            width: self.built.width,
            start,
            end,
        })
    }

    /// Releases every retained entry; the store keeps its capacity.
    fn clear(&mut self) {
        let mut store = std::mem::take(&mut self.store);
        store.clear();
        let built = std::mem::take(&mut self.built);
        *self = Self {
            store,
            built,
            ..Self::new(self.maximum)
        };
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
        let mut known = self.outlines.contains(&source);
        let mut charge = if known { storage } else { full };
        if self.bytes.saturating_add(charge) > maximum {
            if full > maximum {
                return false;
            }
            self.clear();
            (known, charge) = (false, full);
        }
        if !known {
            self.outlines.insert(source);
        }
        self.bytes += charge;
        true
    }

    /// Bytes charged for retained masks, fragments, geometry and outlines.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

/// Bytes charged for a retained entry besides its levels and outline.
const ENTRY_BYTES: usize = 256;

fn entry_bytes(path: &Path, storage: usize) -> usize {
    storage
        .saturating_add(
            path.segments()
                .len()
                .saturating_mul(size_of::<pagura_raster::Segment>()),
        )
        .saturating_add(ENTRY_BYTES)
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
#[derive(Debug, Clone, Copy)]
struct Fragment {
    /// The index of the row's first cell in the accumulator.
    start: u32,
    left: f32,
    right: f32,
    scale: f32,
    height: f32,
}

impl Fragment {
    /// A fragment that adds +0.0 to the three cells from `start`.
    const EMPTY: Self = Self {
        start: 0,
        left: 0.0,
        right: 0.0,
        scale: 0.0,
        height: 0.0,
    };
}

/// Four row fragments side by side, whose areas form in vector lanes.
#[derive(Debug, Clone, Copy)]
struct Quad {
    start: [u32; 4],
    left: [f32; 4],
    right: [f32; 4],
    scale: [f32; 4],
    height: [f32; 4],
}

impl From<[Fragment; 4]> for Quad {
    fn from(lanes: [Fragment; 4]) -> Self {
        Self {
            start: lanes.map(|f| f.start),
            left: lanes.map(|f| f.left),
            right: lanes.map(|f| f.right),
            scale: lanes.map(|f| f.scale),
            height: lanes.map(|f| f.height),
        }
    }
}

impl Quad {
    fn set(&mut self, lane: usize, f: Fragment) {
        self.start[lane] = f.start;
        self.left[lane] = f.left;
        self.right[lane] = f.right;
        self.scale[lane] = f.scale;
        self.height[lane] = f.height;
    }
}

/// Reusable storage for clipping a glyph's edges to rows.
#[derive(Debug, Default)]
struct Clipping {
    /// Each row's first fragment, and then the total.
    starts: Vec<usize>,
    /// While clipping, each row's next fragment; then the rows that have
    /// fragments left to deal.
    rows: Vec<usize>,
    /// The fragments, row by row.
    fragments: Vec<Fragment>,
}

impl Clipping {
    /// The retained capacities, enlarged where needed, and the padded quads.
    fn row_bytes(&self, height: usize, count: usize) -> usize {
        row_bytes(
            self.starts
                .capacity()
                .max(height + 1)
                .saturating_add(self.rows.capacity().max(height)),
            self.fragments.capacity().max(count),
            count.div_ceil(4),
        )
    }
    /// Reserve the row buffers and quads after their logical payload fits.
    /// Keep scratch capacity whenever the complete payload remains in budget.
    fn reserve_rows(&mut self, height: usize, count: usize, budget: usize) -> Result<Vec<Quad>> {
        if self.row_bytes(height, count) > budget && self.fragments.capacity() > count {
            self.fragments = Vec::new();
        }
        if self.row_bytes(height, count) > budget && self.rows.capacity() > height {
            self.rows = Vec::new();
        }
        if self.row_bytes(height, count) > budget && self.starts.capacity() > height + 1 {
            self.starts = Vec::new();
        }
        let mut held = self.row_bytes(height, count);
        let mut quads = Vec::new();
        try_reserve(&mut self.starts, height + 1, &mut held, budget)?;
        try_reserve(&mut self.rows, height, &mut held, budget)?;
        try_reserve(&mut self.fragments, count, &mut held, budget)?;
        try_reserve(&mut quads, count.div_ceil(4), &mut held, budget)?;
        Ok(quads)
    }
}

fn row_bytes(tables: usize, fragments: usize, quads: usize) -> usize {
    tables
        .saturating_mul(size_of::<usize>())
        .saturating_add(fragments.saturating_mul(size_of::<Fragment>()))
        .saturating_add(quads.saturating_mul(size_of::<Quad>()))
}

/// The row fragments of a prepared glyph at one vertical phase.
#[derive(Debug)]
struct Rows {
    _outline: Arc<Path>,
    /// The fragments dealt from the rows in turn, so that neighbours lie in
    /// different rows and never add to one cell; each row keeps the order
    /// of its fragments, and so each cell the order of its additions but
    /// for zero terms (see [`Accumulator::quad`]). Empty fragments on the
    /// spare cells complete the last quad.
    quads: Vec<Quad>,
    /// The fragments before that padding.
    count: usize,
    /// An upper bound on the cells the fragments visit at any phase.
    work: usize,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    bytes: usize,
}

impl Rows {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "row starts are checked to fit 32 bits"
    )]
    fn new(glyph: &Prepared, py: f64, limits: Limits, clipping: &mut Clipping) -> Result<Self> {
        let stride = glyph.width + 1;
        // Every row start, and the spare cells past the last row, fit 32
        // bits.
        let spare = glyph
            .height
            .checked_mul(stride)
            .filter(|cells| {
                cells
                    .checked_add(SPARE)
                    .is_some_and(|padded| u32::try_from(padded).is_ok())
            })
            .ok_or(Error::limit("glyph extent budget exceeded"))?;
        // Half the allocation budget holds the vector payloads of the two
        // row tables, clipped fragments and padded quads. Count span lengths
        // before visiting their rows or allocating storage for this glyph.
        let budget = limits.decoded_bytes / 2;
        let exceeded = || Error::limit("glyph row fragment budget exceeded");
        let tables = glyph.height.saturating_mul(2).saturating_add(1);
        if row_bytes(tables, 0, 0) > budget {
            *clipping = Clipping::default();
            return Err(exceeded());
        }
        let mut total = 0_usize;
        for &edge in &glyph.edges {
            if let Some((.., crossed)) = span(edge, py, glyph.height) {
                total = total.saturating_add(crossed.len());
                if total > limits.raster_work
                    || row_bytes(tables, total, total.div_ceil(4)) > budget
                {
                    *clipping = Clipping::default();
                    return Err(exceeded());
                }
            }
        }
        let mut quads = clipping.reserve_rows(glyph.height, total, budget)?;
        let Clipping {
            starts,
            rows,
            fragments,
        } = clipping;
        // Each row's fragment count, one past its index, then their sums.
        // The bounded span count and all capacity checks precede this walk.
        starts.resize(glyph.height + 1, 0);
        for &edge in &glyph.edges {
            if let Some((.., crossed)) = span(edge, py, glyph.height) {
                for row in crossed {
                    starts[row + 1] += 1;
                }
            }
        }
        let mut sum = 0;
        for start in starts.iter_mut() {
            sum += *start;
            *start = sum;
        }
        fragments.resize(total, Fragment::EMPTY);
        rows.extend_from_slice(&starts[..glyph.height]);
        let work = clip(glyph, py, |row, fragment| {
            let slot = &mut rows[row];
            fragments[*slot] = fragment;
            *slot += 1;
        })?;
        // Deal the rows' fragments in turns: turn `depth` takes the next
        // fragment of each row that has one.
        rows.clear();
        rows.extend((0..glyph.height).filter(|&row| starts[row] < starts[row + 1]));
        let padding = Fragment {
            start: spare as u32,
            ..Fragment::EMPTY
        };
        quads.resize(total.div_ceil(4), Quad::from([padding; 4]));
        let (mut depth, mut dealt) = (0, 0);
        while !rows.is_empty() {
            rows.retain(|&row| {
                let index = starts[row] + depth;
                quads[dealt / 4].set(dealt % 4, fragments[index]);
                dealt += 1;
                index + 1 < starts[row + 1]
            });
            depth += 1;
        }
        // Each padding fragment visits three cells.
        let work = work.saturating_add(3 * (total.next_multiple_of(4) - total));
        let bytes = entry_bytes(
            &glyph.outline,
            quads.capacity().saturating_mul(size_of::<Quad>()),
        );
        Ok(Self {
            _outline: Arc::clone(&glyph.outline),
            quads,
            count: total,
            work,
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
    /// Builds the mask at horizontal phase `px` into `output`, reusing its
    /// storage.
    fn mask(
        &self,
        px: f64,
        limits: Limits,
        accumulator: &mut Accumulator,
        output: &mut Coverage,
    ) -> Result<()> {
        if self.width == 0 {
            output.width = 0;
            output.data.clear();
            return Ok(());
        }
        accumulator.reset(self.width, self.height, limits)?;
        let px = px as f32;
        // A fragment visits at most ceil(right - left) + 3 cells at any
        // phase, so within the bound no fragment can exceed the work budget.
        if self.work <= accumulator.maximum {
            let width = i32::try_from(self.width).unwrap_or(i32::MAX);
            for quad in &self.quads {
                accumulator.quad(quad, px, width);
            }
        } else {
            let lanes = self.quads.iter().flat_map(|q| (0..4).map(move |i| (q, i)));
            for (q, i) in lanes.take(self.count) {
                accumulator.segment(
                    q.start[i] as usize,
                    q.left[i] + px,
                    q.right[i] + px,
                    q.height[i],
                    q.scale[i],
                )?;
            }
        }
        // Rows padded with transparent cells to whole four-pixel blocks let
        // blits run without single-pixel tails; transparent coverage leaves
        // a pixel unchanged.
        let row = self.width.next_multiple_of(4);
        (output.x, output.y, output.width) = (self.x, self.y, row);
        accumulator.finish_into(row, &mut output.data);
        Ok(())
    }
}

/// Clears a row buffer and reserves its required length. `held` includes
/// every buffer's retained or required capacity, including pending quads.
/// An allocator may grant more than requested; reject that excess before
/// filling any rows and release the buffer that exceeded the allowance.
fn try_reserve<T>(
    values: &mut Vec<T>,
    count: usize,
    held: &mut usize,
    budget: usize,
) -> Result<()> {
    let planned = values.capacity().max(count).saturating_mul(size_of::<T>());
    values.clear();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("glyph row allocation failed"))?;
    let actual = values.capacity().saturating_mul(size_of::<T>());
    let reserved = (*held - planned).saturating_add(actual);
    if reserved > budget {
        *values = Vec::new();
        return Err(Error::limit("glyph row fragment budget exceeded"));
    }
    *held = reserved;
    Ok(())
}

/// An edge's ends at vertical phase `py`, upward, its winding sign and the
/// rows it crosses, unless it is horizontal.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "normalized finite geometry and checked raster extents bound row indices"
)]
fn span(
    [mut ax, ay, mut bx, by]: [f32; 4],
    py: f64,
    height: usize,
) -> Option<([f32; 4], f32, std::ops::Range<usize>)> {
    let (mut ay, mut by) = (ay + py as f32, by + py as f32);
    let sign = if ay < by {
        1.0
    } else {
        std::mem::swap(&mut ax, &mut bx);
        std::mem::swap(&mut ay, &mut by);
        -1.0
    };
    (ay != by).then(|| {
        (
            [ax, ay, bx, by],
            sign,
            ay as usize..ceil_index(by).min(height),
        )
    })
}

/// Clips `glyph`'s edges at vertical phase `py` to rows, passing each
/// fragment and its row to `store` edge by edge, as [`span`] counts them,
/// and returns the cells the fragments visit at most.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "row starts are checked to fit 32 bits"
)]
fn clip(glyph: &Prepared, py: f64, mut store: impl FnMut(usize, Fragment)) -> Result<usize> {
    let mut work = 0_usize;
    let stride = glyph.width + 1;
    for &edge in &glyph.edges {
        let Some(([ax, ay, bx, by], sign, rows)) = span(edge, py, glyph.height) else {
            continue;
        };
        let slope = (bx - ax) / (by - ay);
        // Finite endpoints within the glyph's extent give finite row
        // ends for a finite slope; an infinite one makes the first
        // fragment's end NaN or infinite.
        if !slope.is_finite() && !rows.is_empty() {
            return Err(Error::limit("glyph row arithmetic exceeded"));
        }
        for row in rows {
            let low = ay.max(row as f32);
            let high = by.min(row as f32 + 1.0);
            let x0 = ax + slope * (low - ay);
            let x1 = ax + slope * (high - ay);
            let left = x0.min(x1);
            let right = x0.max(x1);
            let height = (high - low) * sign;
            let scale = if right - left < 1e-6 {
                0.0
            } else {
                height / (right - left)
            };
            work = work.saturating_add(ceil_index(right - left) + 3);
            store(
                row,
                Fragment {
                    start: (row * stride) as u32,
                    left,
                    right,
                    scale,
                    height,
                },
            );
        }
    }
    Ok(work)
}

/// Spare cells past a mask's last row: the cells of padding fragments, and
/// the third cell of a one-cell fragment in the last row's last column.
const SPARE: usize = 3;

/// Signed-area cells in f32, one sentinel per row and [`SPARE`] cells after
/// the last; reusable working storage for one mask at a time.
#[derive(Debug, Default)]
pub(crate) struct Accumulator {
    area: Vec<f32>,
    clipping: Clipping,
    width: usize,
    height: usize,
    work: usize,
    maximum: usize,
}

impl Accumulator {
    /// Zeroes `width` by `height` cells and the work count. The cells and the
    /// finished mask, its rows padded to four cells, are charged together to
    /// the render's allocation budget,
    /// and capacity retained from a larger earlier mask is released when that
    /// budget no longer covers it.
    fn reset(&mut self, width: usize, height: usize, limits: Limits) -> Result<()> {
        let count = width
            .checked_mul(height)
            .filter(|&n| n <= limits.pixels && n <= limits.raster_work)
            .ok_or(Error::limit("glyph coverage extent budget exceeded"))?;
        let cells = count
            .checked_add(height)
            .and_then(|cells| cells.checked_add(SPARE))
            .ok_or(Error::limit("glyph coverage allocation budget exceeded"))?;
        let mask = width
            .checked_next_multiple_of(4)
            .and_then(|row| row.checked_mul(height))
            .ok_or(Error::limit("glyph coverage allocation budget exceeded"))?;
        let available = limits
            .decoded_bytes
            .checked_sub(mask)
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
        start: usize,
        left: f32,
        right: f32,
        height: f32,
        scale: f32,
    ) -> Result<()> {
        let lo = (left as usize).min(self.width);
        let hi = ceil_index(right).min(self.width);
        self.work = self.work.saturating_add(hi - lo + 1);
        if self.work > self.maximum {
            return Err(Error::limit("glyph coverage work budget exceeded"));
        }
        self.add(start, left, right, height, scale);
        Ok(())
    }

    /// Adds the signed areas of `quad`'s fragments at horizontal phase `px`,
    /// in lane order and without charging work; `width` is the mask's width,
    /// or `i32::MAX` when that is larger.
    ///
    /// The common fragments are formed in all four lanes at once and added
    /// without branches:
    /// - one within a cell adds [`Self::add`]'s two terms to that cell and
    ///   the next, and +0.0 to the cell after; adding +0.0 changes at most
    ///   the sign of a zero sum, and so no level;
    /// - one that crosses one cell boundary adds [`Self::add`]'s three
    ///   terms. Over its ranges (`next - left` in (0, 1], `next + 1 - left`
    ///   in [1, 2], `next + 1 - right` in [0, 1], `next - right` below zero)
    ///   each ramp area takes one branch of [`primitive`], the last +0.0, so
    ///   the terms here are its operations one for one.
    ///
    /// Any other fragment takes [`Self::add`] in its turn.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "lanes outside the checked ranges take `add`"
    )]
    #[inline(always)]
    fn quad(&mut self, quad: &Quad, px: f32, width: i32) {
        const LANES: [usize; 4] = [0, 1, 2, 3];
        // All ones where `condition` holds, so that lanes select by bits.
        let all = |condition: bool| 0_u32.wrapping_sub(u32::from(condition));
        let select =
            |mask: u32, a: f32, b: f32| f32::from_bits(a.to_bits() & mask | b.to_bits() & !mask);
        let (height, scale) = (quad.height, quad.scale);
        let left = quad.left.map(|v| v + px);
        let right = quad.right.map(|v| v + px);
        // Truncation is the floor where it is used: the left end is not
        // negative.
        let cell = left.map(|v| v as i32);
        let next = cell.map(|c| c as f32 + 1.0);
        let one =
            LANES.map(|i| all(left[i] >= 0.0) & all(cell[i] < width) & all(right[i] <= next[i]));
        let two = LANES.map(|i| {
            all(left[i] >= 0.0)
                & all(cell[i] < width - 1)
                & all(right[i] > next[i])
                & all(right[i] <= next[i] + 1.0)
                & all(scale[i] != 0.0)
        });
        #[expect(clippy::manual_midpoint, reason = "as in `add`")]
        let value = LANES.map(|i| (next[i] - (left[i] + right[i]) * 0.5) * height[i]);
        let near = LANES.map(|i| next[i] - left[i]);
        let first = LANES.map(|i| near[i] * near[i] * 0.5 * scale[i]);
        let far = LANES.map(|i| (next[i] + 1.0) - left[i]);
        let end = LANES.map(|i| (next[i] + 1.0) - right[i]);
        let second = LANES.map(|i| ((0.5 + (far[i] - 1.0)) - end[i] * end[i] * 0.5) * scale[i]);
        let a = LANES.map(|i| select(one[i], value[i], first[i]));
        let b = LANES.map(|i| select(one[i], height[i] - value[i], second[i] - first[i]));
        let c = LANES.map(|i| select(one[i], 0.0, height[i] - second[i]));
        let at = LANES.map(|i| quad.start[i].wrapping_add(cell[i].cast_unsigned()) as usize);
        let formed = LANES.map(|i| one[i] | two[i]);
        // The common quad, whose four fragments are all formed, is tested
        // once for all its lanes, so that its additions take no branch per
        // lane.
        for i in LANES {
            if formed != [!0; 4] && formed[i] == 0 {
                self.add(
                    quad.start[i] as usize,
                    left[i],
                    right[i],
                    height[i],
                    scale[i],
                );
                continue;
            }
            let cells = &mut self.area[at[i]..at[i] + 3];
            cells[0] += a[i];
            cells[1] += b[i];
            cells[2] += c[i];
        }
    }

    /// Adds one fragment's signed area to the row whose first cell is
    /// `start`, without charging work.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::inline_always,
        reason = "finite fragment endpoints saturate to the checked local raster extent; \
                  two callers per mask"
    )]
    #[inline(always)]
    fn add(&mut self, start: usize, left: f32, right: f32, height: f32, scale: f32) {
        let row = &mut self.area[start..=start + self.width];
        // A fragment within one cell of the row. One that ends where it
        // starts, on a cell boundary, adds its height to that cell and zero
        // to the next, as the general case does.
        // Truncation is the floor here: the left end is not negative.
        let cell = left as usize;
        let next = cell as f32 + 1.0;
        if left >= 0.0 && cell < self.width && right <= next {
            // Halving the f32 sum is `midpoint`'s result here: both ends are
            // bounded, and halving is exact or, below the normal range, the
            // only rounding (tested).
            #[expect(clippy::manual_midpoint, reason = "bounded ends; one rounding")]
            let value = (next - (left + right) * 0.5) * height;
            row[cell] += value;
            row[cell + 1] += height - value;
            return;
        }
        let lo = (left as usize).min(self.width);
        let hi = ceil_index(right).min(self.width);
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
    }

    #[cfg(test)]
    fn finish(&self, row: usize) -> Vec<u8> {
        let mut data = Vec::new();
        self.finish_into(row, &mut data);
        data
    }

    /// The mask's levels, replacing `data`'s contents, in rows of `row`
    /// bytes (at least the width), the cells beyond the width transparent.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "coverage is clamped to [0,255] before positive nearest rounding"
    )]
    fn finish_into(&self, row: usize, data: &mut Vec<u8>) {
        let level = |value: f32| (value.abs().min(1.0) * 255.0 + 0.5) as u8;
        let (width, stride) = (self.width, self.width + 1);
        data.clear();
        data.resize(row * self.height, 0);
        // Each row's prefix sums stay serial, in the same order. Four rows
        // run side by side so that their additions overlap.
        let area = &self.area[..self.height * stride];
        let mut cells = area.chunks_exact(4 * stride);
        let mut outputs = data.chunks_exact_mut(4 * row);
        for (cells, output) in cells.by_ref().zip(outputs.by_ref()) {
            let [d0, d1, d2, d3] = [0, 1, 2, 3].map(|i| &cells[i * stride..i * stride + width]);
            let (o0, output) = output.split_at_mut(row);
            let (o1, output) = output.split_at_mut(row);
            let (o2, o3) = output.split_at_mut(row);
            let [o0, o1, o2, o3] = [o0, o1, o2, o3].map(|output| &mut output[..width]);
            let mut value = [0.0_f32; 4];
            for (column, out) in o0.iter_mut().enumerate() {
                value[0] += d0[column];
                value[1] += d1[column];
                value[2] += d2[column];
                value[3] += d3[column];
                *out = level(value[0]);
                o1[column] = level(value[1]);
                o2[column] = level(value[2]);
                o3[column] = level(value[3]);
            }
        }
        let rows = cells.remainder().chunks_exact(stride);
        for (cells, output) in rows.zip(outputs.into_remainder().chunks_exact_mut(row)) {
            let mut value = 0.0_f32;
            for (out, &delta) in output[..width].iter_mut().zip(cells) {
                value += delta;
                *out = level(value);
            }
        }
    }
}

/// `value.ceil() as usize`, saturating. Baseline x86-64 has no rounding
/// instruction and calls a library function for `ceil`; there the truncated
/// value and one comparison give the same index for every input.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturating conversion"
)]
#[inline]
fn ceil_index(value: f32) -> usize {
    if super::NO_ROUNDING_INSTRUCTION {
        ceil_by_truncation(value)
    } else {
        value.ceil() as usize
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "saturating conversions; below 2^24 the truncation is exact, above it every value is integral"
)]
#[inline]
pub(super) fn ceil_by_truncation(value: f32) -> usize {
    let truncated = value as usize;
    truncated.saturating_add(usize::from((truncated as f32) < value))
}

/// The area of a unit ramp left of `value`: 0 below 0, v²/2 up to 1 and
/// v - 1/2 beyond. Without branches: (v - 1) is exact below 2^24, so the
/// last piece rounds v - 1/2 once, and the other pieces add +0.
fn primitive(value: f32) -> f32 {
    let ramp = value.clamp(0.0, 1.0);
    ramp * ramp * 0.5 + (value - ramp).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::{PathBuilder, Point};

    fn accumulator(width: usize, height: usize) -> Accumulator {
        let mut raster = Accumulator::default();
        raster.reset(width, height, Limits::default()).unwrap();
        raster
    }

    #[test]
    fn branchless_ramp_area_matches_its_pieces() {
        let pieces = |v: f32| {
            if v <= 0.0 {
                0.0
            } else if v < 1.0 {
                v * v * 0.5
            } else {
                v - 0.5
            }
        };
        let near_one = (0..2000).map(|k| f32::from_bits(1.0_f32.to_bits() - 1000 + k));
        let tiny = (0..1000).map(f32::from_bits);
        #[expect(clippy::cast_precision_loss, reason = "small exact integers")]
        let grid = (-3000..70_000).map(|i| i as f32 * 0.001);
        for value in near_one.chain(tiny).chain(grid) {
            assert_eq!(
                primitive(value).to_bits(),
                pieces(value).to_bits(),
                "{value}"
            );
        }
        for value in [1.0, 0.5, 2.0, 16_777_215.0, 1e7] {
            assert_eq!(
                primitive(value).to_bits(),
                pieces(value).to_bits(),
                "{value}"
            );
        }
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
        // Six rows: four side by side and two alone.
        for width in 1..=33 {
            let mut raster = accumulator(width, 6);
            let mut expected = Vec::new();
            for row in 0..6 {
                let mut winding = 0_i32;
                for column in 0..width {
                    let delta = deltas[(row + column) % deltas.len()];
                    raster.area[row * (width + 1) + column] = f32::from(delta) / 1024.0;
                    winding += i32::from(delta);
                    let level = (winding.abs().min(1024) * 255 + 512) / 1024;
                    expected.push(u8::try_from(level).unwrap());
                }
            }
            assert_eq!(raster.finish(width), expected);
        }
    }

    #[test]
    fn reused_scratch_is_zeroed_and_released_beyond_the_allocation_budget() {
        let mut raster = accumulator(32, 16);
        raster.area.fill(1.0);
        let limits = Limits {
            decoded_bytes: 44,
            ..Limits::default()
        };
        raster.reset(2, 2, limits).unwrap();
        assert!(raster.area.capacity() * size_of::<f32>() + 4 <= limits.decoded_bytes);
        assert_eq!(raster.finish(2), [0; 4]);
        raster.segment(0, 0.5, 0.5, 1.0, 0.0).unwrap();
        raster.reset(2, 2, limits).unwrap();
        assert_eq!((raster.work, raster.finish(2)), (0, vec![0; 4]));
        let smaller = Limits {
            decoded_bytes: 43,
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
        let mut mask = Coverage::default();
        for py in [0_u16, 1, 16, 127] {
            let rows = Rows::new(
                &glyph,
                f64::from(py) / 128.0,
                Limits::default(),
                &mut Clipping::default(),
            )
            .unwrap();
            for px in [0_u16, 1, 16, 127] {
                rows.mask(
                    f64::from(px) / 128.0,
                    Limits::default(),
                    &mut scratch,
                    &mut mask,
                )
                .unwrap();
                assert_eq!(mask.width, 4);
                assert!(
                    mask.data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|row| row[2..] == [0, 0])
                );
                for (index, a, b) in [
                    (0, 128 - px, 128 - py),
                    (1, px, 128 - py),
                    (4, 128 - px, py),
                    (5, px, py),
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "phases are multiples of 1/128"
    )]
    fn dealt_quads_match_fragments_added_in_clip_order() {
        // Curves and slants give one-cell, two-cell and longer fragments;
        // a rectangle at fractional offsets ends fragments on cell
        // boundaries at some phases.
        let p = Point::new;
        let mut path = PathBuilder::new();
        path.move_to(p(0.1, 0.0));
        path.cubic_to(p(3.9, 0.2), p(4.3, 5.1), p(0.7, 6.3))
            .unwrap();
        path.line_to(p(2.2, 3.1)).unwrap();
        path.close();
        path.move_to(p(5.0, 0.4));
        path.line_to(p(9.75, 1.0)).unwrap();
        path.line_to(p(5.5, 1.3)).unwrap();
        path.close();
        path.rect(6.25, 2.5, 2.0, 3.0);
        let path = Arc::new(path.finish());
        let limits = Limits::default();
        let (mut scratch, mut clipping) = (Accumulator::default(), Clipping::default());
        let (mut mask, mut checked) = (Coverage::default(), Coverage::default());
        let mut tested = 0;
        for (scale, shear) in [(0.37, 0.0), (1.0, 0.21), (2.6, -0.4), (7.1, 0.05)] {
            let transform = Transform::new(scale, 0.0, shear * scale, scale, 0.0, 0.0);
            for resolution in [Resolution::Low, Resolution::High, Resolution::Fine] {
                let glyph = Prepared::new(&path, transform, resolution, limits).unwrap();
                for py in [0_u16, 1, 37, 64, 127] {
                    let py = f64::from(py) / 128.0;
                    let rows = Rows::new(&glyph, py, limits, &mut clipping).unwrap();
                    let mut ordered = Vec::new();
                    clip(&glyph, py, |_, f| ordered.push(f)).unwrap();
                    assert_eq!(ordered.len(), rows.count);
                    for px in [0_u16, 1, 16, 63, 64, 100, 127] {
                        let px = f64::from(px) / 128.0;
                        let mut reference = accumulator(glyph.width, glyph.height);
                        for f in &ordered {
                            let (left, right) = (f.left + px as f32, f.right + px as f32);
                            reference.add(f.start as usize, left, right, f.height, f.scale);
                        }
                        let expected = reference.finish(glyph.width.next_multiple_of(4));
                        rows.mask(px, limits, &mut scratch, &mut mask).unwrap();
                        assert_eq!(mask.data, expected, "{scale} {resolution:?} {py} {px}");
                        // Beyond the work bound, which exceeds the work done
                        // by a cell per fragment, fragments are added one by
                        // one.
                        let tight = Limits {
                            raster_work: rows.work - 1,
                            ..limits
                        };
                        if glyph.width * glyph.height <= tight.raster_work {
                            rows.mask(px, tight, &mut scratch, &mut checked).unwrap();
                            assert_eq!(checked.data, expected);
                            tested += 1;
                        }
                    }
                }
            }
        }
        assert!(tested > 100, "{tested}");
    }

    #[test]
    fn row_spare_cells_reject_the_u32_extent_boundary() {
        let glyph = Prepared {
            outline: Arc::new(PathBuilder::new().finish()),
            edges: Vec::new(),
            x: 0,
            y: 0,
            width: 0,
            height: usize::try_from(u32::MAX).unwrap() - SPARE + 1,
            bytes: 0,
        };
        let error =
            Rows::new(&glyph, 0.0, Limits::default(), &mut Clipping::default()).unwrap_err();
        assert_eq!(error.message, "glyph extent budget exceeded");
    }

    #[test]
    fn warm_row_tables_and_padded_fragments_share_one_budget() {
        let limits = Limits::default();
        let mut tall = PathBuilder::new();
        tall.rect(0.0, 0.0, 1.0, 64.0);
        let tall = Prepared::new(
            &Arc::new(tall.finish()),
            Transform::IDENTITY,
            Resolution::Low,
            limits,
        )
        .unwrap();
        let mut clipping = Clipping::default();
        drop(Rows::new(&tall, 0.0, limits, &mut clipping).unwrap());
        let retained_tables =
            (clipping.starts.capacity() + clipping.rows.capacity()) * size_of::<usize>();

        // Three separated unit squares have six nonhorizontal edge
        // fragments: the last quad needs two transparent padding lanes.
        let mut short = PathBuilder::new();
        for x in [0.0, 2.0, 4.0] {
            short.rect(x, 0.0, 1.0, 1.0);
        }
        let short = Prepared::new(
            &Arc::new(short.finish()),
            Transform::IDENTITY,
            Resolution::Low,
            limits,
        )
        .unwrap();
        // The old tables alone fit, and the short glyph fits with compact
        // tables. Keeping both old tables alongside its fragments does not.
        // Derive the allowance from observed capacity, not exact allocation.
        let budget = retained_tables + size_of::<Quad>();
        let limited = Limits {
            decoded_bytes: 2 * budget,
            ..limits
        };
        let rows = Rows::new(&short, 0.0, limited, &mut clipping).unwrap();
        assert_eq!(rows.count, 6);
        let held = (clipping.starts.capacity() + clipping.rows.capacity()) * size_of::<usize>()
            + clipping.fragments.capacity() * size_of::<Fragment>()
            + rows.quads.capacity() * size_of::<Quad>();
        assert!(held <= budget, "retained payload {held} exceeds {budget}");
        let mut mask = Coverage::default();
        rows.mask(0.0, limited, &mut Accumulator::default(), &mut mask)
            .unwrap();
        assert_eq!(mask.width, 8);
        assert_eq!(
            mask.data,
            [255, 0, 255, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn partial_quad_padding_is_admitted_before_counting_rows() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let glyph = Prepared::new(
            &Arc::new(path.finish()),
            Transform::IDENTITY,
            Resolution::Low,
            Limits::default(),
        )
        .unwrap();
        // Two vertical edge fragments still occupy one entire quad. This
        // allowance is one byte below even the minimum vector payload.
        let required = (2 * glyph.height + 1) * size_of::<usize>()
            + 2 * size_of::<Fragment>()
            + size_of::<Quad>();
        let mut clipping = Clipping::default();
        drop(Rows::new(&glyph, 0.0, Limits::default(), &mut clipping).unwrap());
        assert!(clipping.starts.capacity() > 0 && clipping.fragments.capacity() > 0);
        let limits = Limits {
            decoded_bytes: 2 * (required - 1),
            ..Limits::default()
        };
        let error = Rows::new(&glyph, 0.0, limits, &mut clipping).unwrap_err();
        assert_eq!(error.message, "glyph row fragment budget exceeded");
        assert_eq!(
            clipping.starts.capacity() + clipping.rows.capacity() + clipping.fragments.capacity(),
            0
        );

        // With a normal allowance the same geometry paints the independent
        // unit-square mask, including a transparent padded column and row.
        let rows = Rows::new(&glyph, 0.0, Limits::default(), &mut clipping).unwrap();
        assert_eq!(rows.count, 2);
        let mut mask = Coverage::default();
        rows.mask(
            0.0,
            Limits::default(),
            &mut Accumulator::default(),
            &mut mask,
        )
        .unwrap();
        assert_eq!(mask.data, [255, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn row_budgets_stop_tall_glyphs_before_their_rows_are_counted() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let path = Arc::new(path.finish());
        // A bar one pixel wide and 400,000 rows tall: both its sides cross
        // every row.
        let tall = Transform::new(1.0, 0.0, 0.0, 400_000.0, 0.0, 0.0);
        let glyph = Prepared::new(&path, tall, Resolution::Low, Limits::default()).unwrap();
        let tables = (glyph.height + 1) * 2 * size_of::<usize>();
        let mut clipping = Clipping::default();
        // The row tables fit and a few fragments do; one side's rows do not.
        let fragments = Limits {
            decoded_bytes: 2 * (tables + 4096),
            ..Limits::default()
        };
        let error = Rows::new(&glyph, 0.0, fragments, &mut clipping).unwrap_err();
        assert_eq!(error.message, "glyph row fragment budget exceeded");
        assert!(clipping.fragments.capacity() <= 4096);
        // The row tables alone exceed this smaller budget. The failed
        // request leaves the already-empty row storage unallocated.
        let rows = Limits {
            decoded_bytes: tables / 2,
            ..Limits::default()
        };
        let error = Rows::new(&glyph, 0.0, rows, &mut clipping).unwrap_err();
        assert_eq!(error.message, "glyph row fragment budget exceeded");
        assert_eq!(clipping.starts.capacity() + clipping.rows.capacity(), 0);
    }

    #[test]
    fn last_bit_and_half_coverage_have_independent_rectangle_areas() {
        let mut raster = accumulator(64, 3);
        for [ax, ay, bx, by] in [[0.25, 0.5, 0.25, 1.5], [63.75, 1.5, 63.75, 0.5_f32]] {
            // Each vertical edge crosses rows 0 and 1 by half a pixel.
            let sign = if ay < by { 1.0 } else { -1.0 };
            raster.segment(0, ax, bx, 0.5 * sign, 0.0).unwrap();
            raster.segment(65, ax, bx, 0.5 * sign, 0.0).unwrap();
        }
        let output = raster.finish(64);
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
        for resolution in [Resolution::Low, Resolution::High, Resolution::Fine] {
            let placement = (transform, resolution);
            let (first, offset) = cached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            let first = first.data.to_vec();
            let retained = cached.masks.len();
            let (again, _) = cached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            assert_eq!(again.data, first);
            assert_eq!(cached.masks.len(), retained);
            let (fresh, fresh_offset) = uncached
                .coverage(&path, placement, limits, &mut scratch)
                .unwrap();
            assert_eq!((first.as_slice(), offset), (fresh.data, fresh_offset));
        }
        // Resolutions snap 4.7 to different vertical lattices: a key per resolution.
        let low = cached.coverage(&path, (transform, Resolution::Low), limits, &mut scratch);
        let low = low.unwrap().0.data.to_vec();
        let high = cached.coverage(&path, (transform, Resolution::High), limits, &mut scratch);
        assert_ne!(low.as_slice(), high.unwrap().0.data);
        assert_eq!(uncached.retained_bytes(), 0);
        assert!(cached.retained_bytes() <= 1 << 20);
    }
}
