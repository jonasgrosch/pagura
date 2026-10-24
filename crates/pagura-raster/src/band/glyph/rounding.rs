//! Coverage-byte certification at the preparation boundary. A small area
//! error alone cannot survive repeated composition: both ends of the true
//! coverage interval must quantize to the byte execution will emit.

use super::{AREA, GlyphEdges, Kind, flattened_with, proof::Proof, translated};
use crate::band::cells::Cells;
use crate::band::clip::Paths;
use crate::band::edges::{self, Bounds, Line};
use crate::band::shape::{Body, Frame, GlyphGeometry, Shape, Winding};
use crate::band::union::{Meters, Piece, Resolver};
use crate::band::{Prep, RASTER_WORK, RENDER_BYTES, curve, raster};
use crate::flatten::{Hull, Inspection, MAX_COORDINATE, Scope};
use crate::{FillRule, Path, Point, Segment, Transform, quantize_unit};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Result};

mod rows;

#[cfg(test)]
#[path = "rounding/interval_body_tests.rs"]
mod interval_body_tests;

const ROUNDS: i32 = 12;

#[derive(Debug, Clone, Copy)]
struct Pixel {
    x: u32,
    y: u32,
    byte: u8,
}

const _: () = assert!(size_of::<Pixel>() == 12);

#[derive(Debug)]
struct Ambiguity {
    pixels: Vec<Pixel>,
    surplus: Option<usize>,
}

struct Collected {
    wrong: bool,
    surplus: Option<usize>,
}

impl Collected {
    /// Consumes this inspection's single optional loan after collection.
    fn with_rows(
        self,
        source: Source<'_>,
        shape: Shape,
        options: &Prep<'_>,
        proven: &[Pixel],
        bytes: &mut Meter,
    ) -> Shape {
        rows::replace(source, shape, options, proven, self.surplus, bytes)
    }
}

impl Ambiguity {
    fn collect(
        self,
        source: Source<'_>,
        options: &Prep<'_>,
        (targets, proven): (&mut Vec<[f64; 4]>, &mut Vec<Pixel>),
        retained: usize,
        Meters { work, bytes }: Meters<'_>,
    ) -> Result<Collected> {
        let wrong = collect_targets(
            source,
            self.pixels,
            (targets, proven),
            options,
            Meters { work, bytes },
        )?;
        // Pending pixels and interval scratch have left scope. Keep exactly
        // the old geometry/proof and the collector's two live owners charged.
        let target_bytes = targets.capacity() * size_of::<[f64; 4]>();
        let proven_bytes = proven.capacity() * size_of::<Pixel>();
        bytes.refund(
            bytes
                .used()
                .saturating_sub(retained + target_bytes + proven_bytes),
        );
        Ok(Collected {
            wrong,
            surplus: self.surplus,
        })
    }
}

#[derive(Clone, Copy)]
struct Local<'a> {
    proof: &'a Proof,
    origin: Point,
    other_error: f64,
}

#[derive(Clone, Copy)]
enum Refinement {
    Whole,
    Targeted,
}

struct Replacement {
    shape: Option<Shape>,
    proof: Option<Proof>,
    area: f64,
}

#[derive(Clone, Copy)]
struct Source<'a> {
    outline: &'a Path,
    linear: Transform,
    origin: Point,
    points: &'a [Point],
    initial_area: f64,
    proof: Option<&'a Proof>,
}

impl<'a> Source<'a> {
    fn of(edges: &'a GlyphEdges, origin: Point, work: &mut Meter) -> Result<Option<Self>> {
        let source = match &edges.kind {
            Kind::Curve {
                outline,
                linear,
                spline,
                ..
            } => Self {
                outline,
                linear: *linear,
                origin,
                points: &spline.points,
                initial_area: AREA,
                proof: None,
            },
            Kind::Polygon {
                source: Some(source),
                points,
                ..
            } => Self {
                outline: &source.outline,
                linear: source.linear,
                origin,
                points,
                initial_area: source.initial_area,
                proof: source.proof.as_ref(),
            },
            Kind::Polygon { source: None, .. } => return Ok(None),
        };
        work.charge(source.outline.segments().len())?;
        Ok(source
            .outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
            .then_some(source))
    }

