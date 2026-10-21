//! Exact covered areas: for every pixel of a window, the area of the
//! intersection of the pixel square with several regions, each under its own
//! fill rule.
//!
//! Each pixel row is cut into horizontal slabs at every height where an edge
//! starts or ends, crosses another edge or crosses a pixel column boundary.
//! Within a slab no two edges cross and no edge leaves its pixel column, so
//! the edges are ordered left to right and every region's winding number is
//! constant between two consecutive edges. Where every region is inside, the
//! part of a pixel column between those two edges is a trapezoid, whose area
//! is exact. Nothing is approximated beyond f64 rounding.

use pagura_raster::{DeviceRect, FillRule, Point};

/// A region: the directed edges of closed device-space contours, filled
/// under a rule.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    edges: Vec<[Point; 2]>,
    rule: FillRule,
}

impl Region {
    /// An empty region (it contains no point).
    pub(crate) fn new(rule: FillRule) -> Self {
        Self {
            edges: Vec::new(),
            rule,
        }
    }

    /// Adds a contour, closed from its last point back to its first.
    pub(crate) fn push_contour(&mut self, points: &[Point]) {
        let Some(&first) = points.first() else {
            return;
        };
        let mut previous = first;
        for &next in points.iter().skip(1).chain(std::iter::once(&first)) {
            if previous.y != next.y {
                self.edges.push([previous, next]);
            }
            previous = next;
        }
    }

    /// The axis-aligned rectangle `[x0, y0, x1, y1]` holding every edge;
    /// `None` when the region has no edge and so contains no point.
    fn bounds(&self) -> Option<[f64; 4]> {
        let mut edges = self.edges.iter().flatten();
        let first = edges.next()?;
        Some(edges.fold([first.x, first.y, first.x, first.y], |b, p| {
            [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]
        }))
    }

    fn inside(&self, winding: i64) -> bool {
        match self.rule {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding % 2 != 0,
        }
    }
}

/// The covered area of each pixel of a rectangle, row by row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Areas {
    rect: DeviceRect,
    values: Vec<f64>,
}

impl Areas {
    /// The pixels that can have a nonzero area.
    pub(crate) fn rect(&self) -> DeviceRect {
        self.rect
    }

    /// The area of pixel `(x, y)`, in `0..=1`; zero outside the rectangle.
    pub(crate) fn get(&self, x: u32, y: u32) -> f64 {
        index(self.rect, x, y)
            .and_then(|i| self.values.get(i))
            .copied()
            .unwrap_or(0.0)
    }
}

/// The index of `(x, y)` in a row-major rectangle.
pub(crate) fn index(rect: DeviceRect, x: u32, y: u32) -> Option<usize> {
    if x < rect.x0 || x >= rect.x1 || y < rect.y0 || y >= rect.y1 {
        return None;
    }
    let row = usize::try_from(y - rect.y0).ok()?;
    let column = usize::try_from(x - rect.x0).ok()?;
    let width = usize::try_from(rect.width()).ok()?;
    row.checked_mul(width)?.checked_add(column)
}

/// The number of pixels of a rectangle.
pub(crate) fn pixels(rect: DeviceRect) -> usize {
    let width = usize::try_from(rect.width()).unwrap_or(usize::MAX);
    let height = usize::try_from(rect.height()).unwrap_or(usize::MAX);
    width.saturating_mul(height)
}

/// The area of `regions`' intersection in every pixel of `window`. With no
/// region, every pixel is fully covered.
pub(crate) fn areas(regions: &[Region], window: DeviceRect) -> Areas {
    let rect = covered_rect(regions, window);
    let mut values = vec![0.0; pixels(rect)];
    if regions.is_empty() {
        values.fill(1.0);
        return Areas { rect, values };
    }
    let width = usize::try_from(rect.width()).unwrap_or(0);
    if width > 0 {
        for (y, row) in (rect.y0..rect.y1).zip(values.chunks_exact_mut(width)) {
            Row::new(regions, rect, y).integrate(row);
        }
    }
    for value in &mut values {
        *value = value.clamp(0.0, 1.0);
    }
    Areas { rect, values }
}

