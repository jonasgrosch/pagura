//! Joint true-control coverage for glyph clipping sources. Preparation uses
//! the ordinary localized row resolver and cell integrator, refining all
//! curved sources together until both area-interval endpoints give one byte.
//! The finished runs use the same representation as ordinary clip rows.

use super::cells::Cells;
use super::clip::{Rows, Segment};
use super::edges::{self, Bounds, Line, Polygon, StripList};
use super::glyph::rounding::{self, Pixel};
use super::shape::{Body, EdgeBody, Resolved, Shape};
use super::union::{Meters, Piece, Resolver};
use super::{DeviceRect, GlyphEdges, Prep, RASTER_WORK, RENDER_BYTES};
use crate::{FillRule, Path, Point, Transform, quantize_unit};
use pagura_core::{Error, Meter, Result};

/// One clip input, preserving the distinction between a canonical path fill
/// and a glyph whose original curves determine coverage (8.5.4, 9.3.6).
#[derive(Debug, Clone, Copy)]
pub struct ClipSource<'a> {
    pub(super) path: &'a Path,
    pub(super) rule: FillRule,
    pub(super) transform: Transform,
    pub(super) kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Fill,
    Glyph,
}

impl<'a> ClipSource<'a> {
    /// A path fill, with the ordinary canonical flattened-path semantics.
    pub fn fill(path: &'a Path, rule: FillRule, transform: Transform) -> Self {
        Self {
            path,
            rule,
            transform,
            kind: Kind::Fill,
        }
    }

    /// A glyph outline whose true curves and geometric points determine
    /// coverage under its complete device transform.
    pub fn glyph(path: &'a Path, transform: Transform) -> Self {
        Self {
            path,
            rule: FillRule::NonZero,
            transform,
            kind: Kind::Glyph,
        }
    }
}

#[derive(Debug)]
pub(super) struct Source {
    geometry: Geometry,
    hull: Option<Bounds>,
}

#[derive(Debug)]
enum Geometry {
    Lines {
        lines: Vec<Line>,
        rule: FillRule,
    },
    Glyph {
        path: Path,
        linear: Transform,
        origin: Point,
        points: Vec<Point>,
        paths: Box<super::clip::Paths>,
        hulls: Vec<crate::flatten::Hull>,
        strips: StripList,
    },
}

struct Outlined {
    path: Path,
    linear: Transform,
    origin: Point,
    points: Vec<Point>,
    polygon: Polygon,
    hulls: Vec<crate::flatten::Hull>,
    hull: Option<Bounds>,
}

