//! The previous shading sampling (`pagura-render` `compositor.rs`
//! `shading_sample`, `shading.rs` `Shading::sample`, `shading/mesh.rs` with
//! its bounds hierarchy and `shading/mesh/patches.rs`), ported as a
//! test-only oracle. Function evaluation and colour conversion use the new
//! `Function` and `Converter`, which `pagura-color` tests against the
//! previous code bit for bit.
#![expect(clippy::cast_precision_loss, reason = "ported arithmetic")]

use super::parse::{OldPrimitive, Parsed};
use pagura_color::Converter;
use pagura_interpret::{ShadingFunction, ShadingKind};
use pagura_raster::{Point, Transform};

/// A pixel: its colour channels and covered fraction, `None` when no
/// sample hit, or `Err` where the previous code failed.
pub(super) type Sample = Result<Option<(Vec<f64>, f64)>, &'static str>;

pub(super) struct Oracle<'a> {
    pub(super) parsed: &'a Parsed,
    pub(super) converter: &'a Converter,
    pub(super) inverse: Transform,
    pub(super) rate: usize,
    /// A shading pattern's paint (`background` set in the previous command).
    pub(super) pattern: bool,
    pub(super) certified: bool,
    pub(super) index: Index,
}

impl Oracle<'_> {
    fn convert(&self, values: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.converter.target().channels()];
        self.converter
            .convert(&values[..self.parsed.space.components()], &mut out);
        out
    }

    /// `shading_sample`.
    pub(super) fn pixel(&self, coordinate: (u32, u32), boundary: Option<f64>) -> Sample {
        let background = boundary
            .and(self.parsed.background.as_ref())
            .map(|values| self.convert(values));
        let mut values = [0.0; 32];
        let channels = self.converter.target().channels();
        let mut color = vec![0.0; channels];
        let mut covered = 0;
        let divisor = self.rate as f64;
        for y in 0..self.rate {
            for x in 0..self.rate {
                let point = Point::new(
                    f64::from(coordinate.0) + (x as f64 + 0.5) / divisor,
                    f64::from(coordinate.1) + (y as f64 + 0.5) / divisor,
                ) * self.inverse;
                let inside = self.certified
                    || self.parsed.bbox.is_none_or(|b| {
                        point.x >= b.x0()
                            && point.x <= b.x1()
                            && point.y >= b.y0()
                            && point.y <= b.y1()
                    });
                let hit = (!self.pattern || inside) && self.sample(point, &mut values)?;
                if !hit {
                    if self.pattern {
                        if let Some(background) = &self.parsed.background {
                            values = [0.0; 32];
                            values[..background.len()].copy_from_slice(background);
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                let sample = self.convert(&values);
                for (i, out) in color.iter_mut().enumerate() {
                    *out += if let (Some(coverage), Some(background)) = (boundary, &background) {
                        coverage * sample[i] + (1.0 - coverage) * background[i]
                    } else {
                        sample[i]
                    };
                }
                covered += 1;
            }
        }
        if covered == 0 {
            return Ok(None);
        }
        let shape = f64::from(covered) / (divisor * divisor);
        Ok(Some((
            color.iter().map(|v| v / f64::from(covered)).collect(),
            shape,
        )))
    }

    /// `Shading::sample`.
    fn sample(&self, point: Point, output: &mut [f64; 32]) -> Result<bool, &'static str> {
        let n = self.parsed.space.components();
        let (input, count) = match &self.parsed.kind {
            ShadingKind::Triangles(_) | ShadingKind::Patches(_) => {
                let Some(values) =
                    self.index
                        .sample(point, &self.parsed.old_mesh, self.mesh_inputs())?
                else {
                    return Ok(false);
                };
                let count = self.mesh_inputs();
                (values, count)
            }
            ShadingKind::Function { domain, matrix } => {
                let p = point * matrix.inverse().ok_or("singular")?;
                if p.x < domain[0] || p.x > domain[1] || p.y < domain[2] || p.y > domain[3] {
                    return Ok(false);
                }
                let mut input = [0.0; 32];
                input[0] = p.x;
                input[1] = p.y;
                (input, 2)
            }
            ShadingKind::Axial {
                start,
                end,
                domain,
                extend,
            } => {
                let delta = Point::new(end.x - start.x, end.y - start.y);
                let denominator = delta.x * delta.x + delta.y * delta.y;
                if denominator == 0.0 {
                    return Ok(false);
                }
                let t =
                    ((point.x - start.x) * delta.x + (point.y - start.y) * delta.y) / denominator;
                let Some(t) = parameter(t, *extend) else {
                    return Ok(false);
                };
                let mut input = [0.0; 32];
                input[0] = domain[0] + t * (domain[1] - domain[0]);
                (input, 1)
            }
            ShadingKind::Radial {
                start,
                start_radius,
                end,
                end_radius,
                domain,
                extend,
            } => {
                let delta = Point::new(end.x - start.x, end.y - start.y);
                let radius = *start_radius;
                let change = end_radius - start_radius;
                if radius == 0.0 && change == 0.0 {
                    return Ok(false);
                }
                let x = point.x - start.x;
                let y = point.y - start.y;
                let a = delta.x * delta.x + delta.y * delta.y - change * change;
                let b = -2.0 * (x * delta.x + y * delta.y + radius * change);
                let c = x * x + y * y - radius * radius;
                let t = roots(a, b, c)
                    .into_iter()
                    .filter(|t| t.is_finite() && radius + change * t >= 0.0)
                    .filter_map(|t| parameter(t, *extend))
                    .max_by(f64::total_cmp);
                let Some(t) = t else { return Ok(false) };
                let mut input = [0.0; 32];
                input[0] = domain[0] + t * (domain[1] - domain[0]);
                (input, 1)
            }
            _ => return Err("unknown"),
        };
        match &self.parsed.function {
            None => output[..n].copy_from_slice(&input[..n]),
            Some(ShadingFunction::Single(f)) => f.evaluate(&input[..count], &mut output[..n]),
            Some(ShadingFunction::PerComponent(fs)) => {
                for (f, out) in fs.iter().zip(&mut output[..n]) {
                    f.evaluate(&input[..count], std::slice::from_mut(out));
                }
            }
        }
        Ok(true)
    }

    fn mesh_inputs(&self) -> usize {
        if self.parsed.function.is_some() {
            1
        } else {
            self.parsed.space.components()
        }
    }
}

fn parameter(t: f64, extend: [bool; 2]) -> Option<f64> {
    (t.is_finite() && (t >= 0.0 || extend[0]) && (t <= 1.0 || extend[1])).then(|| t.clamp(0.0, 1.0))
}

fn roots(a: f64, b: f64, c: f64) -> [f64; 2] {
    if a == 0.0 {
        return if b == 0.0 { [f64::NAN; 2] } else { [-c / b; 2] };
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return [f64::NAN; 2];
    }
    let q = -0.5 * (b + discriminant.sqrt().copysign(b));
    if q == 0.0 {
        [-b / (2.0 * a); 2]
    } else {
        [q / a, c / q]
    }
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo: Point,
    hi: Point,
}

impl Bounds {
    fn points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut b = Self {
            lo: Point::new(f64::INFINITY, f64::INFINITY),
            hi: Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
        for p in points {
            b.lo.x = b.lo.x.min(p.x);
            b.lo.y = b.lo.y.min(p.y);
            b.hi.x = b.hi.x.max(p.x);
            b.hi.y = b.hi.y.max(p.y);
        }
        b
    }

    fn contains(self, p: Point) -> bool {
        p.x >= self.lo.x && p.x <= self.hi.x && p.y >= self.lo.y && p.y <= self.hi.y
    }

    fn union(self, b: Self) -> Self {
        Self::points([self.lo, self.hi, b.lo, b.hi])
    }
}

fn bounds(p: &OldPrimitive) -> Bounds {
    match p {
        OldPrimitive::Triangle(v) => Bounds::points(v.iter().map(|v| v.0)),
        OldPrimitive::Patch { points, .. } => Bounds::points(*points),
    }
}

enum Kind {
    Leaf(usize, usize),
    Branch(usize, usize),
}

struct Node {
    bounds: Bounds,
    kind: Kind,
}

/// The previous median-split bounds hierarchy.
pub(super) struct Index {
    nodes: Vec<Node>,
    order: Vec<usize>,
}

impl Index {
    pub(super) fn new(primitives: &[OldPrimitive]) -> Self {
        let mut index = Self {
            nodes: Vec::new(),
            order: (0..primitives.len()).collect(),
        };
        if !primitives.is_empty() {
            index.build(primitives, 0, primitives.len());
        }
        index
    }

    fn build(&mut self, p: &[OldPrimitive], start: usize, end: usize) -> usize {
        let b = self.order[start..end]
            .iter()
            .map(|i| bounds(&p[*i]))
            .reduce(Bounds::union)
            .unwrap_or_else(|| Bounds::points([]));
        let id = self.nodes.len();
        self.nodes.push(Node {
            bounds: b,
            kind: Kind::Leaf(start, end),
        });
        if end - start > 8 {
            let x = (b.hi.x - b.lo.x) >= (b.hi.y - b.lo.y);
            let center = |i: usize| {
                let b = bounds(&p[i]);
                if x { b.lo.x + b.hi.x } else { b.lo.y + b.hi.y }
            };
            let middle = (end - start) / 2;
            self.order[start..end]
                .select_nth_unstable_by(middle, |a, b| center(*a).total_cmp(&center(*b)));
            let a = self.build(p, start, start + middle);
            let b = self.build(p, start + middle, end);
            self.nodes[id].kind = Kind::Branch(a, b);
        }
        id
    }

    fn sample(
        &self,
        point: Point,
        primitives: &[OldPrimitive],
        n: usize,
    ) -> Result<Option<[f64; 32]>, &'static str> {
        let mut hit: Option<(usize, [f64; 32])> = None;
        if !self.nodes.is_empty() {
            self.walk(0, point, primitives, n, &mut hit)?;
        }
        Ok(hit.map(|h| h.1))
    }

    fn walk(
        &self,
        id: usize,
        p: Point,
        primitives: &[OldPrimitive],
        n: usize,
        hit: &mut Option<(usize, [f64; 32])>,
    ) -> Result<(), &'static str> {
        let node = &self.nodes[id];
        if !node.bounds.contains(p) {
            return Ok(());
        }
        match node.kind {
            Kind::Leaf(a, b) => {
                for &i in &self.order[a..b] {
                    if hit.is_some_and(|(previous, _)| previous > i) {
                        continue;
                    }
                    let value = match &primitives[i] {
                        OldPrimitive::Triangle(v) => triangle(v, p, n),
                        OldPrimitive::Patch { points, colors } => {
                            patch_sample(points, colors, p, n)?
                        }
                    };
                    if let Some(value) = value {
                        *hit = Some((i, value));
                    }
                }
            }
            Kind::Branch(a, b) => {
                self.walk(a, p, primitives, n, hit)?;
                self.walk(b, p, primitives, n, hit)?;
            }
        }
        Ok(())
    }
}

