//! Independent quadrature of an outline's true coverage, for tests only.
//!
//! The curve sweep reduces admitted cubics to midpoint quadratics and
//! integrates them in closed form. This oracle shares none of that: it keeps
//! the source segments as stored and integrates, row by row, the covered
//! length of each column at the heights of an adaptive Gauss-Legendre rule
//! (ADR 0004, direct glyph curves). Panels break wherever that length loses
//! smoothness: at the ends of monotone pieces and where a piece crosses a
//! column boundary. A smoothstep substitution absorbs the square-root
//! behaviour of x(y) at horizontal tangents, which only occur at piece ends.

use crate::{FillRule, Path, Point, Segment, Transform};
use std::f64::consts::PI;

// The halves of a panel must agree this closely, in square pixels per column.
const TOLERANCE: f64 = 1e-14;

/// Covered areas of the integer pixel window an outline touches.
#[derive(Debug, Default)]
pub(super) struct Areas {
    pub(super) left: i64,
    pub(super) top: i64,
    pub(super) width: usize,
    pub(super) height: usize,
    values: Vec<f64>,
}

impl Areas {
    /// The covered area of pixel (x, y), zero outside the window.
    pub(super) fn at(&self, x: i64, y: i64) -> f64 {
        let column = usize::try_from(x - self.left)
            .ok()
            .filter(|&x| x < self.width);
        let row = usize::try_from(y - self.top)
            .ok()
            .filter(|&y| y < self.height);
        column
            .zip(row)
            .map_or(0.0, |(x, y)| self.values[y * self.width + x])
    }

    /// The covered area of the whole window.
    pub(super) fn total(&self) -> f64 {
        self.values.iter().sum()
    }
}

/// The coverage of `outline` under `transform` and `rule`, pixel by pixel.
pub(super) fn areas(outline: &Path, transform: Transform, rule: FillRule) -> Areas {
    integrate(&pieces(outline, transform), rule, 1)
}

/// Original-curve coverage intersected with a straight or curved clip.
pub(super) fn clipped_areas(outline: &Path, transform: Transform, clip: &Path) -> Areas {
    let mut boundaries = pieces(outline, transform);
    let mut clipping = pieces(clip, Transform::IDENTITY);
    for piece in &mut clipping {
        piece.region = 1;
    }
    boundaries.extend(clipping);
    integrate(&boundaries, FillRule::NonZero, 2)
}

fn integrate(pieces: &[Piece], rule: FillRule, groups: usize) -> Areas {
    if pieces.is_empty() {
        return Areas::default();
    }
    // Both coordinates are monotone on a piece, so its ends bound it.
    let ends = || {
        pieces
            .iter()
            .flat_map(|piece| piece.t.map(|t| piece.curve.x.at(t)))
    };
    let left = pixel(ends().fold(f64::INFINITY, f64::min).floor());
    let right = pixel(ends().fold(f64::NEG_INFINITY, f64::max).ceil());
    let top = pixel(
        pieces
            .iter()
            .map(|piece| piece.y[0])
            .fold(f64::INFINITY, f64::min)
            .floor(),
    );
    let bottom = pixel(
        pieces
            .iter()
            .map(|piece| piece.y[1])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil(),
    );
    let width = usize::try_from(right - left).unwrap().max(1);
    let height = usize::try_from(bottom - top).unwrap();
    let mut values = vec![0.0; width * height];
    let mut quadrature = Quadrature {
        nodes: nodes(),
        rule,
        groups,
        left: left as f64,
        width,
        active: Vec::new(),
        crossings: Vec::new(),
    };
    let mut breaks = Vec::new();
    for (row, sums) in (top..).zip(values.chunks_mut(width)) {
        let (lo, hi) = (row as f64, row as f64 + 1.0);
        breaks.clear();
        breaks.extend([lo, hi]);
        for piece in pieces {
            let (a, b) = (piece.y[0].max(lo), piece.y[1].min(hi));
            if a >= b {
                continue;
            }
            breaks.extend([a, b]);
            let (from, to) = (piece.x(a), piece.x(b));
            let mut column = from.min(to).floor() + 1.0;
            while column < from.max(to) {
                breaks.push(piece.height(column).clamp(a, b));
                column += 1.0;
            }
        }
        breaks.sort_by(f64::total_cmp);
        breaks.dedup();
        for pair in breaks.windows(2) {
            let middle = f64::midpoint(pair[0], pair[1]);
            quadrature.active.clear();
            quadrature.active.extend(
                pieces
                    .iter()
                    .filter(|piece| piece.y[0] < middle && middle < piece.y[1])
                    .copied(),
            );
            if !quadrature.active.is_empty() {
                let whole = quadrature.panel(pair[0], pair[1]);
                quadrature.refine(pair[0], pair[1], &whole, 0, sums);
            }
        }
    }
    Areas {
        left,
        top,
        width,
        height,
        values,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "test outlines lie far inside the i64 range"
)]
fn pixel(value: f64) -> i64 {
    value as i64
}