#[derive(Clone, Copy)]
enum Borrowed<'a> {
    Lines(&'a [Line], FillRule),
    Canonical(&'a super::clip::Paths),
    Glyph {
        source: rounding::Source<'a>,
        paths: &'a super::clip::Paths,
        hulls: &'a [crate::flatten::Hull],
        strips: &'a StripList,
    },
}

impl Source {
    pub(super) fn new(
        source: ClipSource<'_>,
        options: &Prep<'_>,
        bytes: &mut Meter,
        work: &mut Meter,
    ) -> Result<Self> {
        work.charge_product(source.path.segments().len(), 2)?;
        if source.kind == Kind::Fill {
            let (polygon, rule) = super::clip::fill_polygon(
                source.path,
                source.transform,
                source.rule,
                options.limits,
                bytes,
            )?;
            return Self::polygon(polygon, rule, bytes);
        }
        if source.path.segments().len() > options.limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        if !source.transform.is_finite() {
            return Err(Error::invalid_argument("glyph transform is not finite"));
        }
        let linear = Transform {
            e: 0.0,
            f: 0.0,
            ..source.transform
        };
        let origin = Point::new(source.transform.e, source.transform.f);
        let path = source.path.clone_with_bytes(bytes)?;
        let before = bytes.used();
        let targets = [[-f64::MAX, -f64::MAX, f64::MAX, f64::MAX]];
        let mut hulls = Vec::new();
        let true_curves =
            linear.determinant() != 0.0 && super::curve::quadratic_outline(&path, linear);
        let (mut polygon, points) = super::glyph::flattened_with(
            &path,
            linear,
            options.limits,
            bytes,
            true_curves.then_some(crate::flatten::Inspection {
                hulls: Some(&mut hulls),
                work,
                rounding: 0.0,
                targets: &targets,
                offset: Point::default(),
                factor: 1.0,
            }),
        )?;
        let kept = polygon
            .heap_bytes()
            .saturating_add(points.capacity().saturating_mul(size_of::<Point>()))
            .saturating_add(
                hulls
                    .capacity()
                    .saturating_mul(size_of::<crate::flatten::Hull>()),
            );
        bytes.refund(bytes.used().saturating_sub(before.saturating_add(kept)));
        let hull = glyph_hull(&path, linear, origin, &points);
        validate_hull(hull)?;
        work.charge(polygon.points.len().saturating_add(points.len()))?;
        for p in &mut polygon.points {
            *p += origin;
        }
        for &p in &points {
            let p = p + origin;
            polygon.cell(Point::new(p.x.floor(), p.y.floor()), options.limits, bytes)?;
        }
        if !true_curves {
            let lines = edges::lines(&polygon, bytes)?;
            let released = polygon
                .heap_bytes()
                .saturating_add(path.retained_bytes())
                .saturating_add(points.capacity().saturating_mul(size_of::<Point>()));
            drop((polygon, path, points));
            bytes.refund(released);
            return Ok(Self {
                geometry: Geometry::Lines {
                    lines,
                    rule: FillRule::NonZero,
                },
                hull,
            });
        }
        Self::finish(
            Outlined {
                path,
                linear,
                origin,
                points,
                polygon,
                hulls,
                hull,
            },
            options,
            bytes,
            work,
        )
    }

    fn finish(
        outline: Outlined,
        options: &Prep<'_>,
        bytes: &mut Meter,
        work: &mut Meter,
    ) -> Result<Self> {
        let Outlined {
            path,
            linear,
            origin,
            points,
            polygon,
            mut hulls,
            hull,
        } = outline;
        work.charge_product(hulls.len(), 2)?;
        for hull in &mut hulls {
            let margin = 16.0 * f64::EPSILON * (1.0 + origin.x.abs() + origin.y.abs());
            hull.bounds = [
                (hull.bounds[0] + origin.x - margin).next_down(),
                (hull.bounds[1] + origin.y - margin).next_down(),
                (hull.bounds[2] + origin.x + margin).next_up(),
                (hull.bounds[3] + origin.y + margin).next_up(),
            ];
        }
        let mut hull = hull;
        for h in &hulls {
            let b = Bounds {
                left: h.bounds[0],
                top: h.bounds[1],
                right: h.bounds[2],
                bottom: h.bounds[3],
            };
            hull = Some(hull.map_or(b, |a| Bounds {
                left: a.left.min(b.left),
                top: a.top.min(b.top),
                right: a.right.max(b.right),
                bottom: a.bottom.max(b.bottom),
            }));
        }
        let bounds = hull.map_or_else(DeviceRect::default, |h| h.pixels(options.area.region));
        let before = bytes.used();
        let groups = [(polygon, FillRule::NonZero)];
        let paths = super::clip::classify_metered(&groups, None, bounds, bytes, work)?;
        let released = groups[0].0.heap_bytes();
        drop(groups);
        bytes.refund(released);
        bytes.refund(
            bytes.used().saturating_sub(
                before
                    .saturating_sub(released)
                    .saturating_add(paths.retained_bytes()),
            ),
        );
        work.charge_product(
            hulls.len(),
            (bounds.height() as usize / super::cells::STRIP as usize + 2)
                .saturating_mul(2)
                .saturating_add(2),
        )?;
        let strips = StripList::new(
            (bounds.y0, bounds.y1),
            hulls.len(),
            hulls
                .iter()
                .enumerate()
                .filter_map(|(i, h)| edges::rows(h.bounds[1], h.bounds[3], bounds).map(|r| (i, r))),
            bytes,
        )?;
        bytes.charge(size_of::<super::clip::Paths>())?;
        Ok(Self {
            geometry: Geometry::Glyph {
                path,
                linear,
                origin,
                points,
                paths: Box::new(paths),
                hulls,
                strips,
            },
            hull,
        })
    }

    fn polygon(polygon: Polygon, rule: FillRule, bytes: &mut Meter) -> Result<Self> {
        let hull = polygon.bounds().map(|[a, b]| Bounds {
            left: a.x,
            top: a.y,
            right: b.x,
            bottom: b.y,
        });
        let lines = edges::lines(&polygon, bytes)?;
        let released = polygon.heap_bytes();
        drop(polygon);
        bytes.refund(released);
        Ok(Self {
            geometry: Geometry::Lines { lines, rule },
            hull,
        })
    }

    pub(super) fn retained_bytes(&self) -> usize {
        match &self.geometry {
            Geometry::Lines { lines, .. } => lines.capacity().saturating_mul(size_of::<Line>()),
            Geometry::Glyph {
                path,
                points,
                paths,
                hulls,
                strips,
                ..
            } => path
                .retained_bytes()
                .saturating_add(points.capacity().saturating_mul(size_of::<Point>()))
                .saturating_add(paths.retained_bytes())
                .saturating_add(size_of::<super::clip::Paths>())
                .saturating_add(strips.retained_bytes())
                .saturating_add(
                    hulls
                        .capacity()
                        .saturating_mul(size_of::<crate::flatten::Hull>()),
                ),
        }
    }
    fn borrowed(&self) -> Borrowed<'_> {
        match &self.geometry {
            Geometry::Lines { lines, rule } => Borrowed::Lines(lines, *rule),
            Geometry::Glyph {
                path,
                linear,
                origin,
                points,
                paths,
                hulls,
                strips,
            } => Borrowed::Glyph {
                source: rounding::Source {
                    outline: path,
                    linear: *linear,
                    origin: *origin,
                    points,
                },
                paths,
                hulls,
                strips,
            },
        }
    }
}

