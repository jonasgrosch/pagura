//! Coverage-byte certification at the preparation boundary. A small area
//! error alone cannot survive repeated composition: both ends of the true
//! coverage interval must quantize to the byte execution will emit. Exact
//! quadratics integrated in closed form skip inspection only while a cached
//! arithmetic allowance stays small at their actual placement.

use super::{GlyphEdges, Kind, Relative, area_share, flattened_with, translated};
use crate::band::cells::Cells;
use crate::band::clip::Paths;
use crate::band::edges::{self, Bounds, Line};
use crate::band::shape::{Body, Frame, GlyphGeometry, Shape, Winding};
use crate::band::union::{Meters, Piece, Resolver};
use crate::band::{Prep, RASTER_WORK, RENDER_BYTES, curve, raster};
use crate::flatten::{Hull, Inspection};
use crate::{FillRule, Path, Point, Segment, Transform, quantize_unit};
use pagura_core::{Error, Meter, Result};
use std::sync::Arc;

const ROUNDS: i32 = 12;

/// The direct path reserves all but 1/1024 of a coverage level for the
/// ordinary kernel's rounding margin. This is an arithmetic admission
/// bound, not a source-curve approximation allowance.
const DIRECT: f64 = 1.0 / (255.0 * 1024.0);

/// Original controls and reusable certification facts for one linear map.
/// The Arc replaces the original Path's Arc; placement data stays borrowed.
#[derive(Debug)]
pub(in crate::band) struct Proof {
    pub(in crate::band) outline: Path,
    pub(in crate::band) linear: Transform,
    direct: Option<Direct>,
    curved: bool,
    share: f64,
    whole: f64,
    shares: Option<Shares>,
}

impl Proof {
    pub(super) fn new(
        outline: &Path,
        linear: Transform,
        relative: Option<&Relative>,
        bytes: &mut Meter,
    ) -> Result<Arc<Self>> {
        bytes.charge(size_of::<Self>() + 2 * size_of::<usize>())?;
        let outline = outline.clone_with_bytes(bytes)?;
        let direct = relative.and_then(|relative| Direct::new(&outline, linear, relative));
        let curved = outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }));
        let (curves, share) = area_share(&outline, true);
        let whole = outward(f64::from(u32::try_from(curves).unwrap_or(u32::MAX)) * share);
        // Ordinary analytic placements need no boxes. Cache them for the
        // other glyphs only if the owner can retain them; a tight owner
        // budget leaves the same transient, per-placement construction.
        let before = bytes.used();
        let shares = if direct.is_none() && curved {
            if let Ok(shares) = Shares::new(&outline, (linear, share), bytes) {
                shares
            } else {
                bytes.refund(bytes.used() - before);
                None
            }
        } else {
            None
        };
        Ok(Arc::new(Self {
            outline,
            linear,
            direct,
            curved,
            share,
            whole,
            shares,
        }))
    }

    #[cfg(test)]
    pub(super) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + 2 * size_of::<usize>()
            + self.outline.retained_bytes()
            + self.shares.as_ref().map_or(0, Shares::bytes)
    }
}

/// Cached once per source outline and linear map. Kept quadratics remove
/// reduction error, but not cancellation in H or mapping and translation.
#[derive(Debug)]
pub(super) struct Direct {
    /// Largest translation whose arithmetic allowance still fits DIRECT.
    origin: f64,
}

impl Direct {
    pub(super) fn new(outline: &Path, linear: Transform, relative: &Relative) -> Option<Self> {
        if outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            return None;
        }
        let pieces = f64::from(
            u32::try_from(
                outline
                    .segments()
                    .len()
                    .max(relative.lines.len() + relative.curves.len()),
            )
            .unwrap_or(u32::MAX),
        );
        let arithmetic = relative
            .curves
            .iter()
            .map(curve::Mono::arithmetic_allowance)
            .fold(0.0, |sum, error| (sum + error).next_up());
        // Solve arithmetic + 384*EPS*pieces*(magnitude + |origin|)
        // <= DIRECT once. Round every inverse step down so the cached
        // translation bound cannot admit more than the original allowance.
        let room = (DIRECT - arithmetic).next_down();
        let magnitude = mapped_magnitude(outline, linear, Point::default());
        let origin = ((room / (384.0 * f64::EPSILON * pieces)).next_down() - magnitude).next_down();
        (origin >= 0.0).then_some(Self { origin })
    }

    fn admits(&self, origin: Point) -> bool {
        origin.x.abs().max(origin.y.abs()) <= self.origin
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::band) struct Pixel {
    pub(in crate::band) x: u32,
    pub(in crate::band) y: u32,
    pub(in crate::band) byte: u8,
}

#[derive(Clone, Copy)]
pub(in crate::band) struct Source<'a> {
    pub(in crate::band) outline: &'a Path,
    pub(in crate::band) linear: Transform,
    pub(in crate::band) origin: Point,
    pub(in crate::band) points: &'a [Point],
}

impl<'a> Source<'a> {
    fn of(edges: &'a GlyphEdges, origin: Point, work: &mut Meter) -> Result<Option<Self>> {
        let Some(proof) = edges.source() else {
            return Ok(None);
        };
        let points = match &edges.kind {
            Kind::Curve { point, .. } => point.as_slice(),
            Kind::Union { points, .. } | Kind::Polygon { points, .. } => points.as_slice(),
        };
        let (outline, linear) = (&proof.outline, proof.linear);
        work.charge(outline.segments().len())?;
        Ok(proof.curved.then_some(Self {
            outline,
            linear,
            origin,
            points,
        }))
    }
}