fn triangle(v: &[(Point, [f64; 32]); 3], p: Point, n: usize) -> Option<[f64; 32]> {
    let cross =
        |a: Point, b: Point, c: Point| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let area = cross(v[0].0, v[1].0, v[2].0);
    if area == 0.0 {
        return None;
    }
    let w1 = cross(v[0].0, p, v[2].0) / area;
    let w2 = cross(v[0].0, v[1].0, p) / area;
    let w0 = 1.0 - w1 - w2;
    if w0.min(w1).min(w2) < -1e-12 {
        return None;
    }
    let mut out = [0.0; 32];
    for (i, out) in out.iter_mut().enumerate().take(n) {
        *out = w0 * v[0].1[i] + w1 * v[1].1[i] + w2 * v[2].1[i];
    }
    Some(out)
}

fn patch_sample(
    points: &[Point; 16],
    colors: &[[f64; 32]; 4],
    p: Point,
    n: usize,
) -> Result<Option<[f64; 32]>, &'static str> {
    if !Bounds::points(*points).contains(p) {
        return Ok(None);
    }
    let mut best = if injective(points) {
        inverse(points, p)
    } else {
        None
    };
    if best.is_none() {
        search(*points, [0.0, 0.0, 1.0, 1.0], p, &mut best, 0)?;
    }
    let Some((u, v)) = best else { return Ok(None) };
    let mut out = [0.0; 32];
    for (i, out) in out.iter_mut().enumerate().take(n) {
        *out = (1.0 - u) * (1.0 - v) * colors[0][i]
            + (1.0 - u) * v * colors[1][i]
            + u * v * colors[2][i]
            + u * (1.0 - v) * colors[3][i];
    }
    Ok(Some(out))
}