fn glyph_hull(path: &Path, linear: Transform, origin: Point, points: &[Point]) -> Option<Bounds> {
    let hull = super::glyph::control_hull(path, linear)
        .or_else(|| points.first().map(|p| [p.x, p.y, p.x, p.y]))?;
    let mut hull = Bounds {
        left: hull[0] + origin.x,
        top: hull[1] + origin.y,
        right: hull[2] + origin.x,
        bottom: hull[3] + origin.y,
    };
    for &point in points {
        let p = point + origin;
        hull.left = hull.left.min(p.x.floor());
        hull.top = hull.top.min(p.y.floor());
        hull.right = hull.right.max(p.x.floor() + 1.0);
        hull.bottom = hull.bottom.max(p.y.floor() + 1.0);
    }
    Some(hull)
}

pub(super) fn rows(
    sources: &[Source],
    options: &Prep<'_>,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<(DeviceRect, Rows)> {
    let bounds = source_bounds(sources, options.area.region);
    prepare(sources, None, bounds, options, work, bytes)
}

fn source_bounds(sources: &[Source], region: DeviceRect) -> DeviceRect {
    sources.iter().fold(region, |bounds, source| {
        bounds.intersect(
            &source
                .hull
                .map_or_else(DeviceRect::default, |h| h.pixels(region)),
        )
    })
}

pub(super) fn polygon(
    polygon: Polygon,
    rule: FillRule,
    options: &Prep<'_>,
    bytes: &mut Meter,
    work: &mut Meter,
) -> Result<Option<Shape>> {
    let source = Source::polygon(polygon, rule, bytes)?;
    let result = shape(source.borrowed(), source.hull, options, bytes, work);
    let released = source.retained_bytes();
    drop(source);
    bytes.refund(released);
    result
}