/// The pixels of `window` that every region's edges can reach.
fn covered_rect(regions: &[Region], window: DeviceRect) -> DeviceRect {
    let mut bounds = [
        f64::from(window.x0),
        f64::from(window.y0),
        f64::from(window.x1),
        f64::from(window.y1),
    ];
    for region in regions {
        let Some(b) = region.bounds() else {
            return DeviceRect::default();
        };
        bounds = [
            bounds[0].max(b[0].floor()),
            bounds[1].max(b[1].floor()),
            bounds[2].min(b[2].ceil()),
            bounds[3].min(b[3].ceil()),
        ];
    }
    if !(bounds[0] < bounds[2] && bounds[1] < bounds[3]) {
        return DeviceRect::default();
    }
    DeviceRect {
        x0: pixel(bounds[0]),
        y0: pixel(bounds[1]),
        x1: pixel(bounds[2]),
        y1: pixel(bounds[3]),
    }
}

/// An integral coordinate within the `u32` range of a window.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass integral values clamped to a u32 window"
)]
fn pixel(value: f64) -> u32 {
    value.clamp(0.0, f64::from(u32::MAX)) as u32
}

/// An edge restricted to one pixel row and to the window's columns, as a
/// segment from its upper to its lower end.
#[derive(Debug, Clone, Copy)]
struct Piece {
    top: Point,
    bottom: Point,
    winding: i64,
    region: usize,
}

impl Piece {
    /// The x coordinate at height `y`, exact at the ends.
    fn x(&self, y: f64) -> f64 {
        if y <= self.top.y {
            self.top.x
        } else if y >= self.bottom.y {
            self.bottom.x
        } else {
            self.top.x
                + (y - self.top.y) / (self.bottom.y - self.top.y) * (self.bottom.x - self.top.x)
        }
    }

    /// The height at which the piece reaches `x`.
    fn y(&self, x: f64) -> f64 {
        self.top.y + (x - self.top.x) / (self.bottom.x - self.top.x) * (self.bottom.y - self.top.y)
    }

    fn left(&self) -> f64 {
        self.top.x.min(self.bottom.x)
    }

    fn right(&self) -> f64 {
        self.top.x.max(self.bottom.x)
    }
}

/// One pixel row of the window and the edge pieces that meet it.
struct Row<'a> {
    regions: &'a [Region],
    rect: DeviceRect,
    top: f64,
    bottom: f64,
    pieces: Vec<Piece>,
}

impl<'a> Row<'a> {
    fn new(regions: &'a [Region], rect: DeviceRect, y: u32) -> Self {
        let mut row = Self {
            regions,
            rect,
            top: f64::from(y),
            bottom: f64::from(y) + 1.0,
            pieces: Vec::new(),
        };
        for (region, r) in regions.iter().enumerate() {
            for &[a, b] in &r.edges {
                row.add(a, b, region);
            }
        }
        row
    }

    /// Adds the part of edge `a → b` within the row, with parts beside the
    /// window moved onto its nearer boundary, which keeps every winding
    /// number inside the window.
    fn add(&mut self, a: Point, b: Point, region: usize) {
        let (winding, upper, lower) = if a.y < b.y { (1, a, b) } else { (-1, b, a) };
        if lower.y <= self.top || upper.y >= self.bottom {
            return;
        }
        let whole = Piece {
            top: upper,
            bottom: lower,
            winding,
            region,
        };
        let (y0, y1) = (upper.y.max(self.top), lower.y.min(self.bottom));
        let piece = Piece {
            top: Point::new(whole.x(y0), y0),
            bottom: Point::new(whole.x(y1), y1),
            winding,
            region,
        };
        let (left, right) = (f64::from(self.rect.x0), f64::from(self.rect.x1));
        let mut cuts = vec![piece.top.y, piece.bottom.y];
        for boundary in [left, right] {
            if piece.left() < boundary && boundary < piece.right() {
                cuts.push(piece.y(boundary).clamp(piece.top.y, piece.bottom.y));
            }
        }
        cuts.sort_by(f64::total_cmp);
        for pair in cuts.windows(2) {
            let (y0, y1) = (pair[0], pair[1]);
            if y0 >= y1 {
                continue;
            }
            let (x0, x1) = (piece.x(y0), piece.x(y1));
            let middle = f64::midpoint(x0, x1);
            let (x0, x1) = if middle <= left {
                (left, left)
            } else if middle >= right {
                (right, right)
            } else {
                (x0.clamp(left, right), x1.clamp(left, right))
            };
            self.pieces.push(Piece {
                top: Point::new(x0, y0),
                bottom: Point::new(x1, y1),
                winding,
                region,
            });
        }
    }