fn search(
    points: [Point; 16],
    r: [f64; 4],
    p: Point,
    best: &mut Option<(f64, f64)>,
    depth: usize,
) -> Result<(), &'static str> {
    if best.is_some_and(|(u, v)| v > r[3] || (v == r[3] && u >= r[2]))
        || !Bounds::points(points).contains(p)
    {
        return Ok(());
    }
    if (injective(&points) || depth >= 24)
        && let Some((u, v)) = inverse(&points, p)
    {
        let candidate = (r[0] + u * (r[2] - r[0]), r[1] + v * (r[3] - r[1]));
        if best.is_none_or(|(u, v)| {
            candidate.1 > v + 1e-9 || ((candidate.1 - v).abs() <= 1e-9 && candidate.0 > u)
        }) {
            *best = Some(candidate);
        }
        return Ok(());
    }
    if points.iter().all(|q| q.x == points[0].x) || points.iter().all(|q| q.y == points[0].y) {
        return Ok(());
    }
    if depth >= 24 {
        return Err("patch inverse cannot be resolved at numeric precision");
    }
    let (left, right) = split_u(points);
    let (ll, lh) = split_v(left);
    let (rl, rh) = split_v(right);
    let um = r[0].midpoint(r[2]);
    let vm = r[1].midpoint(r[3]);
    for (points, r) in [
        (rh, [um, vm, r[2], r[3]]),
        (lh, [r[0], vm, um, r[3]]),
        (rl, [um, r[1], r[2], vm]),
        (ll, [r[0], r[1], um, vm]),
    ] {
        search(points, r, p, best, depth + 1)?;
    }
    Ok(())
}