pub(super) fn glyph(
    edges: &GlyphEdges,
    origin: Point,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    crate::flatten::bounded(origin)?;
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    if let Some(proof) = edges.source() {
        let source = Source::new(
            ClipSource::glyph(
                &proof.outline,
                Transform {
                    e: origin.x,
                    f: origin.y,
                    ..proof.linear
                },
            ),
            options,
            &mut bytes,
            &mut work,
        )?;
        let result = shape(
            source.borrowed(),
            source.hull,
            options,
            &mut bytes,
            &mut work,
        );
        let released = source.retained_bytes();
        drop(source);
        bytes.refund(released);
        return result;
    }

    let super::glyph::Kind::Polygon {
        polygon, points, ..
    } = &edges.kind
    else {
        return Err(Error::limit("glyph coverage source unavailable"));
    };
    let view = super::glyph::PolygonLines::new(polygon, points, origin);
    let mut lines = Vec::new();
    for (_, line) in view.indexed() {
        bytes.reserve_capacity(&mut lines, 1)?;
        lines.push(line);
    }
    let hull = edges.hull().map(|h| Bounds {
        left: h[0] + origin.x,
        top: h[1] + origin.y,
        right: h[2] + origin.x,
        bottom: h[3] + origin.y,
    });
    let hull = lines.iter().flat_map(|l| [l.a, l.b]).fold(hull, |h, p| {
        Some(h.map_or(
            Bounds {
                left: p.x,
                top: p.y,
                right: p.x,
                bottom: p.y,
            },
            |h| Bounds {
                left: h.left.min(p.x),
                top: h.top.min(p.y),
                right: h.right.max(p.x),
                bottom: h.bottom.max(p.y),
            },
        ))
    });
    validate_hull(hull)?;
    shape(
        Borrowed::Lines(&lines, FillRule::NonZero),
        hull,
        options,
        &mut bytes,
        &mut work,
    )
}

fn validate_hull(hull: Option<Bounds>) -> Result<()> {
    if let Some(hull) = hull {
        crate::flatten::bounded(Point::new(hull.left, hull.top))?;
        crate::flatten::bounded(Point::new(hull.right, hull.bottom))?;
    }
    Ok(())
}

fn shape(
    leaf: Borrowed<'_>,
    hull: Option<Bounds>,
    options: &Prep<'_>,
    bytes: &mut Meter,
    work: &mut Meter,
) -> Result<Option<Shape>> {
    let Some(clip) = options.clip else {
        return Err(Error::invalid_argument("curved clipping source is absent"));
    };
    let sources = clip.sources();
    let region = options.area.region;
    let bounds = source_bounds(sources, region)
        .intersect(&hull.map_or_else(DeviceRect::default, |h| h.pixels(region)));
    let (bounds, coverage) = prepare(sources, Some(leaf), bounds, options, work, bytes)?;
    if bounds.is_empty() {
        return Ok(None);
    }
    bytes.charge(size_of::<Shape>())?;
    let retained = size_of::<Shape>().saturating_add(coverage.retained_bytes());
    let emitted = coverage
        .len()
        .saturating_add((bounds.height() as usize).saturating_mul(2));
    work.charge(emitted)?;
    Ok(Some(Shape {
        area: options.area,
        bounds,
        work: work.used(),
        retained,
        rule: FillRule::NonZero,
        body: Body::Edges(EdgeBody::default()),
        clip: Some(clip.clone()),
        masked: false,
        resolved: Resolved::default(),
        certified: Some(coverage),
    }))
}

fn prepare(
    sources: &[Source],
    leaf: Option<Borrowed<'_>>,
    bounds: DeviceRect,
    options: &Prep<'_>,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<(DeviceRect, Rows)> {
    let mut rows = Rows::default();
    let count = sources.len().saturating_add(usize::from(leaf.is_some()));
    if count > usize::from(u8::MAX) + 1 {
        return Err(Error::limit("clip path count exceeds u8"));
    }
    for y in bounds.y0..bounds.y1 {
        work.charge(bounds.width() as usize)?;
        for x in bounds.x0..bounds.x1 {
            let pixel = Pixel { x, y, byte: 0 };
            let value = byte(sources, leaf, pixel, options, work, bytes)?;
            if value != 0 {
                rows.push(
                    y,
                    Segment {
                        x0: x,
                        x1: x + 1,
                        value,
                        inside: false,
                    },
                    bytes,
                )?;
            }
        }
    }
    Ok((bounds, rows))
}

