//! Cell analysis at preparation: which cells the signed area alone does not
//! determine, and their exact coverage.
//!
//! A cell crossed by one monotone chain has winding numbers w and w ± 1
//! only, so its signed area folds to its covered area under either rule;
//! so does a cell no edge crosses. A cell that pieces of two or more chains
//! cross, or that a mark and a clip boundary both cross, goes to the local
//! resolver. The resolver needs the winding along the cell's left side:
//! it starts at the cell's upper left corner and changes where pieces meet
//! that side. Integrating it must give the cover carried into the cell,
//! which determines the corner's winding exactly (an integer, rounded from
//! a sum whose error is far below one half).

use super::edges::{Chained, Line, Place, rows};
use super::resolve::{LocalEdge, Resolver};
use super::row::{Row, fold};
use crate::{FillRule, Point};
use pagura_core::{Error, Meter, Result};

const NONE: u32 = u32::MAX;
const MULTIPLE: u32 = u32::MAX - 1;

/// One edge's part within one row: from `(xa, ya)` to `(xb, yb)` in the
/// edge's direction, with `ya <= yb` unless it is horizontal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowPiece {
    xa: f64,
    ya: f64,
    xb: f64,
    yb: f64,
    /// +1 downward, -1 upward, 0 horizontal.
    direction: f64,
    /// Whether the edge runs from `(xa, ya)` to `(xb, yb)`.
    forward: bool,
    chain: u32,
    group: u8,
    enclosed: bool,
    place: Place,
    /// Whether `(xa, ya)` and `(xb, yb)` are the edge's own end points
    /// rather than where the row cuts it.
    vertices: [bool; 2],
}

impl RowPiece {
    /// The piece of `edge` within row `top..top + 1`, if it has one there;
    /// horizontal edges count only strictly inside the row.
    pub(crate) fn new(edge: Chained, top: f64) -> Option<Self> {
        let Line { a, b } = edge.line;
        if a.y == b.y {
            return (a.y > top && a.y < top + 1.0 && a.x != b.x).then_some(Self {
                xa: a.x,
                ya: a.y,
                xb: b.x,
                yb: b.y,
                direction: 0.0,
                forward: true,
                chain: edge.chain,
                group: edge.group,
                enclosed: edge.enclosed,
                place: edge.place,
                vertices: [true, true],
            });
        }
        let down = a.y < b.y;
        let (upper, lower) = if down { (a, b) } else { (b, a) };
        let lo = upper.y.max(top);
        let hi = lower.y.min(top + 1.0);
        let ends = [lo == upper.y, hi == lower.y];
        (lo < hi).then(|| Self {
            xa: edge.line.x(lo),
            ya: lo,
            xb: edge.line.x(hi),
            yb: hi,
            direction: if down { 1.0 } else { -1.0 },
            forward: down,
            chain: edge.chain,
            group: edge.group,
            enclosed: edge.enclosed,
            place: edge.place,
            vertices: if down { ends } else { [ends[1], ends[0]] },
        })
    }

