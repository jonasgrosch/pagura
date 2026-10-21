//! Clip states (8.5.4) prepared once per render.
//!
//! The axis-aligned rectangles among a state's paths intersect into one
//! rectangle, which clips every mark's edges. The other paths are rows of
//! inside runs and boundary cells over the area: a boundary cell keeps its
//! coverage and its cell-local edges, so that a mark crossing it is
//! integrated together with the clip's geometry rather than multiplied.

use super::approximate::Product;
use super::certify::certified;
use super::edges::{self, Bounds, Chained, Polygon};
use super::rect::rectangle;
use super::resolve::{LocalEdge, Resolver};
use super::scan::{RowIndex, Scanner};
use super::{DeviceRect, Precision, ShapeOptions};
use crate::{FillRule, Path, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

/// The message of every clip preparation budget error.
const CLIP_WORK: &str = "clip raster work budget exceeded";

/// A clip state prepared once per render: the intersection of a chain of
/// clipping paths, classified per band row into outside, inside and
/// boundary runs. A rectangular clip becomes edge clipping. Cloning shares
/// the prepared state.
#[derive(Debug, Clone)]
pub struct ClipShape {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    bounds: DeviceRect,
    rect: Option<Bounds>,
    rows: Option<Rows>,
    product: Option<Product>,
    work: usize,
    retained: usize,
}

/// A run of one clip row: inside pixels, or one boundary cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) x0: u32,
    pub(crate) x1: u32,
    /// The boundary cell, or [`INSIDE`].
    pub(crate) cell: u32,
}

pub(crate) const INSIDE: u32 = u32::MAX;

/// A boundary cell: its coverage, and the edges and rules of the paths
/// that cross it, numbered from zero.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cell {
    pub(crate) value: u8,
    edges: (u32, u32),
    rules: (u32, u32),
}

/// The classified rows of a clip's non-rectangular paths.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    index: Vec<(u32, u32)>,
    segments: Vec<Segment>,
    cells: Vec<Cell>,
    edges: Vec<LocalEdge>,
    rules: Vec<FillRule>,
}

impl Rows {
    /// The runs of row `y`, left to right; pixels between them are outside.
    pub(crate) fn row(&self, y: u32) -> &[Segment] {
        let index = self.index.partition_point(|&(row, _)| row < y);
        match self.index.get(index) {
            Some(&(row, end)) if row == y => {
                let start = index.checked_sub(1).map_or(0, |i| self.index[i].1);
                &self.segments[start as usize..end as usize]
            }
            _ => &[],
        }
    }

    pub(crate) fn cell(&self, index: u32) -> Option<(&Cell, &[LocalEdge], &[FillRule])> {
        let cell = self.cells.get(index as usize)?;
        Some((
            cell,
            self.edges.get(cell.edges.0 as usize..cell.edges.1 as usize)?,
            self.rules.get(cell.rules.0 as usize..cell.rules.1 as usize)?,
        ))
    }

    fn retained_bytes(&self) -> usize {
        (self.index.capacity() * 8)
            .saturating_add(self.segments.capacity() * size_of::<Segment>())
            .saturating_add(self.cells.capacity() * size_of::<Cell>())
            .saturating_add(self.edges.capacity() * size_of::<LocalEdge>())
            .saturating_add(self.rules.capacity() * size_of::<FillRule>())
    }

    fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<()> {
        values
            .try_reserve(count)
            .map_err(|_| Error::limit("clip row allocation failed"))
    }

    fn push_segment(&mut self, y: u32, segment: Segment) -> Result<()> {
        if let Some(last) = self.segments.last_mut()
            && self.index.last().is_some_and(|&(row, _)| row == y)
            && last.cell == INSIDE
            && segment.cell == INSIDE
            && last.x1 == segment.x0
        {
            last.x1 = segment.x1;
            return Ok(());
        }
        Self::reserve(&mut self.segments, 1)?;
        Self::reserve(&mut self.index, 1)?;
        self.segments.push(segment);
        let end = u32::try_from(self.segments.len()).map_err(|_| Error::limit("clip rows exceed u32"))?;
        match self.index.last_mut() {
            Some(last) if last.0 == y => last.1 = end,
            _ => self.index.push((y, end)),
        }
        Ok(())
    }
}