    /// The heights at which the row is cut into slabs.
    fn cuts(&self) -> Vec<f64> {
        let mut cuts = vec![self.top, self.bottom];
        for piece in &self.pieces {
            cuts.push(piece.top.y);
            cuts.push(piece.bottom.y);
            let (left, right) = (piece.left(), piece.right());
            let mut column = left.floor() + 1.0;
            while column < right {
                cuts.push(piece.y(column).clamp(piece.top.y, piece.bottom.y));
                column += 1.0;
            }
        }
        let mut order: Vec<usize> = (0..self.pieces.len()).collect();
        order.sort_by(|&a, &b| self.pieces[a].left().total_cmp(&self.pieces[b].left()));
        for (i, &a) in order.iter().enumerate() {
            let first = self.pieces[a];
            for &b in &order[i + 1..] {
                let second = self.pieces[b];
                if second.left() > first.right() {
                    break;
                }
                if let Some(y) = crossing(&first, &second) {
                    cuts.push(y);
                }
            }
        }
        cuts.retain(|y| (self.top..=self.bottom).contains(y));
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts
    }

    /// Adds every slab's trapezoids to `row`, the areas of the row's pixels.
    fn integrate(&self, row: &mut [f64]) {
        let cuts = self.cuts();
        let mut windings = vec![0_i64; self.regions.len()];
        let mut active = Vec::new();
        for pair in cuts.windows(2) {
            let (y0, y1) = (pair[0], pair[1]);
            if y0 >= y1 {
                continue;
            }
            active.clear();
            active.extend(
                self.pieces
                    .iter()
                    .filter(|p| p.top.y <= y0 && p.bottom.y >= y1)
                    .map(|p| Crossing {
                        top: p.x(y0),
                        bottom: p.x(y1),
                        winding: p.winding,
                        region: p.region,
                    }),
            );
            active.sort_by(|a, b| (a.top + a.bottom).total_cmp(&(b.top + b.bottom)));
            windings.fill(0);
            let mut inside = 0;
            for (i, crossing) in active.iter().enumerate() {
                let region = &self.regions[crossing.region];
                let winding = &mut windings[crossing.region];
                let before = region.inside(*winding);
                *winding += crossing.winding;
                let after = region.inside(*winding);
                if !before && after {
                    inside += 1;
                } else if before && !after {
                    inside -= 1;
                }
                if inside == self.regions.len()
                    && let Some(next) = active.get(i + 1)
                {
                    self.trapezoid(crossing, next, y1 - y0, row);
                }
            }
        }
    }

    /// Adds the area between `left` and `right` over a slab of `height` to
    /// each pixel column it meets.
    fn trapezoid(&self, left: &Crossing, right: &Crossing, height: f64, row: &mut [f64]) {
        let origin = f64::from(self.rect.x0);
        let first = left.top.min(left.bottom).floor().max(origin);
        let last = right.top.max(right.bottom).ceil();
        let mut column = first;
        while column < last {
            let width = |l: f64, r: f64| (r.min(column + 1.0) - l.max(column)).max(0.0);
            let area = height
                * f64::midpoint(width(left.top, right.top), width(left.bottom, right.bottom));
            if let Some(value) = row.get_mut(offset(column - origin)) {
                *value += area;
            }
            column += 1.0;
        }
    }
}

/// A piece's crossing of one slab.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    top: f64,
    bottom: f64,
    winding: i64,
    region: usize,
}

/// The height strictly inside both pieces' spans where they cross, if any.
fn crossing(a: &Piece, b: &Piece) -> Option<f64> {
    let (y0, y1) = (a.top.y.max(b.top.y), a.bottom.y.min(b.bottom.y));
    if y0 >= y1 {
        return None;
    }
    let (d0, d1) = (a.x(y0) - b.x(y0), a.x(y1) - b.x(y1));
    if d0 == 0.0 || d1 == 0.0 || (d0 < 0.0) == (d1 < 0.0) {
        return None;
    }
    Some(y0 + d0 / (d0 - d1) * (y1 - y0))
}

/// A non-negative integral column offset as an index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "integral offsets within a u32 window"
)]
fn offset(value: f64) -> usize {
    value.max(0.0) as usize
}

#[cfg(test)]
mod tests;