fn basis(t: f64) -> ([f64; 4], [f64; 4]) {
    let s = 1.0 - t;
    (
        [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t],
        [
            -3.0 * s * s,
            3.0 * s * s - 6.0 * t * s,
            6.0 * t * s - 3.0 * t * t,
            3.0 * t * t,
        ],
    )
}

fn evaluate(points: &[Point; 16], u: f64, v: f64) -> (Point, Point, Point) {
    let (bu, du) = basis(u);
    let (bv, dv) = basis(v);
    let (mut p, mut a, mut b) = (Point::default(), Point::default(), Point::default());
    for j in 0..4 {
        for i in 0..4 {
            let q = points[j * 4 + i];
            p.x += q.x * bu[i] * bv[j];
            p.y += q.y * bu[i] * bv[j];
            a.x += q.x * du[i] * bv[j];
            a.y += q.y * du[i] * bv[j];
            b.x += q.x * bu[i] * dv[j];
            b.y += q.y * bu[i] * dv[j];
        }
    }
    (p, a, b)
}

fn solve(a: Point, b: Point, p: Point) -> Option<Point> {
    let det = a.x * b.y - a.y * b.x;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let p = Point::new((b.y * p.x - b.x * p.y) / det, (a.x * p.y - a.y * p.x) / det);
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}