/// One path of a clip, as prepared.
pub(crate) struct Group {
    pub(crate) polygon: Polygon,
    pub(crate) rule: FillRule,
}

/// The fill a clipping path of `path` under `transform` and `rule` paints:
/// a certified empty region becomes its hairline outline (10.7.4).
pub(crate) fn fill_polygon(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    tolerance: Option<f64>,
    limits: &Limits,
) -> Result<(Polygon, FillRule)> {
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if let Some(outline) = crate::empty_fill::outline(path, transform, rule, limits)? {
        return Ok((
            Polygon::from_path(&outline, Transform::IDENTITY, tolerance, limits)?,
            FillRule::NonZero,
        ));
    }
    Ok((Polygon::from_path(path, transform, tolerance, limits)?, rule))
}

impl ClipShape {
    /// The intersection of `paths` (user space, outermost first) under
    /// `transform`.
    ///
    /// # Errors
    ///
    /// As [`Shape::fill`](super::Shape::fill), plus a `Limit` error beyond
    /// `mask_sources`.
    pub fn new(
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Self> {
        let limits = options.limits;
        if paths.len() > limits.mask_sources {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let region = options.area.region;
        let mut rect: Option<Bounds> = None;
        let mut groups = Vec::new();
        for &(path, rule) in paths {
            let (polygon, rule) = fill_polygon(path, transform, rule, None, limits)?;
            if let Some(r) = rectangle(&polygon) {
                rect = Some(rect.map_or(r, |b| b.intersect(r)));
            } else {
                groups
                    .try_reserve(1)
                    .map_err(|_| Error::limit("clip allocation failed"))?;
                groups.push(Group { polygon, rule });
            }
        }
        let mut bounds = region;
        if let Some(r) = rect {
            bounds = bounds.intersect(&r.pixels(region));
        }
        for group in &groups {
            let pixels = group
                .polygon
                .bounds()
                .map_or_else(DeviceRect::default, |b| edges::pixels(b, region, false));
            bounds = bounds.intersect(&pixels);
        }
        let mut inner = Inner {
            bounds,
            rect,
            rows: None,
            product: None,
            work: 0,
            retained: size_of::<Inner>(),
        };
        if !groups.is_empty() && !bounds.is_empty() {
            if options.precision == Precision::Approximate {
                let product = Product::new(paths, transform, rect, options, bounds)?;
                inner.retained = inner.retained.saturating_add(product.retained_bytes());
                inner.work = product.work();
                inner.product = Some(product);
            }
            let rows = classify(&groups, rect, bounds, options)?;
            inner.work = inner
                .work
                .saturating_add(rows.segments.len())
                .saturating_add(bounds.height() as usize * 2);
            inner.retained = inner.retained.saturating_add(rows.retained_bytes());
            inner.rows = Some(rows);
        } else {
            inner.work = bounds.height() as usize * 4;
        }
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// The pixels the clip can keep within the area; empty when it removes
    /// everything.
    pub fn bounds(&self) -> DeviceRect {
        self.inner.bounds
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.inner.retained
    }

    /// Work units rasterizing the clip alone over every band costs at most.
    pub fn work(&self) -> usize {
        self.inner.work
    }

    pub(crate) fn rect(&self) -> Option<Bounds> {
        self.inner.rect
    }

    pub(crate) fn rows(&self) -> Option<&Rows> {
        self.inner.rows.as_ref()
    }

    pub(crate) fn product(&self) -> Option<&Product> {
        self.inner.product.as_ref()
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// One path's state over a run of one row.
#[derive(Debug, Clone, Copy)]
enum State {
    Empty,
    Full,
    /// A boundary cell, its coverage if one chain crosses it, and its edges.
    Partial(Option<u8>, (u32, u32)),
}

/// Classifies the rows of `bounds` for the non-rectangular `groups`.
fn classify(
    groups: &[Group],
    rect: Option<Bounds>,
    bounds: DeviceRect,
    options: &ShapeOptions<'_>,
) -> Result<Rows> {
    let limits = options.limits;
    let mut meter = Meter::new(limits.raster_work, CLIP_WORK);
    let width = super::row::width(bounds.x1)?.saturating_add(1);
    let mut scanner = Scanner::new(width, &mut meter)?;
    let mut prepared = Vec::new();
    prepared
        .try_reserve(groups.len())
        .map_err(|_| Error::limit("clip allocation failed"))?;
    let mut next = 0;
    for (g, group) in (0_u8..).zip(groups) {
        let mut chained = Vec::new();
        edges::chained(&group.polygon, g, &mut next, &mut chained)?;
        let chained = match rect {
            Some(r) => {
                let mut clipped = Vec::new();
                for &edge in &chained {
                    edges::clip(edge, r, &mut clipped)?;
                }
                clipped
            }
            None => chained,
        };
        let index = RowIndex::new(&chained, (bounds.y0, bounds.y1 - 1), &mut meter)?;
        let range = scan_range(&chained, width);
        let certain = certified(&group.polygon, 0.0);
        prepared.push((chained, index, range, certain, group.rule));
    }
    let mut rows = Rows::default();
    let mut runs: Vec<Vec<(u32, u32, State)>> = Vec::new();
    runs.try_reserve(groups.len())
        .map_err(|_| Error::limit("clip allocation failed"))?;
    runs.resize_with(groups.len(), Vec::new);
    let mut local = Vec::new();
    let mut resolver = Resolver::default();
    for y in bounds.y0..bounds.y1 {
        local.clear();
        for (g, (chained, index, range, certain, rule)) in prepared.iter().enumerate() {
            let result = group_row(
                &mut scanner,
                (chained, index.row(y), y),
                (*range, bounds),
                (*certain, *rule, u8::try_from(g).unwrap_or(u8::MAX)),
                (&mut runs[g], &mut local),
                &mut meter,
            );
            scanner.unload();
            if let Err(error) = result {
                scanner.recover();
                return Err(error);
            }
        }
        merge(&runs, &prepared, (y, bounds), &local, &mut resolver, &mut rows, &mut meter)?;
    }
    Ok(rows)
}

/// The columns a group's cells can be touched in: from its leftmost edge
/// (or the page's left side) to one past its rightmost.
pub(crate) fn scan_range(edges: &[Chained], width: usize) -> (usize, usize) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for e in edges {
        lo = lo.min(e.line.a.x).min(e.line.b.x);
        hi = hi.max(e.line.a.x).max(e.line.b.x);
    }
    columns(lo, hi, width)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the row width before conversion"
)]
pub(crate) fn columns(lo: f64, hi: f64, width: usize) -> (usize, usize) {
    if lo > hi {
        return (0, 0);
    }
    let w = width as f64;
    let from = lo.floor().clamp(0.0, w) as usize;
    let to = (hi.ceil() + 1.0).clamp(0.0, w) as usize;
    (from, to.max(from))
}

/// Loads one group's row and appends its runs within `bounds` to `runs`.
fn group_row(
    scanner: &mut Scanner,
    (edges, items, y): (&[Chained], &[u32], u32),
    (range, bounds): ((usize, usize), DeviceRect),
    (certain, rule, group): (bool, FillRule, u8),
    (runs, local): (&mut Vec<(u32, u32, State)>, &mut Vec<LocalEdge>),
    meter: &mut Meter,
) -> Result<()> {
    runs.clear();
    scanner.load(edges, items, y, range, meter)?;
    let (x0, x1) = (bounds.x0, bounds.x1);
    let mut position = x0;
    let mut gap = State::Empty;
    let push = |runs: &mut Vec<(u32, u32, State)>, a: u32, b: u32, state: State| -> Result<()> {
        if a < b {
            runs.try_reserve(1)
                .map_err(|_| Error::limit("clip row allocation failed"))?;
            runs.push((a, b, state));
        }
        Ok(())
    };
    let solid = |value: u8| if value >= 128 { State::Full } else { State::Empty };
    for index in 0..scanner.cells.len() {
        let cell = scanner.cells[index];
        if cell.x >= x1 {
            break;
        }
        let after = solid(super::row::fold(cell.cover, rule));
        if cell.x >= x0 {
            push(runs, position, cell.x, gap)?;
            let state = if cell.listed {
                let start = u32::try_from(local.len()).map_err(|_| Error::limit("clip edges exceed u32"))?;
                scanner.local_edges(&cell, group, local)?;
                let end = u32::try_from(local.len()).map_err(|_| Error::limit("clip edges exceed u32"))?;
                let value = (certain || cell.certain).then(|| cell.value(rule));
                State::Partial(value, (start, end))
            } else {
                solid(cell.value(rule))
            };
            push(runs, cell.x, cell.x + 1, state)?;
            position = cell.x + 1;
        }
        gap = after;
    }
    push(runs, position.max(x0), x1, gap)?;
    meter.charge(runs.len())?;
    Ok(())
}

type Prepared = (Vec<Chained>, RowIndex, (usize, usize), bool, FillRule);

/// Combines the groups' runs of row `y` into the clip's segments.
fn merge(
    runs: &[Vec<(u32, u32, State)>],
    prepared: &[Prepared],
    (y, bounds): (u32, DeviceRect),
    local: &[LocalEdge],
    resolver: &mut Resolver,
    rows: &mut Rows,
    meter: &mut Meter,
) -> Result<()> {
    let mut cursor = vec![0_usize; runs.len()];
    let mut x = bounds.x0;
    let mut partial: Vec<(usize, Option<u8>, (u32, u32))> = Vec::new();
    while x < bounds.x1 {
        let mut next = bounds.x1;
        let mut empty = false;
        partial.clear();
        for (g, group) in runs.iter().enumerate() {
            while group.get(cursor[g]).is_some_and(|&(_, end, _)| end <= x) {
                cursor[g] += 1;
            }
            let Some(&(_, end, state)) = group.get(cursor[g]) else {
                empty = true;
                continue;
            };
            next = next.min(end);
            match state {
                State::Empty => empty = true,
                State::Full => {}
                State::Partial(value, edges) => partial.push((g, value, edges)),
            }
        }
        meter.charge(runs.len())?;
        if empty {
            x = next;
            continue;
        }
        if partial.is_empty() {
            rows.push_segment(
                y,
                Segment {
                    x0: x,
                    x1: next,
                    cell: INSIDE,
                },
            )?;
            x = next;
            continue;
        }
        let edge_start = u32::try_from(rows.edges.len()).map_err(|_| Error::limit("clip edges exceed u32"))?;
        let rule_start = u32::try_from(rows.rules.len()).map_err(|_| Error::limit("clip rules exceed u32"))?;
        for (renumbered, &(g, _, (a, b))) in (0_u8..).zip(&partial) {
            let edges = local
                .get(a as usize..b as usize)
                .ok_or(Error::invalid_argument("clip edges out of range"))?;
            Rows::reserve(&mut rows.edges, edges.len())?;
            rows.edges.extend(edges.iter().map(|e| LocalEdge {
                group: renumbered,
                ..*e
            }));
            Rows::reserve(&mut rows.rules, 1)?;
            rows.rules.push(prepared[g].4);
        }
        let edge_end = u32::try_from(rows.edges.len()).map_err(|_| Error::limit("clip edges exceed u32"))?;
        let rule_end = u32::try_from(rows.rules.len()).map_err(|_| Error::limit("clip rules exceed u32"))?;
        let value = match partial.as_slice() {
            [(_, Some(value), _)] => *value,
            _ => resolver.coverage(
                &rows.edges[edge_start as usize..edge_end as usize],
                &rows.rules[rule_start as usize..rule_end as usize],
                meter,
            )?,
        };
        let cell = u32::try_from(rows.cells.len()).map_err(|_| Error::limit("clip cells exceed u32"))?;
        Rows::reserve(&mut rows.cells, 1)?;
        rows.cells.push(Cell {
            value,
            edges: (edge_start, edge_end),
            rules: (rule_start, rule_end),
        });
        rows.push_segment(
            y,
            Segment {
                x0: x,
                x1: x + 1,
                cell,
            },
        )?;
        x += 1;
    }
    Ok(())
}