    /// The columns whose interior the piece meets, or the column whose left
    /// side a vertical piece lies on.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "device coordinates are bounded by 10^12, within i64"
    )]
    fn columns(&self) -> (i64, i64) {
        let lo = self.xa.min(self.xb);
        let hi = self.xa.max(self.xb);
        let first = lo.floor() as i64;
        let last = (hi.ceil() as i64 - 1).max(first);
        (first, last)
    }

    fn y_at(&self, x: f64) -> f64 {
        self.ya + (x - self.xa) / (self.xb - self.xa) * (self.yb - self.ya)
    }

    /// Whether one of the edge's own end points lies on a side of column
    /// `c` within heights `span` of row `top`: a region may touch the side
    /// there at a single point.
    fn touches(&self, c: f64, (top, span): (f64, (f64, f64))) -> bool {
        [(self.xa, self.ya), (self.xb, self.yb)]
            .into_iter()
            .zip(self.vertices)
            .any(|((x, y), vertex)| {
                vertex
                    && x >= c
                    && x <= c + 1.0
                    && (x == c || x == c + 1.0 || y - top == span.0 || y - top == span.1)
            })
    }

    /// The part of the piece within column `c`, in cell coordinates, unless
    /// it lies on the column's left side.
    fn part(&self, c: f64, top: f64) -> Option<[(f64, f64); 2]> {
        if self.xa == self.xb && self.xa == c {
            return None;
        }
        let clamp = |x: f64, y: f64| {
            if x < c {
                (c, self.y_at(c))
            } else if x > c + 1.0 {
                (c + 1.0, self.y_at(c + 1.0))
            } else {
                (x, y)
            }
        };
        let (pa, pb) = if self.xa == self.xb {
            ((self.xa, self.ya), (self.xb, self.yb))
        } else {
            (clamp(self.xa, self.ya), clamp(self.xb, self.yb))
        };
        Some([(pa.0 - c, pa.1 - top), (pb.0 - c, pb.1 - top)])
    }

    /// Adds this piece's edges within column `c` of row `top` to `out` as
    /// group `group`: its part inside the column, and its crossing of the
    /// column's left side as a change of the winding there. Returns the
    /// cover its part left of the column carries in.
    fn local(&self, c: f64, (top, end): (f64, f64), group: u8, out: &mut Vec<LocalEdge>) -> f64 {
        let vertical = self.xa == self.xb;
        if vertical && self.xa == c {
            // On the left side: counted in the cover carried in.
            return 0.0;
        }
        let clamp = |x: f64, y: f64| {
            if x < c {
                (c, self.y_at(c))
            } else if x > c + 1.0 {
                (c + 1.0, self.y_at(c + 1.0))
            } else {
                (x, y)
            }
        };
        let (pa, pb) = if vertical {
            ((self.xa, self.ya), (self.xb, self.yb))
        } else {
            (clamp(self.xa, self.ya), clamp(self.xb, self.yb))
        };
        let mut carried = 0.0;
        if self.direction != 0.0 && self.xa.min(self.xb) < c {
            let y = self.y_at(c);
            let height = if self.xa < c { y - self.ya } else { self.yb - y };
            carried = self.direction * height;
        }
        let traversal = if self.forward { pb.0 - pa.0 } else { pa.0 - pb.0 };
        if traversal != 0.0 {
            let delta = if traversal > 0.0 { -1 } else { 1 };
            for (x, y) in [pa, pb] {
                if x == c && y - top < end {
                    out.push(LocalEdge {
                        x0: 0.0,
                        y0: y - top,
                        x1: 0.0,
                        y1: end,
                        winding: delta,
                        group,
                    });
                }
            }
        }
        if self.direction != 0.0 {
            out.push(LocalEdge {
                x0: pa.0 - c,
                y0: pa.1 - top,
                x1: pb.0 - c,
                y1: pb.1 - top,
                #[expect(clippy::cast_possible_truncation, reason = "a unit direction")]
                winding: self.direction as i32,
                group,
            });
        }
        carried
    }
}

/// What a cell's perimeter tells about its windings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walk {
    /// The signed area folds to the coverage.
    Fold,
    /// The coverage, whatever the signed area.
    Value(u8),
    /// The resolver decides.
    Unknown,
}

/// The windings met along a cell's sides: their least and greatest, and
/// which parities (bit 0 even, bit 1 odd).
#[derive(Debug, Clone, Copy)]
struct Windings {
    lo: i64,
    hi: i64,
    parities: u8,
}

impl Windings {
    fn add(&mut self, w: i64) {
        self.lo = self.lo.min(w);
        self.hi = self.hi.max(w);
        self.parities |= 1 << (w & 1);
    }
}