// The outline's segments under `transform`, split into monotone pieces. Each
// subpath closes with a line back to its start, as filling requires.
fn pieces(outline: &Path, transform: Transform) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let (mut start, mut current) = (Point::new(0.0, 0.0), Point::new(0.0, 0.0));
    for segment in outline.segments() {
        let curve = match *segment {
            Segment::MoveTo(p) => {
                let closing = Curve::line(current, start);
                start = transform.apply(p);
                current = start;
                closing
            }
            Segment::LineTo(p) => {
                let p = transform.apply(p);
                let line = Curve::line(current, p);
                current = p;
                line
            }
            Segment::CubicTo {
                c1: a,
                c2: b,
                end: c,
            } => {
                let [a, b, c] = [a, b, c].map(|p| transform.apply(p));
                let cubic = Curve::cubic([current, a, b, c]);
                current = c;
                cubic
            }
            Segment::Close => {
                let closing = Curve::line(current, start);
                current = start;
                closing
            }
        };
        Piece::split(curve, &mut pieces);
    }
    Piece::split(Curve::line(current, start), &mut pieces);
    pieces
}

/// One coordinate of a curve in power form, c0 + c1 t + c2 t² + c3 t³.
#[derive(Debug, Clone, Copy)]
struct Polynomial([f64; 4]);

impl Polynomial {
    fn line(a: f64, b: f64) -> Self {
        Self([a, b - a, 0.0, 0.0])
    }

    fn bezier([p0, p1, p2, p3]: [f64; 4]) -> Self {
        Self([
            p0,
            3.0 * (p1 - p0),
            3.0 * (p2 - 2.0 * p1 + p0),
            p3 - p0 + 3.0 * (p1 - p2),
        ])
    }

    fn at(self, t: f64) -> f64 {
        let [a, b, c, d] = self.0;
        a + t * (b + t * (c + t * d))
    }

    fn slope(self, t: f64) -> f64 {
        let [_, b, c, d] = self.0;
        b + t * (2.0 * c + t * 3.0 * d)
    }

    // Parameters strictly between the ends at which the slope vanishes.
    fn turns(self) -> impl Iterator<Item = f64> {
        let [_, c, b, a] = self.0;
        let (a, b) = (3.0 * a, 2.0 * b);
        let roots = if a == 0.0 {
            [(b != 0.0).then_some(-c / b), None]
        } else {
            // The pair of roots of a t² + b t + c without cancellation.
            let discriminant = b * b - 4.0 * a * c;
            let q = -0.5 * (b + discriminant.sqrt().copysign(b));
            if discriminant < 0.0 || q == 0.0 {
                [None, None]
            } else {
                [Some(q / a), Some(c / q)]
            }
        };
        roots.into_iter().flatten().filter(|&t| t > 0.0 && t < 1.0)
    }
}

/// A source segment under the placement transform.
#[derive(Debug, Clone, Copy)]
struct Curve {
    x: Polynomial,
    y: Polynomial,
}

impl Curve {
    fn line(a: Point, b: Point) -> Self {
        Self {
            x: Polynomial::line(a.x, b.x),
            y: Polynomial::line(a.y, b.y),
        }
    }

    fn cubic(points: [Point; 4]) -> Self {
        Self {
            x: Polynomial::bezier(points.map(|p| p.x)),
            y: Polynomial::bezier(points.map(|p| p.y)),
        }
    }
}

/// A part of a curve on which both coordinates are monotone: `t` holds the
/// parameters of its lower and upper ends, `y` their heights.
#[derive(Debug, Clone, Copy)]
struct Piece {
    curve: Curve,
    t: [f64; 2],
    y: [f64; 2],
    winding: i64,
    region: usize,
}

impl Piece {
    fn split(curve: Curve, pieces: &mut Vec<Self>) {
        let mut cuts: Vec<f64> = [0.0, 1.0]
            .into_iter()
            .chain(curve.x.turns())
            .chain(curve.y.turns())
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (a, b) = (curve.y.at(pair[0]), curve.y.at(pair[1]));
            // Horizontal pieces bound no area.
            let (t, y, winding) = if a < b {
                ([pair[0], pair[1]], [a, b], 1)
            } else if a > b {
                ([pair[1], pair[0]], [b, a], -1)
            } else {
                continue;
            };
            pieces.push(Self {
                curve,
                t,
                y,
                winding,
                region: 0,
            });
        }
    }

    // Newton steps for y(t) = y, bisecting the root's bracket whenever a
    // step would leave it.
    fn parameter(&self, y: f64) -> f64 {
        let [mut below, mut above] = self.t;
        let fraction = (y - self.y[0]) / (self.y[1] - self.y[0]);
        let mut t = below + (above - below) * fraction.clamp(0.0, 1.0);
        for _ in 0..100 {
            let error = self.curve.y.at(t) - y;
            if error < 0.0 {
                below = t;
            } else if error > 0.0 {
                above = t;
            } else {
                break;
            }
            let (lo, hi) = (below.min(above), below.max(above));
            let step = t - error / self.curve.y.slope(t);
            let next = if step > lo && step < hi {
                step
            } else {
                f64::midpoint(lo, hi)
            };
            if (next - t).abs() <= 1e-16 {
                return next;
            }
            t = next;
        }
        t
    }

    fn x(&self, y: f64) -> f64 {
        self.curve.x.at(self.parameter(y))
    }

    // Where the piece crosses the vertical line at `x`, by bisection.
    fn height(&self, x: f64) -> f64 {
        let [mut left, mut right] = self.t;
        if self.curve.x.at(left) > self.curve.x.at(right) {
            std::mem::swap(&mut left, &mut right);
        }
        for _ in 0..64 {
            let middle = f64::midpoint(left, right);
            if self.curve.x.at(middle) < x {
                left = middle;
            } else {
                right = middle;
            }
        }
        self.curve.y.at(f64::midpoint(left, right))
    }
}