    /// Coarse vertices must not hide a tight mesh's placed range error,
    /// even when the crop or clip would cull every output pixel.
    fn safely_placed(self, work: &mut Meter) -> Result<bool> {
        work.charge_product(self.outline.segments().len(), 4)?;
        let bound = |a: f64, b: f64, origin: f64| {
            ((a.abs().next_up() + b.abs().next_up()).next_up() + origin.abs()).next_up()
        };
        let safe = |p: Point| {
            bound(p.x * self.linear.a, p.y * self.linear.c, self.origin.x) <= MAX_COORDINATE / 4.0
                && bound(p.x * self.linear.b, p.y * self.linear.d, self.origin.y)
                    <= MAX_COORDINATE / 4.0
        };
        Ok(self
            .outline
            .segments()
            .iter()
            .all(|segment| match *segment {
                Segment::MoveTo(p) | Segment::LineTo(p) => safe(p),
                Segment::CubicTo { c1, c2, end } => [c1, c2, end].into_iter().all(safe),
                Segment::Close => true,
            }))
    }

    /// An outward control hull of the original placed path and the exact
    /// canonical floor cells. It contains support before any approximation.
    fn support(self, work: &mut Meter) -> Result<Option<Bounds>> {
        work.charge_product(self.outline.segments().len(), 4)?;
        work.charge(self.points.len())?;
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut finite = true;
        let mut include = |p: Point| {
            let x = mapped_range(p, [self.linear.a, self.linear.c], self.origin.x);
            let y = mapped_range(p, [self.linear.b, self.linear.d], self.origin.y);
            finite &= x.into_iter().chain(y).all(f64::is_finite);
            lo = Point::new(lo.x.min(x[0]), lo.y.min(y[0]));
            hi = Point::new(hi.x.max(x[1]), hi.y.max(y[1]));
        };
        for segment in self.outline.segments() {
            match *segment {
                Segment::MoveTo(p) | Segment::LineTo(p) => include(p),
                Segment::CubicTo { c1, c2, end } => {
                    include(c1);
                    include(c2);
                    include(end);
                }
                Segment::Close => {}
            }
        }
        for &point in self.points {
            // The point addition and integer corners are the canonical
            // operations, including corners beyond the coordinate limit.
            let [a, _, b, _] = edges::Polygon::cell_corners(point + self.origin);
            finite &= [a.x, a.y, b.x, b.y].into_iter().all(f64::is_finite);
            lo = Point::new(lo.x.min(a.x), lo.y.min(a.y));
            hi = Point::new(hi.x.max(b.x), hi.y.max(b.y));
        }
        Ok(
            (finite && [lo.x, lo.y, hi.x, hi.y].into_iter().all(f64::is_finite)).then_some(
                Bounds {
                    left: lo.x,
                    top: lo.y,
                    right: hi.x,
                    bottom: hi.y,
                },
            ),
        )
    }
}

/// Outward products and additions cover cancellation, underflow and the
/// final origin addition without a scale-dependent guessed tolerance.
fn mapped_range(point: Point, [a, b]: [f64; 2], origin: f64) -> [f64; 2] {
    let x = point.x * a;
    let y = point.y * b;
    let lo = (x.next_down() + y.next_down()).next_down();
    let hi = (x.next_up() + y.next_up()).next_up();
    [(lo + origin).next_down(), (hi + origin).next_up()]
}

fn analytic(shape: &Shape) -> bool {
    match &shape.body {
        Body::Glyph(body) => body.geometry == GlyphGeometry::Relative,
        Body::Edges(body) => !body.curves.is_empty(),
        Body::Certified(_) => false,
    }
}

/// Whether the existing inspection visits every requested pixel that the
/// original path may cover. Crop and clip bounds only restrict the test;
/// the execution scan and its left carries remain unchanged.
fn support_contained(
    source: Source<'_>,
    shape: Option<&Shape>,
    options: &Prep<'_>,
    work: &mut Meter,
) -> Result<bool> {
    let Some(frame) = Frame::new(options) else {
        return Ok(true);
    };
    let Some(support) = source.support(work)? else {
        return Ok(false);
    };
    let support = edges::pixels(
        [
            Point::new(support.left, support.top),
            Point::new(support.right, support.bottom),
        ],
        frame.limit,
        false,
    );
    if support.is_empty() {
        return Ok(true);
    }
    let Some(shape) = shape else {
        return Ok(false);
    };
    let scan = match &shape.body {
        Body::Glyph(body) => body.scan,
        Body::Edges(body) => body.scan,
        Body::Certified(_) => (shape.bounds.x0 as usize, shape.bounds.x1 as usize),
    };
    let bounds = shape.bounds();
    Ok(bounds.x0 <= support.x0
        && bounds.y0 <= support.y0
        && bounds.x1 >= support.x1
        && bounds.y1 >= support.y1
        && scan.0 <= support.x0 as usize
        && scan.1 >= support.x1 as usize)
}