/// A placement leaves this seam in the ordinary shape representation;
/// there are no execution-time corrections or fallible raster operations.
pub(super) fn certify(
    edges: &GlyphEdges,
    origin: Point,
    mut shape: Shape,
    options: &Prep<'_>,
) -> Result<Shape> {
    if exact(edges, origin, &shape, options) {
        return Ok(shape);
    }
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    let Some(proof) = edges.source() else {
        return Ok(shape);
    };
    let Some(source) = Source::of(edges, origin, &mut work)? else {
        return Ok(shape);
    };
    let Source {
        outline, linear, ..
    } = source;
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes())?;
    let mut targets = Vec::new();
    let owned;
    let shares = if let Some(shares) = &proof.shares {
        // The same per-placement reservation as transient construction:
        // caching must not change render-budget admission.
        bytes.charge(shares.bytes())?;
        Some(shares)
    } else {
        owned = Shares::new(outline, (linear, proof.share), &mut bytes)?;
        owned.as_ref()
    };
    let whole = proof.whole;
    let share_bytes = shares.as_ref().map_or(0, |shares| shares.bytes());
    for round in 0..=ROUNDS {
        work.charge_product(outline.segments().len(), 2)?;
        let placement = placement_error(outline, linear, origin, &shape);
        let clip = options
            .clip
            .and_then(|clip| clip.paths())
            .map_or(0.0, Paths::arithmetic_error);
        let bound = match &shares {
            _ if analytic(&shape) => Bound::Whole(
                curve::reduction_area(outline, linear) + allowance(edges) + placement + clip,
            ),
            Some(shares) => Bound::Shared {
                shares,
                origin,
                whole,
                rest: placement + clip,
            },
            None => Bound::Whole(whole + placement + clip),
        };
        let pending = ambiguous(&shape, &bound, (&mut work, &mut bytes))?;
        let mut wrong = false;
        for pixel in pending {
            let expected = true_byte(source, pixel, options, (&mut work, &mut bytes))?;
            if expected != pixel.byte {
                wrong = true;
                let target = [
                    f64::from(pixel.x),
                    f64::from(pixel.y),
                    f64::from(pixel.x) + 1.0,
                    f64::from(pixel.y) + 1.0,
                ];
                work.charge(targets.len())?;
                if !targets.contains(&target) {
                    bytes.reserve_capacity(&mut targets, 1)?;
                    targets.push(target);
                }
            }
        }
        let target_bytes = targets.capacity() * size_of::<[f64; 4]>();
        // Inspection cells, pending pixels and local interval geometry have
        // left scope. Keep the shares, the target allocation and the live
        // shape charged.
        bytes.refund(
            bytes
                .used()
                .saturating_sub(shape.retained_bytes() + target_bytes + share_bytes),
        );
        if !wrong {
            return Ok(shape);
        }
        if round == ROUNDS {
            break;
        }
        shape = replacement(
            source,
            shape,
            &targets,
            (options, round),
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )?;
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

fn replacement(
    source: Source<'_>,
    shape: Shape,
    targets: &[[f64; 4]],
    (options, round): (&Prep<'_>, i32),
    Meters { work, bytes }: Meters<'_>,
) -> Result<Shape> {
    let before = bytes.used();
    let (polygon, points) = flattened_with(
        source.outline,
        source.linear,
        options.limits,
        bytes,
        Some(Inspection {
            hulls: None,
            work,
            rounding: 0.0,
            targets,
            offset: source.origin,
            factor: 16.0_f64.powi(-round - 1),
        }),
    )?;
    let polygon_bytes = polygon.heap_bytes() + points.capacity() * size_of::<Point>();
    bytes.refund(bytes.used().saturating_sub(before + polygon_bytes));
    // Old geometry stays charged until its replacement is fully ready. As
    // for any polygon placement, only the rows its winding certificate
    // leaves in doubt are resolved; the next round certifies the bytes.
    let prepared = translated(
        (&polygon, &points),
        source.origin,
        Winding::Check(0.0),
        options,
        bytes,
        Some(work),
        None,
    )?
    .ok_or(Error::limit("unresolved curve coverage interval"))?;
    drop((polygon, points));
    bytes.refund(polygon_bytes);
    // The replacement's row resolver and lists have dropped their
    // scratch. Keep only old geometry, targets and the ready replacement
    // charged before the next cell inspection can allocate anything.
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before + prepared.retained_bytes()),
    );
    let released = shape.retained_bytes();
    drop(shape);
    bytes.refund(released);
    Ok(prepared)
}

/// Whether `shape` integrates quadratics in closed form.
fn analytic(shape: &Shape) -> bool {
    match &shape.body {
        Body::Glyph(body) => {
            body.geometry == GlyphGeometry::Relative
                && matches!(body.edges.kind, Kind::Curve { .. } | Kind::Union { .. })
        }
        Body::Edges(body) => !body.curves.is_empty(),
    }
}

/// The area by which a covered set's parts can miss it; zero for a
/// certified spline's pieces.
fn allowance(edges: &GlyphEdges) -> f64 {
    match edges.kind {
        Kind::Union { allowance, .. } => allowance,
        Kind::Curve { .. } | Kind::Polygon { .. } => 0.0,
    }
}

/// Direct integration is reserved for well-conditioned kept quadratics.
/// Ill-conditioned mapping, large pieces or placement cancellation use the
/// measured certificate even though their reduction error is zero.
fn exact(edges: &GlyphEdges, origin: Point, shape: &Shape, options: &Prep<'_>) -> bool {
    analytic(shape)
        && options.clip.and_then(|clip| clip.paths()).is_none()
        && edges
            .source()
            .and_then(|source| source.direct.as_ref())
            .is_some_and(|direct| direct.admits(origin))
}