/// The windings just inside the sides of a cell whose edges (cell
/// coordinates, the left side's windings as edges at x = 0) do not cross,
/// over heights `span`; `None` when the cell has no height.
fn perimeter(edges: &[LocalEdge], (bottom, top): (f64, f64)) -> Option<Windings> {
    if top <= bottom {
        return None;
    }
    let mut windings = Windings {
        lo: i64::MAX,
        hi: i64::MIN,
        parities: 0,
    };
    let mut heights: Vec<f64> = Vec::new();
    heights.try_reserve(2 * edges.len() + 2).ok()?;
    heights.extend([bottom, top]);
    heights.extend(edges.iter().flat_map(|e| [e.y0, e.y1]).filter(|&y| y > bottom && y < top));
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    // The left and right sides, between consecutive end heights.
    for pair in heights.windows(2) {
        let (mut left, mut right) = (0, 0);
        for e in edges.iter().filter(|e| e.y0 <= pair[0] && e.y1 >= pair[1]) {
            right += i64::from(e.winding);
            if e.x0 == 0.0 && e.x1 == 0.0 {
                left += i64::from(e.winding);
            }
        }
        windings.add(left);
        windings.add(right);
    }
    // The top and bottom sides, left to right.
    let mut ends: Vec<(f64, i64)> = Vec::new();
    ends.try_reserve(edges.len()).ok()?;
    for (side, at) in [(bottom, true), (top, false)] {
        ends.clear();
        for e in edges {
            if at && e.y0 == side && e.y1 > side {
                ends.push((e.x0, i64::from(e.winding)));
            } else if !at && e.y1 == side && e.y0 < side {
                ends.push((e.x1, i64::from(e.winding)));
            }
        }
        ends.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut w = 0;
        for &(_, winding) in &ends {
            w += winding;
            windings.add(w);
        }
    }
    (windings.lo <= windings.hi).then_some(windings)
}

/// Whether two segments in cell coordinates meet, other than at one shared
/// end point of `adjacent` edges.
fn meet([a, b]: [(f64, f64); 2], [c, d]: [(f64, f64); 2], adjacent: bool) -> bool {
    let orient = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        let v = (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let within = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        p.0.min(q.0) <= r.0 && r.0 <= p.0.max(q.0) && p.1.min(q.1) <= r.1 && r.1 <= p.1.max(q.1)
    };
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    if o1 * o2 < 0 && o3 * o4 < 0 {
        return true;
    }
    let shared = [(a, c), (a, d), (b, c), (b, d)]
        .iter()
        .filter(|(p, q)| p == q)
        .count();
    let collinear = o1 == 0 && o2 == 0;
    let touch = (o1 == 0 && within(a, b, c))
        || (o2 == 0 && within(a, b, d))
        || (o3 == 0 && within(c, d, a))
        || (o4 == 0 && within(c, d, b));
    touch && (!adjacent || shared != 1 || collinear)
}

/// Resolved cells per row, in row then column order.
#[derive(Debug, Default)]
pub(crate) struct Stored {
    rows: Vec<(u32, u32)>,
    cells: Vec<(u32, u8)>,
}

impl Stored {
    /// The resolved cells of row `y`, by column.
    pub(crate) fn row(&self, y: u32) -> &[(u32, u8)] {
        let index = self.rows.partition_point(|&(row, _)| row < y);
        match self.rows.get(index) {
            Some(&(row, end)) if row == y => {
                let start = index.checked_sub(1).map_or(0, |i| self.rows[i].1);
                &self.cells[start as usize..end as usize]
            }
            _ => &[],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn push(&mut self, y: u32, x: u32, value: u8) -> Result<()> {
        self.cells
            .try_reserve(1)
            .and_then(|()| self.rows.try_reserve(1))
            .map_err(|_| Error::limit("resolved cell allocation failed"))?;
        self.cells.push((x, value));
        let end = u32::try_from(self.cells.len()).map_err(|_| Error::limit("resolved cells exceed u32"))?;
        match self.rows.last_mut() {
            Some(last) if last.0 == y => last.1 = end,
            _ => self.rows.push((y, end)),
        }
        Ok(())
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.rows
            .capacity()
            .saturating_mul(size_of::<(u32, u32)>())
            .saturating_add(self.cells.capacity().saturating_mul(size_of::<(u32, u8)>()))
    }
}

/// One analysed cell of a row: its column, its signed area and the cover
/// carried up to and including it, and whether pieces cross it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cell {
    pub(crate) x: u32,
    pub(crate) area: f64,
    pub(crate) cover: f64,
    /// Pieces cross the cell (or lie on its left side).
    pub(crate) listed: bool,
    /// The signed area folds to the coverage: no two chains meet here and
    /// the row is whole.
    pub(crate) certain: bool,
}

/// The constant winding of a cell no piece crosses, over the heights
/// `span` of its row, from the cover carried into it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a winding number bounded by the row's pieces"
)]
pub(crate) fn gap_winding(cover: f64, (bottom, top): (f64, f64)) -> i64 {
    if top > bottom {
        (cover / (top - bottom)).round() as i64
    } else {
        0
    }
}