fn guard_candidate(
    source: Source<'_>,
    mut shape: Option<Shape>,
    options: &Prep<'_>,
    Meters { work, bytes }: Meters<'_>,
) -> Result<(Option<Shape>, f64)> {
    if source.initial_area <= AREA
        || (source.safely_placed(work)?
            && (shape.as_ref().is_some_and(analytic)
                || support_contained(source, shape.as_ref(), options, work)?))
    {
        return Ok((shape, source.initial_area));
    }
    if let Some(candidate) = shape.take() {
        let released = candidate.retained_bytes();
        drop(candidate);
        bytes.refund(released);
    }
    Ok((
        tight_placement(source, options, Meters { work, bytes })?,
        AREA,
    ))
}

/// A placement leaves this seam in the ordinary shape representation;
/// there are no execution-time corrections or fallible raster operations.
pub(super) fn certify(
    edges: &GlyphEdges,
    origin: Point,
    shape: Option<Shape>,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    if shape.is_none()
        && !matches!(&edges.kind, Kind::Polygon { source: Some(source), .. }
                if source.initial_area > AREA)
    {
        return Ok(None);
    }
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    let Some(source) = Source::of(edges, origin, &mut work)? else {
        return Ok(shape);
    };
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.as_ref().map_or(0, Shape::retained_bytes))?;
    let (shape, mut current_area) = guard_candidate(
        source,
        shape,
        options,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )?;
    let Some(mut shape) = shape else {
        return Ok(None);
    };
    // A leaf envelope belongs to this stored polygon. Analytic geometry
    // and every tight replacement keep their own approximation bounds.
    let mut shared_proof = source
        .proof
        .filter(|_| current_area > AREA && !analytic(&shape));
    let mut owned_proof = None;
    let mut targets = Vec::new();
    // Successful original-control bytes belong only to this immutable
    // source/clip invocation, never to the returned execution geometry.
    let mut proven = Vec::new();
    for round in 0..=ROUNDS {
        work.charge_product(source.outline.segments().len(), 2)?;
        let (error, other_error) = uncertainties(source, &shape, current_area, options);
        let proof = owned_proof.as_ref().or(shared_proof);
        let local = proof.map(|proof| Local {
            proof,
            origin,
            other_error,
        });
        let collected = ambiguous(&shape, error, local, (&mut work, &mut bytes))?.collect(
            source,
            options,
            (&mut targets, &mut proven),
            shape.retained_bytes() + owned_proof.as_ref().map_or(0, Proof::retained_bytes),
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )?;
        if !collected.wrong {
            return Ok(Some(shape));
        }
        if round == 0 {
            shape = collected.with_rows(source, shape, options, &proven, &mut bytes);
            if matches!(&shape.body, Body::Certified(_)) {
                return Ok(Some(shape));
            }
        }
        if round == ROUNDS {
            break;
        }
        let refinement = if proof.is_some() {
            Refinement::Targeted
        } else {
            Refinement::Whole
        };
        let replacement = replacement(
            source,
            shape,
            &targets,
            (options, round),
            refinement,
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        )?;
        let released = owned_proof.as_ref().map_or(0, Proof::retained_bytes);
        drop(owned_proof.take());
        bytes.refund(released);
        shared_proof = None;
        current_area = replacement.area;
        owned_proof = replacement.proof;
        let Some(replacement) = replacement.shape else {
            return Ok(None);
        };
        shape = replacement;
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

fn uncertainties(source: Source<'_>, shape: &Shape, area: f64, options: &Prep<'_>) -> (f64, f64) {
    let base = if analytic(shape) {
        curve::reduction_area(source.outline, source.linear)
    } else {
        area
    };
    let placement = placement_error(source.outline, source.linear, source.origin, shape);
    let clip = options
        .clip
        .and_then(|clip| clip.paths())
        .map_or(0.0, Paths::arithmetic_error);
    (base + placement + clip, placement + clip)
}

fn collect_targets(
    source: Source<'_>,
    pending: Vec<Pixel>,
    (targets, proven): (&mut Vec<[f64; 4]>, &mut Vec<Pixel>),
    options: &Prep<'_>,
    Meters { work, bytes }: Meters<'_>,
) -> Result<bool> {
    // The sorted prefix comes from earlier replacement passes. Inspection
    // visits each pixel once, so new successes can append in query order.
    let known = proven.len();
    let mut wrong = false;
    for pixel in pending {
        work.charge(u32_to_usize(known.max(1).ilog2()) + 2)?;
        let expected = if let Ok(index) =
            proven[..known].binary_search_by_key(&(pixel.y, pixel.x), |p| (p.y, p.x))
        {
            proven[index].byte
        } else {
            let byte = true_byte(source, pixel, options, (work, bytes))?;
            if proven.len() == proven.capacity() {
                work.charge(proven.len())?;
            }
            bytes.reserve_capacity(proven, 1)?;
            proven.push(Pixel { byte, ..pixel });
            byte
        };
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
                bytes.reserve_capacity(targets, 1)?;
                targets.push(target);
            }
        }
    }
    if proven.len() > known {
        work.charge_product(proven.len(), u32_to_usize(proven.len().max(1).ilog2()) + 2)?;
        proven.sort_unstable_by_key(|p| (p.y, p.x));
    }
    Ok(wrong)
}