/// Translation and row/intersection arithmetic can move a boundary. The
/// per-edge homotopy bound is at most three times the two coordinate
/// displacements inside any pixel. This broad initial bound only selects
/// pixels for the local, original-control certificate below.
fn placement_error(outline: &Path, linear: Transform, origin: Point, shape: &Shape) -> f64 {
    let magnitude = mapped_magnitude(outline, linear, origin);
    let pieces = f64::from(
        u32::try_from(outline.segments().len().max(shape.boundaries())).unwrap_or(u32::MAX),
    );
    384.0 * f64::EPSILON * magnitude * pieces
}

fn mapped_magnitude(outline: &Path, linear: Transform, origin: Point) -> f64 {
    // Per axis, so the two maxima do not wait on each other.
    let mut magnitude = [origin.x.abs().max(origin.y.abs()).max(1.0); 2];
    for segment in outline.segments() {
        let mut include = |p: Point| {
            magnitude = [
                magnitude[0].max((p.x * linear.a).abs() + (p.y * linear.c).abs() + origin.x.abs()),
                magnitude[1].max((p.x * linear.b).abs() + (p.y * linear.d).abs() + origin.y.abs()),
            ];
        };
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => include(p),
            Segment::CubicTo { c1, c2, end } => {
                include(c1);
                include(c2);
                include(end);
            }
            Segment::QuadTo { control, end } => {
                include(control);
                include(end);
            }
            Segment::Close => {}
        }
    }
    magnitude[0].max(magnitude[1])
}

/// Each curve's share of a polygon's area allowance and the boxes of the
/// hulls that spend it, relative to the glyph origin.
#[derive(Debug)]
struct Shares {
    share: f64,
    boxes: Vec<[f64; 4]>,
}

impl Shares {
    fn bytes(&self) -> usize {
        self.boxes.capacity() * size_of::<[f64; 4]>()
    }
    /// `None` for an outline with cubics, whose reduced quadratics may
    /// leave their control boxes.
    fn new(
        outline: &Path,
        (linear, share): (Transform, f64),
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        if outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            return Ok(None);
        }
        let mut boxes = Vec::new();
        bytes.reserve(&mut boxes, outline.segments().len())?;
        boxes.extend(crate::flatten::curve_boxes(outline, linear));
        Ok(Some(Self { share, boxes }))
    }

    /// The area allowance the hulls of a placement at `origin` can spend
    /// in pixels `x0..x1` of row `y`.
    fn within(&self, origin: Point, y: u32, (x0, x1): (u32, u32)) -> f64 {
        let pixels = [
            (f64::from(x0) - origin.x).next_down(),
            (f64::from(y) - origin.y).next_down(),
            (f64::from(x1) - origin.x).next_up(),
            (f64::from(y) + 1.0 - origin.y).next_up(),
        ];
        let meeting = self
            .boxes
            .iter()
            .filter(|&&bounds| Hull { bounds, area: 0.0 }.intersects(pixels));
        outward(f64::from(u32::try_from(meeting.count()).unwrap_or(u32::MAX)) * self.share)
    }
}

/// How far a placement's area can be from the original outline's, apart
/// from the arithmetic of its cells.
enum Bound<'a> {
    /// One bound for every pixel.
    Whole(f64),
    /// The polygon's area allowance over the whole outline, which a pixel's
    /// curves can spend only as far as their hulls meet it, plus `rest`.
    Shared {
        shares: &'a Shares,
        origin: Point,
        whole: f64,
        rest: f64,
    },
}