fn injective(points: &[Point; 16]) -> bool {
    let (_, a, b) = evaluate(points, 0.5, 0.5);
    let (mut diag, mut cross) = (f64::INFINITY, 0.0_f64);
    for axis in 0..2 {
        for j in 0..if axis == 0 { 4 } else { 3 } {
            for i in 0..if axis == 0 { 3 } else { 4 } {
                let p = points[j * 4 + i];
                let q = points[j * 4 + i + if axis == 0 { 1 } else { 4 }];
                let Some(d) = solve(a, b, Point::new(3.0 * (q.x - p.x), 3.0 * (q.y - p.y))) else {
                    return false;
                };
                diag = diag.min(if axis == 0 { d.x } else { d.y });
                cross = cross.max(if axis == 0 { d.y.abs() } else { d.x.abs() });
            }
        }
    }
    diag > cross + 1e-12
}

fn inverse(points: &[Point; 16], p: Point) -> Option<(f64, f64)> {
    let absolute = points
        .iter()
        .map(|p| p.x.abs().max(p.y.abs()))
        .fold(0.0, f64::max);
    let origin = points[0];
    let points = points.map(|q| Point::new(q.x - origin.x, q.y - origin.y));
    let p = Point::new(p.x - origin.x, p.y - origin.y);
    let scale = points
        .iter()
        .map(|q| q.x.abs().max(q.y.abs()))
        .fold(0.0, f64::max);
    let (mut u, mut v) = (0.5, 0.5);
    for _ in 0..24 {
        let (q, a, b) = evaluate(&points, u, v);
        let error = Point::new(q.x - p.x, q.y - p.y);
        if error.x.abs().max(error.y.abs()) <= scale * 1e-12 + 8.0 * f64::EPSILON * absolute
            && (-1e-10..=1.0 + 1e-10).contains(&u)
            && (-1e-10..=1.0 + 1e-10).contains(&v)
        {
            return Some((u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)));
        }
        let delta = solve(a, b, error)?;
        u -= delta.x;
        v -= delta.y;
        if !(-2.0..=3.0).contains(&u) || !(-2.0..=3.0).contains(&v) {
            return None;
        }
    }
    None
}

fn mix(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x * (1.0 - t) + b.x * t, a.y * (1.0 - t) + b.y * t)
}

fn split_curve(p: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let a = mix(p[0], p[1], 0.5);
    let b = mix(p[1], p[2], 0.5);
    let c = mix(p[2], p[3], 0.5);
    let d = mix(a, b, 0.5);
    let e = mix(b, c, 0.5);
    let f = mix(d, e, 0.5);
    ([p[0], a, d, f], [f, e, c, p[3]])
}

fn split_u(p: [Point; 16]) -> ([Point; 16], [Point; 16]) {
    let (mut a, mut b) = ([Point::default(); 16], [Point::default(); 16]);
    for j in 0..4 {
        let (x, y) = split_curve([p[4 * j], p[4 * j + 1], p[4 * j + 2], p[4 * j + 3]]);
        a[4 * j..4 * j + 4].copy_from_slice(&x);
        b[4 * j..4 * j + 4].copy_from_slice(&y);
    }
    (a, b)
}

fn split_v(p: [Point; 16]) -> ([Point; 16], [Point; 16]) {
    let transpose = |p: [Point; 16]| std::array::from_fn(|i| p[(i % 4) * 4 + i / 4]);
    let (a, b) = split_u(transpose(p));
    (transpose(a), transpose(b))
}