/// Rebuilds the existing tight mesh after the rejected candidate has left
/// scope. Empty inspection targets keep the global subdivision decisions
/// while charging each subdivision before it is performed.
fn tight_placement(
    source: Source<'_>,
    options: &Prep<'_>,
    Meters { work, bytes }: Meters<'_>,
) -> Result<Option<Shape>> {
    let before = bytes.used();
    let (polygon, points) = flattened_with(
        source.outline,
        source.linear,
        options.limits,
        bytes,
        AREA,
        Some(Inspection {
            scope: Scope::Global,
            hulls: None,
            work,
            rounding: 0.0,
            targets: &[],
            offset: Point::default(),
            factor: 1.0,
        }),
    )?;
    let polygon_bytes = polygon.heap_bytes() + points.capacity() * size_of::<Point>();
    bytes.refund(bytes.used().saturating_sub(before + polygon_bytes));
    let prepared = translated(
        (&polygon, &points),
        source.origin,
        Winding::Check(0.0),
        options,
        bytes,
        Some(work),
        None,
    );
    drop((polygon, points));
    let retained = prepared
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map_or(0, Shape::retained_bytes);
    bytes.refund(bytes.used().saturating_sub(before + retained));
    prepared
}

fn replacement(
    source: Source<'_>,
    shape: Shape,
    targets: &[[f64; 4]],
    (options, round): (&Prep<'_>, i32),
    refinement: Refinement,
    Meters { work, bytes }: Meters<'_>,
) -> Result<Replacement> {
    let before = bytes.used();
    let capture = matches!(refinement, Refinement::Targeted);
    let area = if capture { source.initial_area } else { AREA };
    let mut hulls = Vec::new();
    let (polygon, points) = flattened_with(
        source.outline,
        source.linear,
        options.limits,
        bytes,
        area,
        Some(Inspection {
            scope: Scope::Global,
            hulls: capture.then_some(&mut hulls),
            work,
            rounding: 0.0,
            targets,
            offset: source.origin,
            factor: (AREA / area) * 16.0_f64.powi(-round - 1),
        }),
    )?;
    let polygon_bytes = polygon.heap_bytes() + points.capacity() * size_of::<Point>();
    let hull_bytes = hulls.capacity() * size_of::<Hull>();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before + polygon_bytes + hull_bytes),
    );
    // Old geometry stays charged until its replacement is fully ready.
    let prepared = translated(
        (&polygon, &points),
        source.origin,
        Winding::Unknown,
        options,
        bytes,
        Some(work),
        None,
    )?;
    drop((polygon, points));
    bytes.refund(polygon_bytes);
    // Release row-builder scratch before proof indexing or a guarded tight
    // fallback needs more storage. Old shape and proof still coexist here.
    bytes.refund(
        bytes.used().saturating_sub(
            before + prepared.as_ref().map_or(0, Shape::retained_bytes) + hull_bytes,
        ),
    );
    let (prepared, area) = guard_candidate(
        Source {
            initial_area: area,
            ..source
        },
        prepared,
        options,
        Meters { work, bytes },
    )?;
    let proof = if capture && area > AREA {
        Some(Proof::new(hulls, bytes, work)?)
    } else {
        drop(hulls);
        bytes.refund(hull_bytes);
        None
    };
    if !capture && prepared.is_none() {
        return Err(Error::limit("unresolved curve coverage interval"));
    }
    let released = shape.retained_bytes();
    drop(shape);
    bytes.refund(released);
    Ok(Replacement {
        shape: prepared,
        proof,
        area,
    })
}