/// The pixels whose byte the shape's area within `bound` leaves in doubt.
fn ambiguous(
    shape: &Shape,
    bound: &Bound<'_>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<Vec<Pixel>> {
    let width = match &shape.body {
        Body::Glyph(body) => body.scan.1.saturating_sub(body.scan.0),
        Body::Edges(body) => body.scan.1.saturating_sub(body.scan.0),
    };
    let mut cells = Cells::new(width, bytes)?;
    let cell_bytes = cells.retained_bytes();
    work.charge(shape.work())?;
    let mut pending = Vec::new();
    let mut failure = None;
    raster::inspect(shape, &mut cells, |y, x0, x1, area, arithmetic| {
        // Signed winding can exceed one in the canonical point cell.
        // Clamp the certificate center before adding uncertainty, so a
        // saturated raw area cannot hide an original-control error.
        let center = area.clamp(0.0, 1.0);
        let certain = |error: f64| {
            let error = outward(error + arithmetic);
            area.is_finite()
                && error.is_finite()
                && quantize_unit(center - error) == quantize_unit(center + error)
        };
        let whole = match *bound {
            Bound::Whole(error) => error,
            Bound::Shared { whole, rest, .. } => whole + rest,
        };
        if failure.is_some() || certain(whole) {
            return;
        }
        let result = (|| {
            if let Bound::Shared {
                shares,
                origin,
                rest,
                ..
            } = *bound
            {
                work.charge(shares.boxes.len())?;
                let column =
                    |x| u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"));
                let columns = (column(x0)?, column(x1)?);
                if certain(shares.within(origin, y, columns) + rest) {
                    return Ok(());
                }
            }
            work.charge(x1 - x0)?;
            bytes.reserve_capacity(&mut pending, x1 - x0)?;
            for x in x0..x1 {
                pending.push(Pixel {
                    x: u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"))?,
                    y,
                    byte: quantize_unit(area),
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            failure = Some(error);
        }
    })?;
    drop(cells);
    bytes.refund(cell_bytes);
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(pending)
}

fn true_byte(
    source: Source<'_>,
    pixel: Pixel,
    options: &Prep<'_>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<u8> {
    for round in 0..=ROUNDS {
        let before = bytes.used();
        let result = interval(source, pixel, (options, round), (work, bytes));
        // All interval geometry and resolver scratch has left scope.
        bytes.refund(bytes.used().saturating_sub(before));
        let (area, error) = result?;
        let lo = quantize_unit(area - error);
        if lo == quantize_unit(area + error) {
            return Ok(lo);
        }
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

fn interval(
    source: Source<'_>,
    pixel: Pixel,
    (options, round): (&Prep<'_>, i32),
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<(f64, f64)> {
    let frame = Frame::new(options).ok_or(Error::limit("unresolved curve coverage interval"))?;
    let bounds = clip_bounds(&frame, pixel);
    let mut pieces = Vec::new();
    let mut error = source_pieces(
        source,
        pixel,
        (options, round),
        (bounds, 0),
        &mut pieces,
        (work, bytes),
    )?;
    let mut rules = Vec::new();
    bytes.reserve_capacity(&mut rules, 1)?;
    rules.push(FillRule::NonZero);
    error = outward(
        error
            + clip_pieces(
                &frame,
                pixel,
                (&mut pieces, &mut rules),
                Meters { work, bytes },
            )?,
    );
    work.charge(pieces.len())?;
    let mut sides = Vec::new();
    Resolver::default().row(&pieces, &rules, 0.0, (&mut sides, Meters { work, bytes }))?;
    let mut cells = Cells::new(1, bytes)?;
    if !cells.open((0, 1), 0..1) {
        return Err(Error::limit("curve interval cells exceed allocation"));
    }
    work.charge_product(sides.len(), 8)?;
    for side in sides {
        cells.line_measured(side);
    }
    let mut area = 0.0;
    cells.inspect(0, |_, _, value| area = value.abs());
    error = outward(error + cells.arithmetic_error());
    Ok((area, error))
}

fn clip_pieces(
    frame: &Frame<'_>,
    pixel: Pixel,
    (pieces, rules): (&mut Vec<Piece>, &mut Vec<FillRule>),
    Meters { work, bytes }: Meters<'_>,
) -> Result<f64> {
    let mut error = 0.0;
    if let Some(rect) = frame.rect {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        bytes.reserve_capacity(rules, 1)?;
        rules.push(FillRule::NonZero);
        let corners = [
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
            Point::new(rect.right, rect.bottom),
            Point::new(rect.left, rect.bottom),
        ];
        for i in 0..4 {
            let (line, bound) = localized_line(
                Line {
                    a: corners[i],
                    b: corners[(i + 1) % 4],
                },
                pixel,
            );
            error = outward(
                error
                    + edge_error(line, bound, [0.0, 0.0, 1.0, 1.0])
                    + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]),
            );
            if let Some(piece) = Piece::new(line, 0.0, group) {
                bytes.reserve_capacity(pieces, 1)?;
                pieces.push(piece);
            }
        }
    }
    if let Some(paths) = frame.paths {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        let start = pieces.len();
        paths.pieces(pixel.y, group, (pieces, bytes), work)?;
        for piece in pieces.get_mut(start..).unwrap_or_default() {
            let (line, bound) = localized_line(piece.line, pixel);
            error = outward(
                error
                    + edge_error(line, bound, [0.0, 0.0, 1.0, 1.0])
                    + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]),
            );
            *piece = Piece::new(line, 0.0, piece.group)
                .ok_or(Error::limit("unresolved clip localization interval"))?;
        }
        let count = paths.rules().count();
        bytes.reserve_capacity(rules, count)?;
        rules.extend(paths.rules());
    }
    Ok(error)
}

/// Error-free two-sum subtraction, retaining the exact stored-coordinate
/// difference even when the integer pixel lies far from an endpoint.
fn subtract(a: f64, b: f64) -> (f64, f64) {
    let high = a - b;
    let back = high - a;
    (high, (a - (high - back)) + (-b - back))
}

pub(in crate::band) fn outward(value: f64) -> f64 {
    value * (1.0 + 16.0 * f64::EPSILON)
}

pub(in crate::band) fn localized_line(line: Line, pixel: Pixel) -> (Line, Point) {
    let shift = |p: Point| {
        let (x, dx) = subtract(p.x, f64::from(pixel.x));
        let (y, dy) = subtract(p.y, f64::from(pixel.y));
        (Point::new(x, y), Point::new(dx.abs(), dy.abs()))
    };
    let (a, da) = shift(line.a);
    let (b, db) = shift(line.b);
    (Line { a, b }, Point::new(da.x.max(db.x), da.y.max(db.y)))
}

pub(in crate::band) fn edge_error(line: Line, delta: Point, bounds: [f64; 4]) -> f64 {
    let radius = delta.x.max(delta.y);
    let hull = Hull {
        bounds: [
            (line.a.x.min(line.b.x) - radius).next_down(),
            (line.a.y.min(line.b.y) - radius).next_down(),
            (line.a.x.max(line.b.x) + radius).next_up(),
            (line.a.y.max(line.b.y) + radius).next_up(),
        ],
        area: 0.0,
    };
    if !hull.intersects(bounds) {
        return 0.0;
    }
    // Each coordinate of the linear homotopy is monotone. Perturbation
    // adds at most two radii to its variation in a unit pixel; tangential
    // displacement of an axis-aligned edge sweeps no area.
    outward(
        4.0 * (delta.x * ((line.b.y - line.a.y).abs() + 2.0 * delta.y).min(1.0)
            + delta.y * ((line.b.x - line.a.x).abs() + 2.0 * delta.x).min(1.0))
            + 16.0 * delta.x * delta.y,
    )
}

fn clip_bounds(frame: &Frame<'_>, pixel: Pixel) -> [f64; 4] {
    let mut bounds = [0.0_f64, 0.0, 1.0, 1.0];
    let clips = frame
        .rect
        .into_iter()
        .chain(frame.paths.and_then(Paths::proof_bounds));
    for Bounds {
        left,
        top,
        right,
        bottom,
    } in clips
    {
        let local = |a, b, lower| {
            let (high, tail): (f64, f64) = subtract(a, b);
            if lower {
                (high - tail.abs()).next_down()
            } else {
                (high + tail.abs()).next_up()
            }
        };
        bounds[0] = bounds[0].max(local(left, f64::from(pixel.x), true));
        bounds[1] = bounds[1].max(local(top, f64::from(pixel.y), true));
        bounds[2] = bounds[2].min(local(right, f64::from(pixel.x), false));
        bounds[3] = bounds[3].min(local(bottom, f64::from(pixel.y), false));
    }
    bounds
}

fn source_lines(
    outline: &Path,
    linear: Transform,
    origin: Point,
    pixel: Pixel,
    local: Transform,
    bounds: [f64; 4],
    work: &mut Meter,
) -> Result<f64> {
    work.charge(outline.segments().len())?;
    let (mut current, mut start) = (None, Point::default());
    let mut error = 0.0;
    let line = |a: Point, b: Point| {
        let canonical = |p: Point| {
            let p = p * linear;
            Point::new(p.x + origin.x, p.y + origin.y)
        };
        let (target, tail) = localized_line(
            Line {
                a: canonical(a),
                b: canonical(b),
            },
            pixel,
        );
        let placed = Line {
            a: a * local,
            b: b * local,
        };
        let delta = Point::new(
            outward(
                (target.a.x - placed.a.x)
                    .abs()
                    .max((target.b.x - placed.b.x).abs())
                    + tail.x,
            ),
            outward(
                (target.a.y - placed.a.y)
                    .abs()
                    .max((target.b.y - placed.b.y).abs())
                    + tail.y,
            ),
        );
        edge_error(placed, delta, bounds)
    };
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, start));
                }
                current = Some(p);
                start = p;
            }
            Segment::LineTo(p) => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, p));
                }
                current = Some(p);
            }
            Segment::CubicTo { end, .. } | Segment::QuadTo { end, .. } => current = Some(end),
            Segment::Close => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, start));
                }
                current = Some(start);
            }
        }
    }
    if let Some(previous) = current {
        error = outward(error + line(previous, start));
    }
    Ok(error)
}