fn byte(
    sources: &[Source],
    leaf: Option<Borrowed<'_>>,
    pixel: Pixel,
    options: &Prep<'_>,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<u8> {
    for round in 0..=12 {
        let before = bytes.used();
        let result = interval(sources, leaf, pixel, (options, round), work, bytes);
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
    sources: &[Source],
    leaf: Option<Borrowed<'_>>,
    pixel: Pixel,
    (options, round): (&Prep<'_>, i32),
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<(f64, f64)> {
    let mut pieces = Vec::new();
    let mut rules = Vec::new();
    let mut error = 0.0;
    let mut active_curve = false;
    work.charge(sources.len().saturating_add(usize::from(leaf.is_some())))?;
    for source in sources.iter().map(Source::borrowed).chain(leaf) {
        let source = if let Borrowed::Glyph {
            paths,
            hulls,
            strips,
            ..
        } = source
        {
            match constant(paths, hulls, strips, pixel, work)? {
                State::Inside => continue,
                State::Outside => return Ok((0.0, 0.0)),
                State::Linear => Borrowed::Canonical(paths),
                State::Curved => {
                    active_curve = true;
                    source
                }
            }
        } else {
            source
        };
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        bytes.reserve_capacity(&mut rules, 1)?;
        match source {
            Borrowed::Glyph { source, .. } => {
                rules.push(FillRule::NonZero);
                error = rounding::outward(
                    error
                        + rounding::source_pieces(
                            source,
                            pixel,
                            (options, round),
                            ([0.0, 0.0, 1.0, 1.0], group),
                            &mut pieces,
                            (work, bytes),
                        )?,
                );
            }
            Borrowed::Canonical(paths) => {
                rules.push(FillRule::NonZero);
                let start = pieces.len();
                paths.pieces(pixel.y, group, (&mut pieces, bytes), work)?;
                work.charge(pieces.len().saturating_sub(start))?;
                for piece in pieces.get_mut(start..).unwrap_or_default() {
                    let (line, tail) = rounding::localized_line(piece.line, pixel);
                    error = rounding::outward(
                        error
                            + rounding::edge_error(line, tail, [0.0, 0.0, 1.0, 1.0])
                            + rounding::edge_error(
                                line,
                                line.interpolation_error(),
                                [0.0, 0.0, 1.0, 1.0],
                            ),
                    );
                    *piece = Piece::new(line, 0.0, group)
                        .ok_or(Error::limit("unresolved clip localization interval"))?;
                }
            }
            Borrowed::Lines(lines, rule) => {
                rules.push(rule);
                work.charge(lines.len())?;
                for &line in lines {
                    let (line, tail) = rounding::localized_line(line, pixel);
                    error = rounding::outward(
                        error
                            + rounding::edge_error(line, tail, [0.0, 0.0, 1.0, 1.0])
                            + rounding::edge_error(
                                line,
                                line.interpolation_error(),
                                [0.0, 0.0, 1.0, 1.0],
                            ),
                    );
                    if let Some(piece) = Piece::new(line, 0.0, group) {
                        bytes.reserve_capacity(&mut pieces, 1)?;
                        pieces.push(piece);
                    }
                }
            }
        }
    }
    if rules.is_empty() {
        return Ok((1.0, 0.0));
    }
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
    Ok((
        area,
        if active_curve {
            rounding::outward(error + cells.arithmetic_error())
        } else {
            0.0
        },
    ))
}

enum State {
    Inside,
    Outside,
    Linear,
    Curved,
}

/// A source whose original boundary misses the open pixel is constant there.
/// The retained curve hulls contain both the true pieces and their chords;
/// the polygon winding therefore proves the same covered state.
fn constant(
    paths: &super::clip::Paths,
    hulls: &[crate::flatten::Hull],
    strips: &StripList,
    pixel: Pixel,
    work: &mut Meter,
) -> Result<State> {
    let listed = strips.strip(pixel.y / super::cells::STRIP);
    let row = paths.rows.row(pixel.y);
    work.charge(listed.len().saturating_add(row.len()).saturating_add(1))?;
    let (x, y) = (f64::from(pixel.x), f64::from(pixel.y));
    let bounds = [x, y, x + 1.0, y + 1.0];
    if listed
        .iter()
        .any(|&i| hulls.get(i as usize).is_none_or(|h| h.intersects(bounds)))
    {
        return Ok(State::Curved);
    }
    Ok(
        match row.iter().find(|s| s.x0 <= pixel.x && s.x1 > pixel.x) {
            Some(s) if s.inside => State::Inside,
            Some(_) => State::Linear,
            None => State::Outside,
        },
    )
}