/// Translation and row/intersection arithmetic can move a boundary. The
/// per-edge homotopy bound is at most three times the two coordinate
/// displacements inside any pixel. This broad initial bound only selects
/// pixels for the local, original-control certificate below.
fn placement_error(outline: &Path, linear: Transform, origin: Point, shape: &Shape) -> f64 {
    let mut magnitude = origin.x.abs().max(origin.y.abs()).max(1.0);
    for segment in outline.segments() {
        let mut include = |p: Point| {
            magnitude = magnitude
                .max((p.x * linear.a).abs() + (p.y * linear.c).abs() + origin.x.abs())
                .max((p.x * linear.b).abs() + (p.y * linear.d).abs() + origin.y.abs());
        };
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => include(p),
            Segment::CubicTo { c1, c2, end } => {
                include(c1);
                include(c2);
                include(end);
            }
            Segment::Close => {}
        }
    }
    let pieces = f64::from(
        u32::try_from(outline.segments().len().max(shape.boundaries())).unwrap_or(u32::MAX),
    );
    384.0 * f64::EPSILON * magnitude * pieces
}

fn scan_width(shape: &Shape) -> usize {
    match &shape.body {
        Body::Glyph(body) => body.scan.1.saturating_sub(body.scan.0),
        Body::Edges(body) => body.scan.1.saturating_sub(body.scan.0),
        Body::Certified(_) => shape.bounds.width() as usize,
    }
}