#[cfg(test)]
mod tests {
    use super::super::AREA;
    use super::*;
    use crate::{Area, ClipShape, DeviceRect, PathBuilder, ShapeOptions};
    use pagura_core::Limits;

    #[test]
    fn a_pixel_spends_only_the_shares_of_the_curves_that_reach_it() {
        // Two quadratic bumps far apart, then a straight side.
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.quad_to(Point::new(1.0, 2.0), Point::new(2.0, 0.0))
            .unwrap();
        path.line_to(Point::new(20.0, 0.0)).unwrap();
        path.quad_to(Point::new(21.0, 2.0), Point::new(22.0, 0.0))
            .unwrap();
        path.close();
        let path = path.finish();
        let share = area_share(&path, true).1;
        let shares = Shares::new(
            &path,
            (Transform::IDENTITY, share),
            &mut crate::testing::bytes(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(shares.boxes.len(), 2);
        assert_eq!(share, AREA / 2.0);
        let origin = Point::new(100.25, 50.5);
        let within = |x: u32, y: u32, n: u32| shares.within(origin, y, (x, x + n));
        assert_eq!(within(110, 50, 1), 0.0);
        assert!((share..1.01 * share).contains(&within(101, 50, 1)));
        assert!((2.0 * share..1.01 * AREA).contains(&within(100, 50, 30)));
        // Below the bumps' boxes.
        assert_eq!(within(100, 60, 30), 0.0);
        // Keeping the same boxes charges their owner. If they do not fit,
        // construction still succeeds and placements build them transiently.
        let mut bytes = crate::testing::bytes();
        let proof = Proof::new(&path, Transform::IDENTITY, None, &mut bytes).unwrap();
        let cached = proof.shares.as_ref().unwrap();
        assert_eq!(cached.boxes, shares.boxes);
        assert_eq!(bytes.used(), proof.retained_bytes());
        let essential = bytes.used() - cached.bytes();
        let mut bytes = Meter::new(essential, "test");
        let tight = Proof::new(&path, Transform::IDENTITY, None, &mut bytes).unwrap();
        assert!(tight.shares.is_none());
        assert_eq!(tight.whole, proof.whole);
        assert_eq!(tight.share, proof.share);
        assert_eq!(bytes.used(), essential);
        assert_eq!(bytes.used(), tight.retained_bytes());
        // A certified polygon shares the allowance among at most eight
        // curves; one used as it is, among all of them.
        let mut many = PathBuilder::new();
        many.move_to(Point::new(0.0, 0.0));
        for i in 0..20 {
            let x = f64::from(i);
            many.quad_to(Point::new(x + 0.5, 1.0), Point::new(x + 1.0, 0.0))
                .unwrap();
        }
        let many = many.finish();
        assert_eq!(area_share(&many, true), (20, AREA / 8.0));
        assert_eq!(area_share(&many, false), (20, AREA / 20.0));
        // Reduced cubics may leave their control boxes.
        let mut cubic = PathBuilder::new();
        cubic.move_to(Point::new(0.0, 0.0));
        cubic
            .cubic_to(
                Point::new(1.0, 1.0),
                Point::new(2.0, 1.0),
                Point::new(3.0, 0.0),
            )
            .unwrap();
        let cubic = cubic.finish();
        let none = Shares::new(
            &cubic,
            (Transform::IDENTITY, share),
            &mut crate::testing::bytes(),
        );
        assert!(none.unwrap().is_none());
    }

    #[test]
    fn mixed_rows_keep_original_curve_and_point_enclosures() {
        let limits = Limits::default();
        let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
        for reversed in [false, true] {
            let mut path = PathBuilder::new();
            if reversed {
                path.move_to(Point::new(0.0, 7.0));
                path.line_to(Point::new(6.0, 7.0)).unwrap();
                path.line_to(Point::new(6.0, 0.0)).unwrap();
                path.cubic_to(Point::new(4.0, 4.0), Point::new(2.0, 4.0), Point::default())
                    .unwrap();
            } else {
                path.move_to(Point::default());
                path.cubic_to(
                    Point::new(2.0, 4.0),
                    Point::new(4.0, 4.0),
                    Point::new(6.0, 0.0),
                )
                .unwrap();
                path.line_to(Point::new(6.0, 7.0)).unwrap();
                path.line_to(Point::new(0.0, 7.0)).unwrap();
            }
            path.close();
            path.move_to(Point::new(2.25, 4.25));
            path.close();
            let path = path.finish();
            let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
            for band_height in [1, 3, 16, 32] {
                let options = Prep {
                    area: Area {
                        region: DeviceRect {
                            x0: 0,
                            y0: 0,
                            x1: 8,
                            y1: 10,
                        },
                        band_height,
                    },
                    limits: &limits,
                    clip: None,
                };
                let shape = super::super::placed(&edges, origin, &options)
                    .unwrap()
                    .unwrap();
                assert!(shape.resolved.is_empty());
                let relative = edges.relative().unwrap();
                assert_eq!(
                    shape.boundaries(),
                    relative.lines.len() + relative.curves.len() + 2
                );
                let ordinary = placement_error(&path, Transform::IDENTITY, origin, &shape);
                let expected = 384.0
                    * f64::EPSILON
                    * (7.0 + origin.y)
                    * f64::from(
                        u32::try_from(path.segments().len().max(shape.boundaries())).unwrap(),
                    );
                assert_eq!(ordinary.to_bits(), expected.to_bits());
                // The original cubic has x=6t and y=12t(1-t). Over x=2..3,
                // its mean height is26/9, so the covered part of row3 is
                // 4-origin.y-26/9. This evaluation avoids the raster kernel.
                let original = 10.0 / 9.0 - origin.y;
                assert_eq!(quantize_unit(original), 127);
                let mut found = false;
                let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
                raster::inspect(&shape, &mut cells, |y, x0, x1, value, arithmetic| {
                    if y == 3 && x0 <= 2 && x1 > 2 {
                        found = true;
                        let error = ordinary + arithmetic + 8.0 * f64::EPSILON;
                        assert!((value.abs() - original).abs() <= error);
                    }
                })
                .unwrap();
                assert!(found);
                let ready = certify(&edges, origin, shape, &options).unwrap();
                let (coverage, _) = crate::testing::raster(&options.area, Some(&ready), None);
                assert_eq!(coverage[3 * 8 + 2], 127);
                assert_eq!(coverage[4 * 8 + 2], if reversed { 0 } else { 255 });
            }
        }
    }

    #[test]
    fn saturated_winding_keeps_original_control_and_nonfinite_uncertainty() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(2.0, 4.0),
            Point::new(4.0, 4.0),
            Point::new(6.0, 0.0),
        )
        .unwrap();
        path.line_to(Point::new(6.0, 7.0)).unwrap();
        path.line_to(Point::new(0.0, 7.0)).unwrap();
        path.close();
        path.move_to(Point::new(2.25, 4.25));
        path.close();
        let edges =
            Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 8,
                    y1: 8,
                },
                band_height: 3,
            },
            limits: &limits,
            clip: None,
        };
        let shape = super::super::placed(&edges, Point::default(), &options)
            .unwrap()
            .unwrap();
        let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
        let mut found = false;
        raster::inspect(&shape, &mut cells, |y, x0, x1, area, _| {
            if y == 4 && x0 <= 2 && x1 > 2 {
                found = true;
                assert!(area > 1.5);
            }
        })
        .unwrap();
        assert!(found);
        for uncertainty in [1.0 / 255.0, f64::NAN, f64::INFINITY] {
            let mut work = Meter::new(limits.raster_work, RASTER_WORK);
            let mut bytes = crate::testing::bytes();
            let pending =
                ambiguous(&shape, &Bound::Whole(uncertainty), (&mut work, &mut bytes)).unwrap();
            let pixel = *pending.iter().find(|p| p.x == 2 && p.y == 4).unwrap();
            assert_eq!(pixel.byte, 255);
            let source = Source::of(&edges, Point::default(), &mut work)
                .unwrap()
                .unwrap();
            assert_eq!(
                true_byte(source, pixel, &options, (&mut work, &mut bytes)).unwrap(),
                255
            );
        }
    }

    #[test]
    fn local_proof_chords_keep_nonpoint_loops_distinct_from_points() {
        let limits = Limits::default();
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let mut clip_path = PathBuilder::new();
        clip_path.rect(0.75, 0.75, 0.25, 0.25);
        let clip_path = clip_path.finish();
        let clip = ClipShape::new(
            &[(&clip_path, FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap();
        let options = Prep {
            area,
            limits: &limits,
            clip: Some(&clip),
        };
        let start = Point::new(0.125, 0.125);
        for (control, expected) in [(Point::new(0.25, 0.25), 0), (start, 16)] {
            let mut path = PathBuilder::new();
            path.move_to(start);
            path.cubic_to(control, control, start).unwrap();
            path.close();
            let path = path.finish();
            let edges = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
            let mut work = Meter::new(limits.raster_work, RASTER_WORK);
            let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
            let source = Source::of(&edges, Point::default(), &mut work)
                .unwrap()
                .unwrap();
            let (value, error) = interval(
                source,
                Pixel {
                    x: 0,
                    y: 0,
                    byte: 0,
                },
                (&options, 0),
                (&mut work, &mut bytes),
            )
            .unwrap();
            // The out-and-back curve is nonpoint geometry outside the
            // partial clip. A genuine point still owns its whole floor cell.
            assert_eq!(quantize_unit(value - error), expected);
            assert_eq!(quantize_unit(value + error), expected);
        }
    }

    #[test]
    fn analytic_integral_cancellation_cannot_skip_a_coverage_byte() {
        let limits = Limits::default();
        let m = 98_304.0;
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(m / 6.0, m / 3.0),
            Point::new(m / 2.0, 2.0 * m / 3.0),
            Point::new(m, m),
        )
        .unwrap();
        path.line_to(Point::new(m + 2.0, m)).unwrap();
        path.line_to(Point::new(m + 2.0, 0.0)).unwrap();
        path.close();
        let path = path.finish();
        let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(edges.curved());
        let origin = Point::new(-98_302.997_063_444_05, 1.0 - m);
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                },
                band_height: 1,
            },
            limits: &limits,
            clip: None,
        };
        // Recover z=M(1-t) from x=0 using the original quadratic, then
        // integrate its pixel-local polynomial. This avoids H(tb)-H(ta)
        // and its large, nearly equal antiderivative values entirely.
        let u = origin.x + m;
        let z = 2.0 * u / (1.5 + (2.25 - 2.0 * u / m).sqrt());
        let exact = 1.0 - (u * z - 0.75 * z * z + z * z * z / (6.0 * m));
        assert!((exact - 0.664_705_581_461_998_8).abs() < 2e-15);
        assert_eq!(quantize_unit(exact), 169);
        let prepared = super::super::placed(&edges, origin, &options)
            .unwrap()
            .unwrap();
        let mut cells = Cells::new(1, &mut Meter::new(limits.render_bytes, RENDER_BYTES)).unwrap();
        raster::inspect(&prepared, &mut cells, |_, _, _, value, error| {
            assert!(
                (value - exact).abs() <= error,
                "primitive area {value} ±{error} excludes analytic {exact}"
            );
        })
        .unwrap();
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        assert_eq!(
            ambiguous(&prepared, &Bound::Whole(0.0), (&mut work, &mut bytes))
                .unwrap()
                .len(),
            1,
        );
        match super::super::place(&edges, origin, &options) {
            Ok(Some(prepared)) => {
                raster::inspect(&prepared, &mut cells, |_, _, _, value, _| {
                    assert_eq!(quantize_unit(value), 169);
                })
                .unwrap();
            }
            Err(error) => {
                assert!(matches!(error.kind, pagura_core::ErrorKind::Limit));
                assert!(matches!(
                    error.message,
                    "unresolved curve coverage interval" | "curve subdivision depth exceeded"
                ));
            }
            Ok(None) => panic!("the mark covers the pixel"),
        }
    }

    #[test]
    fn replacement_releases_dead_builder_scratch_before_the_next_inspection() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        let h = 3.0 * 0.500_001 / 255.0;
        for _ in 0..2 {
            path.move_to(Point::default());
            path.cubic_to(
                Point::new(1.0 / 3.0, 2.0 * h / 3.0),
                Point::new(2.0 / 3.0, 2.0 * h / 3.0),
                Point::new(1.0, 0.0),
            )
            .unwrap();
            path.close();
        }
        let edges =
            Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        let options = Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                },
                band_height: 1,
            },
            limits: &limits,
            clip: None,
        };
        let shape = super::super::placed(&edges, Point::default(), &options)
            .unwrap()
            .unwrap();
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let source = Source::of(&edges, Point::default(), &mut work)
            .unwrap()
            .unwrap();
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        bytes.charge(shape.retained_bytes()).unwrap();
        let shape = replacement(
            source,
            shape,
            &[[0.0, 0.0, 1.0, 1.0]],
            (&options, 0),
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )
        .unwrap();
        assert_eq!(bytes.used(), shape.retained_bytes());
        let Body::Edges(body) = &shape.body else {
            panic!("replacement must be a polygon")
        };
        let mut needed = Meter::new(usize::MAX, RENDER_BYTES);
        Cells::charge(body.scan.1 - body.scan.0, &mut needed).unwrap();
        // One pending pixel must coexist with the cell buffer. Dead row
        // builder allocations must not consume this exact next-stage peak.
        let mut tight = Meter::new(
            bytes.used() + needed.used() + size_of::<Pixel>(),
            RENDER_BYTES,
        );
        tight.charge(bytes.used()).unwrap();
        let pending = ambiguous(&shape, &Bound::Whole(AREA), (&mut work, &mut tight)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(tight.used(), shape.retained_bytes() + size_of::<Pixel>());
    }

    #[test]
    fn canonical_clip_interpolation_cancellation_stays_inside_the_interval() {
        let limits = Limits::default();
        let n = 2.0_f64.powi(39);
        let u = 2.0_f64.powi(-13);
        let mut clip = PathBuilder::new();
        clip.move_to(Point::new(-n, -n));
        clip.line_to(Point::new(n, n + u)).unwrap();
        clip.line_to(Point::new(-n, n + 1.0)).unwrap();
        clip.close();
        let clip = clip.finish();
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let clip = ClipShape::new(
            &[(&clip, FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap();
        // This quadratic mark contains the whole unit pixel and has its
        // curved boundary outside it. Only the canonical clip can change
        // the pixel, so zero curve/localization error is not sufficient.
        let mut mark = PathBuilder::new();
        mark.move_to(Point::new(-2.0, -2.0));
        mark.cubic_to(
            Point::new(-2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0, -2.0),
        )
        .unwrap();
        mark.line_to(Point::new(2.0, 2.0)).unwrap();
        mark.line_to(Point::new(-2.0, 2.0)).unwrap();
        mark.close();
        let mark = mark.finish();
        let source = Source {
            outline: &mark,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
        };
        let options = Prep {
            area,
            limits: &limits,
            clip: Some(&clip),
        };
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let (actual, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        // The original stored line is x(y)=(y-u/2)/(1+u/(2N)). Integrate
        // from its x=0 crossing u/2 to y=1, independently of Line::x.
        let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
        assert_eq!(quantize_unit(exact), 127);
        assert!(
            (actual - exact).abs() <= error + 1e-15,
            "clip area{actual} ±{error} excludes analytic {exact}"
        );
        let edges = Arc::new(GlyphEdges::new(&mark, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert_eq!(
            super::super::place(&edges, Point::default(), &options)
                .unwrap_err()
                .message,
            "unresolved curve coverage interval",
        );
    }

    #[test]
    fn canonical_source_line_cancellation_cannot_bypass_curve_intervals() {
        let limits = Limits::default();
        let n = 2.0_f64.powi(39);
        let u = 2.0_f64.powi(-13);
        let mut path = PathBuilder::new();
        path.move_to(Point::new(-n, -n));
        path.line_to(Point::new(n, n + u)).unwrap();
        path.line_to(Point::new(-n, n + 1.0)).unwrap();
        path.close();
        for _ in 0..2 {
            path.move_to(Point::new(3.0, 3.0));
            path.cubic_to(
                Point::new(3.0 + 1.0 / 3.0, 3.0 + 2.0 / 3.0),
                Point::new(3.0 + 2.0 / 3.0, 3.0 + 2.0 / 3.0),
                Point::new(4.0, 3.0),
            )
            .unwrap();
            path.close();
        }
        let path = path.finish();
        let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(!edges.curved());
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let options = Prep {
            area,
            limits: &limits,
            clip: None,
        };
        let source = Source {
            outline: &path,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
        };
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let (actual, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
        assert_eq!(quantize_unit(exact), 127);
        assert!((actual - exact).abs() <= error + 1e-15);
        assert_eq!(
            super::super::place(&edges, Point::default(), &options)
                .unwrap_err()
                .message,
            "unresolved curve coverage interval",
        );
    }
}

/// Adds one original-control source to a localized joint coverage proof.
pub(in crate::band) fn source_pieces(
    source: Source<'_>,
    pixel: Pixel,
    (options, round): (&Prep<'_>, i32),
    (bounds, group): ([f64; 4], u8),
    pieces: &mut Vec<Piece>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<f64> {
    let Source {
        outline,
        linear,
        origin,
        points: source_points,
    } = source;
    let (ox, dx) = subtract(origin.x, f64::from(pixel.x));
    let (oy, dy) = subtract(origin.y, f64::from(pixel.y));
    let local = Transform {
        e: ox,
        f: oy,
        ..linear
    };
    let mut hulls = Vec::new();

    let targets = [bounds];
    let before = bytes.used();
    let (mut polygon, local_points) = flattened_with(
        outline,
        local,
        options.limits,
        bytes,
        Some(Inspection {
            hulls: Some(&mut hulls),
            work,
            rounding: dx.abs().max(dy.abs()) * (1.0 + 8.0 * f64::EPSILON),
            targets: &targets,
            offset: Point::default(),
            factor: 16.0_f64.powi(-round),
        }),
    )?;
    let retained = polygon.heap_bytes()
        + local_points.capacity() * size_of::<Point>()
        + hulls.capacity() * size_of::<Hull>();
    bytes.refund(bytes.used().saturating_sub(before + retained));
    let released = local_points.capacity() * size_of::<Point>();
    drop(local_points);
    bytes.refund(released);
    for &point in source_points {
        // A genuine point's floor belongs to its canonical placed position.
        let point = Point::new(
            (point.x + origin.x).floor() - f64::from(pixel.x),
            (point.y + origin.y).floor() - f64::from(pixel.y),
        );
        polygon.cell(point, options.limits, bytes)?;
    }
    let lines = edges::lines(&polygon, bytes)?;
    let released = polygon.heap_bytes();
    drop(polygon);
    bytes.refund(released);
    bytes.reserve_capacity(pieces, lines.len())?;
    pieces.extend(
        lines
            .iter()
            .filter_map(|&line| Piece::new(line, 0.0, group)),
    );
    work.charge(lines.len())?;
    let arithmetic = lines.iter().fold(0.0, |sum, &line| {
        outward(sum + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]))
    });
    let released = lines.capacity() * size_of::<Line>();
    drop(lines);
    bytes.refund(released);
    let mut error = hulls.iter().fold(0.0, |sum, hull| {
        if hull.intersects(bounds) {
            outward(sum + hull.area)
        } else {
            sum
        }
    });
    let released = hulls.capacity() * size_of::<Hull>();
    drop(hulls);
    bytes.refund(released);
    error = outward(
        error + arithmetic + source_lines(outline, linear, origin, pixel, local, bounds, work)?,
    );
    Ok(error)
}