// Sixteen-point Gauss-Legendre on [0, 1] after the substitution
// u -> 3u² - 2u³: each node's fraction of the panel height and its weight.
fn nodes() -> [(f64, f64); 16] {
    let mut nodes = [(0.0, 0.0); 16];
    let n = 16.0;
    for (node, i) in nodes.iter_mut().zip(0_u32..) {
        // Newton's method on the Legendre polynomial from the usual estimate.
        let mut x = (PI * (f64::from(i) + 0.75) / (n + 0.5)).cos();
        let mut slope = 1.0;
        for _ in 0..100 {
            let (mut previous, mut value) = (1.0, x);
            for k in 2..=16 {
                let k = f64::from(k);
                (previous, value) = (
                    value,
                    ((2.0 * k - 1.0) * x * value - (k - 1.0) * previous) / k,
                );
            }
            slope = n * (x * value - previous) / (x * x - 1.0);
            let step = value / slope;
            x -= step;
            if step.abs() <= 1e-16 {
                break;
            }
        }
        let u = f64::midpoint(x, 1.0);
        let weight = 1.0 / ((1.0 - x * x) * slope * slope);
        *node = (u * u * (3.0 - 2.0 * u), weight * 6.0 * u * (1.0 - u));
    }
    nodes
}

// The covered length of each column, integrated over the panels of one row.
struct Quadrature {
    nodes: [(f64, f64); 16],
    rule: FillRule,
    groups: usize,
    left: f64,
    width: usize,
    // The pieces spanning the current panel.
    active: Vec<Piece>,
    crossings: Vec<Crossing>,
}

#[derive(Clone, Copy)]
struct Crossing {
    x: f64,
    winding: i64,
    region: usize,
}

impl Quadrature {
    fn panel(&mut self, lo: f64, hi: f64) -> Vec<f64> {
        let mut sums = vec![0.0; self.width];
        for (fraction, weight) in self.nodes {
            let y = lo + (hi - lo) * fraction;
            let weight = (hi - lo) * weight;
            self.crossings.clear();
            self.crossings
                .extend(self.active.iter().map(|piece| Crossing {
                    x: piece.x(y) - self.left,
                    winding: piece.winding,
                    region: piece.region,
                }));
            self.crossings.sort_by(|a, b| a.x.total_cmp(&b.x));
            let mut winding = [0_i64; 2];
            for pair in self.crossings.windows(2) {
                winding[pair[0].region] += pair[0].winding;
                if !winding[..self.groups].iter().all(|&w| inside(w, self.rule)) {
                    continue;
                }
                let (from, to) = (pair[0].x, pair[1].x);
                let start = index(from.floor(), self.width);
                let end = index(to.ceil(), self.width);
                for (sum, column) in sums[start..end].iter_mut().zip(start..) {
                    let x = column as f64;
                    *sum += weight * (to.min(x + 1.0) - from.max(x));
                }
            }
        }
        sums
    }

    fn refine(&mut self, lo: f64, hi: f64, whole: &[f64], depth: u32, sums: &mut [f64]) {
        let middle = f64::midpoint(lo, hi);
        let (lower, upper) = (self.panel(lo, middle), self.panel(middle, hi));
        let settled = depth == 40
            || whole
                .iter()
                .zip(lower.iter().zip(&upper))
                .all(|(whole, (a, b))| (whole - (a + b)).abs() <= TOLERANCE);
        if settled {
            for (sum, (a, b)) in sums.iter_mut().zip(lower.iter().zip(&upper)) {
                *sum += a + b;
            }
        } else {
            self.refine(lo, middle, &lower, depth + 1, sums);
            self.refine(middle, hi, &upper, depth + 1, sums);
        }
    }
}

// Pixel indexing and winding are evaluated locally so the quadrature shares
// neither geometry reduction nor coverage arithmetic with the renderer.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "small finite test windows"
)]
fn index(value: f64, length: usize) -> usize {
    value.max(0.0).min(length as f64) as usize
}

fn inside(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}