fn ambiguous(
    shape: &Shape,
    error: f64,
    local: Option<Local<'_>>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<Ambiguity> {
    let width = scan_width(shape);
    let mut cells = Cells::new(width, bytes)?;
    let cell_bytes = cells.retained_bytes();
    let allowance = shape.work();
    work.charge(allowance)?;
    let mut pending = Vec::new();
    let mut failure = None;
    let observed = raster::inspect(shape, &mut cells, |y, x0, x1, area, arithmetic| {
        if failure.is_some() || certain(area, outward(error + arithmetic)) {
            return;
        }
        let result = (|| {
            let uncertainty = |x0, x1, work: &mut Meter| -> Result<f64> {
                match local {
                    Some(Local {
                        proof,
                        origin,
                        other_error,
                    }) => {
                        let column = |x| {
                            u32::try_from(x)
                                .map(f64::from)
                                .map_err(|_| Error::limit("glyph column exceeds u32"))
                        };
                        Ok(outward(
                            proof.error(
                                [column(x0)?, f64::from(y), column(x1)?, f64::from(y) + 1.0],
                                origin,
                                work,
                            )? + other_error
                                + arithmetic,
                        ))
                    }
                    None => Ok(outward(error + arithmetic)),
                }
            };
            if certain(area, uncertainty(x0, x1, work)?) {
                return Ok(());
            }
            work.charge(x1 - x0)?;
            for x in x0..x1 {
                // A constant run may meet a boundary hull only near one
                // end. Keep its other pixels out of original-control
                // refinement when their own envelopes certify the byte.
                if local.is_some() && x1 - x0 > 1 && certain(area, uncertainty(x, x + 1, work)?) {
                    continue;
                }
                bytes.reserve_capacity(&mut pending, 1)?;
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
    Ok(Ambiguity {
        pixels: pending,
        surplus: observed.surplus(allowance),
    })
}

fn certain(area: f64, error: f64) -> bool {
    // Signed winding can exceed one in the canonical point cell. Clamp
    // before adding uncertainty so saturation cannot hide a source error.
    let center = area.clamp(0.0, 1.0);
    area.is_finite()
        && error.is_finite()
        && quantize_unit(center - error) == quantize_unit(center + error)
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
    let Source {
        outline,
        linear,
        origin,
        points: source_points,
        ..
    } = source;
    let (ox, dx) = subtract(origin.x, f64::from(pixel.x));
    let (oy, dy) = subtract(origin.y, f64::from(pixel.y));
    let local = Transform {
        e: ox,
        f: oy,
        ..linear
    };
    let mut hulls = Vec::new();
    let frame = Frame::new(options).ok_or(Error::limit("unresolved curve coverage interval"))?;
    let bounds = clip_bounds(&frame, pixel);
    let targets = [bounds];
    let before = bytes.used();
    let (mut polygon, local_points) = flattened_with(
        outline,
        local,
        options.limits,
        bytes,
        AREA,
        Some(Inspection {
            scope: Scope::Local,
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
    let (mut pieces, arithmetic) = interval_body(polygon, work, bytes)?;
    let mut error = hull_error(&hulls, bounds);
    let released = hulls.capacity() * size_of::<Hull>();
    drop(hulls);
    bytes.refund(released);
    error = outward(
        error + arithmetic + source_lines(outline, linear, origin, pixel, local, bounds, work)?,
    );
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

/// Produces the original interval edges and their ordered arithmetic bound.
/// A retained polygon may occupy the old line reservation only when its
/// actual heap fits that reservation. The logical charges and their order
/// remain the same as materializing the original lines before the pieces.
fn interval_body(
    polygon: edges::Polygon,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<(Vec<Piece>, f64)> {
    let vertices = polygon.vertices();
    let retained = polygon.heap_bytes();
    let Some(reserved_lines) = vertices.checked_mul(size_of::<Line>()) else {
        return materialized_interval_body(polygon, work, bytes);
    };
    if retained > reserved_lines {
        return materialized_interval_body(polygon, work, bytes);
    }

    // Preserve the original line request before traversing its sides. The
    // paid reservation then covers the retained polygon until it is dropped.
    bytes.charge_items::<Line>(vertices)?;
    let count = polygon.sides().filter(|line| !line.is_horizontal()).count();
    bytes.refund(retained);
    let mut pieces = Vec::new();
    bytes.reserve(&mut pieces, count)?;
    work.charge(count)?;
    let mut arithmetic = 0.0;
    for line in polygon.sides().filter(|line| !line.is_horizontal()) {
        if let Some(piece) = Piece::new(line, 0.0, 0) {
            pieces.push(piece);
        }
        // Lines outside the row still contribute their original error bound.
        arithmetic = outward(
            arithmetic + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]),
        );
    }
    drop(polygon);
    bytes.refund(reserved_lines);
    Ok((pieces, arithmetic))
}

fn materialized_interval_body(
    polygon: edges::Polygon,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<(Vec<Piece>, f64)> {
    let lines = edges::lines(&polygon, bytes)?;
    let released = polygon.heap_bytes();
    drop(polygon);
    bytes.refund(released);
    let mut pieces = Vec::new();
    bytes.reserve(&mut pieces, lines.len())?;
    pieces.extend(lines.iter().filter_map(|&line| Piece::new(line, 0.0, 0)));
    work.charge(lines.len())?;
    let arithmetic = lines.iter().fold(0.0, |sum, &line| {
        outward(sum + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]))
    });
    let released = lines.capacity() * size_of::<Line>();
    drop(lines);
    bytes.refund(released);
    Ok((pieces, arithmetic))
}

fn hull_error(hulls: &[Hull], bounds: [f64; 4]) -> f64 {
    hulls.iter().fold(0.0, |sum, hull| {
        if hull.intersects(bounds) {
            outward(sum + hull.area)
        } else {
            sum
        }
    })
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

fn outward(value: f64) -> f64 {
    value * (1.0 + 16.0 * f64::EPSILON)
}

fn localized_line(line: Line, pixel: Pixel) -> (Line, Point) {
    let shift = |p: Point| {
        let (x, dx) = subtract(p.x, f64::from(pixel.x));
        let (y, dy) = subtract(p.y, f64::from(pixel.y));
        (Point::new(x, y), Point::new(dx.abs(), dy.abs()))
    };
    let (a, da) = shift(line.a);
    let (b, db) = shift(line.b);
    (Line { a, b }, Point::new(da.x.max(db.x), da.y.max(db.y)))
}

fn edge_error(line: Line, delta: Point, bounds: [f64; 4]) -> f64 {
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
            Segment::CubicTo { end, .. } => current = Some(end),
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
mod tests;