/// Whether `winding` is inside under `rule`.
pub(crate) fn inside(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

impl Cell {
    pub(crate) fn value(&self, rule: FillRule) -> u8 {
        fold(self.area + self.cover, rule)
    }
}

/// Reusable analysis storage for the cells of one column range.
#[derive(Debug)]
pub(crate) struct Scanner {
    row: Row,
    base: usize,
    head: Vec<u32>,
    tag: Vec<u32>,
    nodes: Vec<(u32, u32)>,
    pieces: Vec<RowPiece>,
    listed: Vec<u32>,
    pub(crate) cells: Vec<Cell>,
    local: Vec<LocalEdge>,
    resolver: Resolver,
    top: f64,
    /// The heights of the current row the geometry is clipped to, from
    /// its top.
    pub(crate) span: (f64, f64),
}

impl Scanner {
    /// Storage for the cells of columns `from..to`.
    pub(crate) fn new((from, to): (usize, usize), meter: &mut Meter) -> Result<Self> {
        let width = to.saturating_sub(from);
        let mut scanner = Self {
            row: Row::from(from, width, meter)?,
            base: from,
            head: Vec::new(),
            tag: Vec::new(),
            nodes: Vec::new(),
            pieces: Vec::new(),
            listed: Vec::new(),
            cells: Vec::new(),
            local: Vec::new(),
            resolver: Resolver::default(),
            top: 0.0,
            span: (0.0, 1.0),
        };
        meter.reserve(&mut scanner.head, width)?;
        meter.reserve(&mut scanner.tag, width)?;
        scanner.head.resize(width, NONE);
        scanner.tag.resize(width, NONE);
        Ok(scanner)
    }


    /// Loads row `y` from the edges `items` selects, accumulating their
    /// signed areas and listing their pieces per cell, then collects the
    /// cells of columns `range` in [`Self::cells`]. The edges are clipped to
    /// heights `span` of the row. `meter` is charged per piece and cell.
    pub(crate) fn load(
        &mut self,
        edges: &[Chained],
        (items, y, span): (&[u32], u32, (f64, f64)),
        range: (usize, usize),
        meter: &mut Meter,
    ) -> Result<()> {
        self.top = f64::from(y);
        self.span = span;
        let whole = span == (0.0, 1.0);
        self.pieces.clear();
        self.nodes.clear();
        let base = i64::try_from(self.base).unwrap_or(i64::MAX);
        let end = i64::try_from(self.row.end()).unwrap_or(i64::MAX);
        for &item in items {
            let Some(&edge) = edges.get(item as usize) else {
                continue;
            };
            let Some(piece) = RowPiece::new(edge, self.top) else {
                continue;
            };
            meter.charge(1)?;
            if piece.direction != 0.0 {
                let cells = self.row.line(edge.line, self.top);
                meter.charge(cells)?;
            }
            let id = u32::try_from(self.pieces.len()).map_err(|_| Error::limit("row pieces exceed u32"))?;
            self.pieces
                .try_reserve(1)
                .map_err(|_| Error::limit("row piece allocation failed"))?;
            self.pieces.push(piece);
            let (first, last) = piece.columns();
            let (first, last) = (first.max(base), last.min(end - 1));
            if first > last {
                continue;
            }
            meter.charge(usize::try_from(last - first).unwrap_or(usize::MAX))?;
            self.nodes
                .try_reserve(usize::try_from(last - first + 1).unwrap_or(usize::MAX))
                .map_err(|_| Error::limit("cell list allocation failed"))?;
            for column in first..=last {
                #[expect(clippy::cast_sign_loss, reason = "clamped to the row")]
                let x = (column - base) as usize;
                let node = u32::try_from(self.nodes.len()).map_err(|_| Error::limit("cell lists exceed u32"))?;
                self.nodes.push((id, self.head[x]));
                if self.head[x] == NONE {
                    self.listed
                        .try_reserve(1)
                        .map_err(|_| Error::limit("cell list allocation failed"))?;
                    self.listed.push(column as u32);
                    self.row.add_cell(column, 0.0, 0.0);
                }
                self.head[x] = node;
                self.tag[x] = match self.tag[x] {
                    NONE => piece.chain,
                    tag if tag == piece.chain => tag,
                    _ => MULTIPLE,
                };
            }
        }
        self.cells.clear();
        let (head, tag, cells, base) = (&self.head, &self.tag, &mut self.cells, self.base);
        let mut failed = false;
        self.row.drain(range, |x, area, cover| {
            if cells.try_reserve(1).is_err() {
                failed = true;
                return;
            }
            let i = x - base;
            let listed = head.get(i).is_some_and(|&h| h != NONE);
            cells.push(Cell {
                x: u32::try_from(x).unwrap_or(u32::MAX),
                area,
                cover,
                listed,
                certain: whole && tag.get(i).is_none_or(|&t| t != MULTIPLE),
            });
        });
        if failed {
            return Err(Error::limit("cell analysis allocation failed"));
        }
        meter.charge(self.cells.len())?;
        Ok(())
    }

    /// Clears the lists of the row loaded last.
    pub(crate) fn unload(&mut self) {
        for &x in &self.listed {
            let i = (x as usize).wrapping_sub(self.base);
            if let Some(h) = self.head.get_mut(i) {
                *h = NONE;
            }
            if let Some(t) = self.tag.get_mut(i) {
                *t = NONE;
            }
        }
        self.listed.clear();
    }

    /// Clears everything after an error left a row half loaded.
    pub(crate) fn recover(&mut self) {
        self.unload();
        self.row.reset();
    }

    /// Appends to `out` the edges of `cell` as group `group`: the pieces
    /// within it and the winding along its left side.
    pub(crate) fn local_edges(&self, cell: &Cell, group: u8, out: &mut Vec<LocalEdge>) -> Result<()> {
        let c = f64::from(cell.x);
        let start = out.len();
        let mut carried = cell.cover;
        let mut node = self
            .head
            .get((cell.x as usize).wrapping_sub(self.base))
            .copied()
            .unwrap_or(NONE);
        while node != NONE {
            let Some(&(piece, next)) = self.nodes.get(node as usize) else {
                break;
            };
            if let Some(piece) = self.pieces.get(piece as usize) {
                out.try_reserve(3)
                    .map_err(|_| Error::limit("cell edge allocation failed"))?;
                carried += piece.local(c, (self.top, self.span.1), group, out);
            }
            node = next;
        }
        // The winding at the top of the span on the left side: the carried
        // cover less what the changes along that side contribute.
        let (bottom, top) = self.span;
        let mut steps = 0.0;
        for e in &out[start..] {
            if e.x0 == 0.0 && e.x1 == 0.0 && e.y1 == top {
                steps += f64::from(e.winding) * (top - e.y0);
            }
        }
        let corner = gap_winding(carried - steps, self.span);
        if corner != 0 {
            let winding = i32::try_from(corner).map_err(|_| Error::limit("cell winding exceeds i32"))?;
            out.try_reserve(1)
                .map_err(|_| Error::limit("cell edge allocation failed"))?;
            out.push(LocalEdge {
                x0: 0.0,
                y0: bottom,
                x1: 0.0,
                y1: top,
                winding,
                group,
            });
        }
        Ok(())
    }

    /// Appends to `out` the edges of a cell no piece crosses, into which
    /// `cover` is carried, as group `group`.
    pub(crate) fn gap_edges(&self, cover: f64, group: u8, out: &mut Vec<LocalEdge>) -> Result<()> {
        let winding = gap_winding(cover, self.span);
        if winding != 0 {
            out.try_reserve(1)
                .map_err(|_| Error::limit("cell edge allocation failed"))?;
            out.push(LocalEdge {
                x0: 0.0,
                y0: self.span.0,
                x1: 0.0,
                y1: self.span.1,
                winding: i32::try_from(winding).map_err(|_| Error::limit("cell winding exceeds i32"))?,
                group,
            });
        }
        Ok(())
    }

    /// Certifies a cell that pieces of several chains cross without
    /// meeting: every region then reaches the cell's sides, so the windings
    /// along them are all the cell has. Two neighbouring values let the
    /// signed area fold; one parity under the even-odd rule gives an empty or
    /// full cell. Anything else, or pieces that meet, is left to the
    /// resolver.
    pub(crate) fn walk(&mut self, cell: &Cell, rule: FillRule, meter: &mut Meter) -> Result<Walk> {
        let c = f64::from(cell.x);
        let mut parts: Vec<([(f64, f64); 2], u32, Place)> = Vec::new();
        let mut node = self
            .head
            .get((cell.x as usize).wrapping_sub(self.base))
            .copied()
            .unwrap_or(NONE);
        while node != NONE {
            let Some(&(piece, next)) = self.nodes.get(node as usize) else {
                break;
            };
            if let Some(piece) = self.pieces.get(piece as usize) {
                if piece.enclosed || piece.touches(c, (self.top, self.span)) {
                    return Ok(Walk::Unknown);
                }
                if let Some(part) = piece.part(c, self.top) {
                    parts
                        .try_reserve(1)
                        .map_err(|_| Error::limit("cell walk allocation failed"))?;
                    parts.push((part, piece.chain, piece.place));
                }
            }
            node = next;
        }
        meter.charge(parts.len().saturating_mul(parts.len()).saturating_add(8))?;
        for (i, &(p, chain, place)) in parts.iter().enumerate() {
            for &(q, other, other_place) in &parts[i + 1..] {
                if chain != other && meet(p, q, place.adjacent(other_place)) {
                    return Ok(Walk::Unknown);
                }
            }
        }
        let mut local = std::mem::take(&mut self.local);
        local.clear();
        let result = self
            .local_edges(cell, 0, &mut local)
            .map(|()| perimeter(&local, self.span));
        self.local = local;
        let Some(mut windings) = result? else {
            return Ok(Walk::Unknown);
        };
        if self.span != (0.0, 1.0) {
            // Outside the clip rectangle's span the cell is empty.
            windings.add(0);
        }
        Ok(match rule {
            _ if windings.hi - windings.lo <= 1 => Walk::Fold,
            FillRule::NonZero if windings.lo > 0 || windings.hi < 0 => Walk::Fold,
            FillRule::EvenOdd if windings.parities == 1 => Walk::Value(0),
            FillRule::EvenOdd if windings.parities == 2 => Walk::Value(u8::MAX),
            _ => Walk::Unknown,
        })
    }

    /// The exact coverage of `cell` alone.
    pub(crate) fn resolve(&mut self, cell: &Cell, rule: FillRule, meter: &mut Meter) -> Result<u8> {
        let mut local = std::mem::take(&mut self.local);
        local.clear();
        let span = self.span;
        let result = self
            .local_edges(cell, 0, &mut local)
            .and_then(|()| self.resolver.coverage(&local, &[rule], span, meter));
        self.local = local;
        result
    }

    /// The exact coverage of `cell` (or, without one, of a cell no piece
    /// crosses into which `cover` is carried) within a clip cell whose own
    /// edges and group rules are given.
    pub(crate) fn resolve_joint(
        &mut self,
        (cell, cover): (Option<&Cell>, f64),
        rule: FillRule,
        clip: (&[LocalEdge], &[FillRule]),
        meter: &mut Meter,
    ) -> Result<u8> {
        let (edges, rules) = clip;
        let group = u8::try_from(rules.len()).map_err(|_| Error::limit("clip group count exceeds u8"))?;
        let mut local = std::mem::take(&mut self.local);
        local.clear();
        let mut all = Vec::new();
        let result = (|| {
            local
                .try_reserve(edges.len())
                .map_err(|_| Error::limit("cell edge allocation failed"))?;
            local.extend_from_slice(edges);
            match cell {
                Some(cell) => self.local_edges(cell, group, &mut local)?,
                None => self.gap_edges(cover, group, &mut local)?,
            }
            all.try_reserve(rules.len() + 1)
                .map_err(|_| Error::limit("cell edge allocation failed"))?;
            all.extend_from_slice(rules);
            all.push(rule);
            self.resolver.coverage(&local, &all, self.span, meter)
        })();
        self.local = local;
        result
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.row
            .retained_bytes()
            .saturating_add((self.head.capacity() + self.tag.capacity() + self.listed.capacity()) * 4)
            .saturating_add(self.nodes.capacity() * 8)
            .saturating_add(self.pieces.capacity() * size_of::<RowPiece>())
            .saturating_add(self.cells.capacity() * size_of::<Cell>())
            .saturating_add(self.local.capacity() * size_of::<LocalEdge>())
            .saturating_add(self.resolver.retained_bytes())
    }
}

/// The edges of each row of `rows`, by index, as `(offsets, items)`: row
/// `rows.0 + i` lists `items[offsets[i]..offsets[i + 1]]`.
pub(crate) struct RowIndex {
    first: u32,
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl RowIndex {
    pub(crate) fn new(edges: &[Chained], (first, last): (u32, u32), meter: &mut Meter) -> Result<Self> {
        let count = (last - first) as usize + 1;
        let region = super::DeviceRect {
            x0: 0,
            y0: first,
            x1: 1,
            y1: last + 1,
        };
        let range = |e: &Chained| {
            let (lo, hi) = (e.line.a.y.min(e.line.b.y), e.line.a.y.max(e.line.b.y));
            rows(lo, hi, region)
        };
        let mut index = Self {
            first,
            offsets: Vec::new(),
            items: Vec::new(),
        };
        meter.reserve(&mut index.offsets, count + 1)?;
        index.offsets.resize(count + 1, 0);
        let mut total = 0_usize;
        for e in edges {
            if let Some((a, b)) = range(e) {
                for row in a..=b {
                    index.offsets[(row - first) as usize + 1] += 1;
                }
                total = total.saturating_add((b - a) as usize + 1);
            }
        }
        if u32::try_from(total).is_err() || u32::try_from(edges.len()).is_err() {
            return Err(Error::limit("row index exceeds u32"));
        }
        for i in 0..count {
            index.offsets[i + 1] += index.offsets[i];
        }
        meter.reserve(&mut index.items, total)?;
        index.items.resize(total, 0);
        let mut next = index.offsets.clone();
        for (i, e) in edges.iter().enumerate() {
            if let Some((a, b)) = range(e) {
                for row in a..=b {
                    let slot = &mut next[(row - first) as usize];
                    index.items[*slot as usize] = i as u32;
                    *slot += 1;
                }
            }
        }
        Ok(index)
    }

    pub(crate) fn row(&self, y: u32) -> &[u32] {
        let Some(i) = y.checked_sub(self.first) else {
            return &[];
        };
        let i = i as usize;
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.items[a as usize..b as usize],
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chained(contours: &[&[(f64, f64)]]) -> Vec<Chained> {
        let mut polygon = super::super::edges::Polygon::default();
        for contour in contours {
            polygon
                .begin(&pagura_core::Limits::default(), contour.len())
                .unwrap();
            polygon
                .points
                .extend(contour.iter().map(|&(x, y)| Point::new(x, y)));
            polygon.end();
        }
        let mut out = Vec::new();
        super::super::edges::chained(&polygon, 0, &mut 0, &mut out).unwrap();
        out
    }

    #[test]
    fn corner_windings_rebuild_cells_of_a_rectangle_inside_a_row() {
        // [3, 5] × [0.25, 0.75] counterclockwise in device space, and a
        // second rectangle overlapping it in [4.5, 6] × [0.5, 1].
        let edges = chained(&[
            &[(3.0, 0.75), (3.0, 0.25), (5.0, 0.25), (5.0, 0.75)],
            &[(4.5, 1.0), (4.5, 0.5), (6.0, 0.5), (6.0, 1.0)],
        ]);
        let mut meter = Meter::new(usize::MAX, "budget");
        let mut scanner = Scanner::new((0, 8), &mut meter).unwrap();
        let items: Vec<u32> = (0..edges.len() as u32).collect();
        scanner.load(&edges, (&items, 0, (0.0, 1.0)), (0, 8), &mut meter).unwrap();
        let cells = scanner.cells.clone();
        let mut values = Vec::new();
        for cell in &cells {
            let value = if cell.certain {
                cell.value(FillRule::NonZero)
            } else {
                scanner.resolve(cell, FillRule::NonZero, &mut meter).unwrap()
            };
            values.push((cell.x, value));
        }
        scanner.unload();
        // Cell 3: half covered; cell 4: [4, 4.5] half and [4.5, 5] from
        // 0.25 to 1 (0.75): 0.25 + 0.375 = 0.625; cell 5: [5, 6] × [0.5, 1].
        assert_eq!(values[..3], [(3, 128), (4, 159), (5, 128)]);
        assert!(!cells[1].certain);
    }
}
