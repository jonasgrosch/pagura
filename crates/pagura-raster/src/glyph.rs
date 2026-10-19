//! Position-independent glyph polygons, with exact per-instance translation.
//!
//! Only the linear transform participates in subdivision. A renderer adds the
//! device origin to each emitted vertex, without rounding either operand to a
//! pixel grid (ADR 0013). Point-contour classification precedes subdivision:
//! a small curved contour can collapse to a point without becoming the
//! geometric point of ISO 32000-2, 8.5.3.3.1.

use crate::coverage::capacity_bytes;
use crate::flatten::{bounded, flatten};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

/// One flattened contour relative to a glyph's device origin.
pub type GlyphContour = crate::flatten::Polyline;

impl GlyphContour {
    /// Vertices after the linear transform, before instance translation.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Whether the source explicitly closes this contour.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether all linearly mapped source controls coincide before subdivision.
    /// A fill gives this contour its translated point's floor cell (8.5.3.3.1).
    pub fn is_degenerate(&self) -> bool {
        self.degenerate
    }
}

/// A glyph polygon determined solely by its outline and device linear map.
///
/// No coverage is retained. Each placement still needs its own rasterization
/// and mark in content order (9.3.6, 9.3.8; ADR 0013).
#[derive(Debug, PartialEq)]
pub struct GlyphGeometry {
    contours: Vec<GlyphContour>,
    vertices: usize,
    bounds: Option<[Point; 2]>,
}

impl GlyphGeometry {
    /// Subdivides an outline under the linear part of `linear`; its
    /// translation is ignored, because each placement supplies its own
    /// origin ([`Rasterizer::clipped_glyph`](crate::Rasterizer::clipped_glyph)).
    ///
    /// Uses the shared flattener at the unchanged 1/256-device-pixel tolerance.
    /// The instance translation never participates in subdivision.
    /// Limits apply equally when this computation precedes cache admission.
    /// `max_bytes` bounds [`Self::retained_bytes`]; a cache passes its
    /// remaining allowance.
    ///
    /// # Errors
    /// Returns invalid geometry, excessive coordinates, segment, depth or
    /// byte limits.
    pub fn new(
        outline: &Path,
        linear: Transform,
        limits: &Limits,
        max_bytes: usize,
    ) -> Result<Self> {
        let transform = Transform {
            e: 0.0,
            f: 0.0,
            ..linear
        };
        if !transform.is_finite() {
            return Err(Error::invalid_argument("glyph linear map is not finite"));
        }
        if max_bytes < size_of::<Self>() {
            return Err(Error::limit("glyph geometry byte budget exceeded"));
        }
        let mut contours = flatten(outline, transform, limits)?;
        classify(outline, transform, &mut contours)?;
        let mut vertices = 0;
        for contour in &mut contours {
            vertices += contour.points.len();
            for point in &mut contour.points {
                *point = *point * transform;
                bounded(*point)?;
            }
        }
        if vertices > limits.path_segments {
            return Err(Error::limit("flattened segment budget exceeded"));
        }
        let mut result = Self {
            contours,
            vertices,
            bounds: None,
        };
        if result.retained_bytes() > max_bytes {
            return Err(Error::limit("glyph geometry byte budget exceeded"));
        }
        result.bounds = bounds(&result.contours);
        Ok(result)
    }

    /// Relative device bounds, including point contours.
    pub fn bounds(&self) -> Option<[Point; 2]> {
        self.bounds
    }

    /// Contours in source order, including trailing open point contours.
    /// A raster consumer omits a final open one-point contour as for paths.
    pub fn contours(&self) -> &[GlyphContour] {
        &self.contours
    }

    /// Vertex count, charged for every placement even after a cache hit.
    pub fn vertices(&self) -> usize {
        self.vertices
    }

    /// Capacity-based estimate of owned storage, with saturating arithmetic.
    /// Shared outline ownership and a cache index need separate accounting.
    pub fn retained_bytes(&self) -> usize {
        self.contours.iter().fold(
            size_of::<Self>().saturating_add(capacity_bytes(&self.contours)),
            |bytes, contour| bytes.saturating_add(capacity_bytes(&contour.points)),
        )
    }
}

fn bounds(contours: &[GlyphContour]) -> Option<[Point; 2]> {
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in contours.iter().flat_map(|c| &c.points) {
        lo.x = lo.x.min(point.x);
        lo.y = lo.y.min(point.y);
        hi.x = hi.x.max(point.x);
        hi.y = hi.y.max(point.y);
    }
    (lo.x <= hi.x && lo.y <= hi.y).then_some([lo, hi])
}

// The shared flattener retains source point classification. A singular linear
// map can additionally collapse all source controls to one genuine point.
// Classify before mapping the emitted vertices, so approximation never creates
// a geometric point from a nonpoint curve (8.5.3.3.1).
fn classify(outline: &Path, transform: Transform, contours: &mut [GlyphContour]) -> Result<()> {
    let mut index = 0;
    let mut occupied = false;
    let mut closed = false;
    for segment in outline.segments() {
        if (closed && matches!(segment, Segment::LineTo(_) | Segment::CubicTo { .. }))
            || (occupied && matches!(segment, Segment::MoveTo(_)))
        {
            index += 1;
            occupied = false;
            closed = false;
        }
        let contour = contours
            .get_mut(index)
            .ok_or(Error::invalid_argument("invalid flattened contour"))?;
        if !occupied {
            contour.degenerate = true;
        }
        let first = contour.points.first().copied().unwrap_or_default() * transform;
        let same = |point: Point| point * transform == first;
        match *segment {
            Segment::MoveTo(_) => {
                occupied = true;
                closed = false;
            }
            Segment::LineTo(point) => {
                contour.degenerate &= same(point);
                occupied = true;
            }
            Segment::CubicTo { c1, c2, end } => {
                contour.degenerate &= [c1, c2, end].into_iter().all(same);
                occupied = true;
            }
            Segment::Close => closed = true,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    /// A cache allowance as large as the default decoded-stream cap.
    const BUDGET: usize = 256 << 20;

    fn linear([a, b, c, d]: [f64; 4]) -> Transform {
        Transform::new(a, b, c, d, 0.0, 0.0)
    }

    fn tiny_curve() -> Path {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(4.0, 4.0));
        path.cubic_to(
            Point::new(4.001, 4.0),
            Point::new(4.0, 4.001),
            Point::new(4.0, 4.0),
        )
        .unwrap();
        path.close();
        path.finish()
    }

    #[test]
    fn collapsed_curve_is_not_a_geometric_point() {
        let geometry = GlyphGeometry::new(
            &tiny_curve(),
            Transform::IDENTITY,
            &Limits::default(),
            BUDGET,
        )
        .unwrap();
        let contour = &geometry.contours()[0];
        assert_eq!(contour.points(), &[Point::new(4.0, 4.0); 2]);
        assert!(!contour.is_degenerate());
        assert!(contour.is_closed());
        assert_eq!(geometry.vertices(), 2);
    }

    #[test]
    fn collapsed_curve_requires_contour_metadata_at_rasterization() {
        let path = tiny_curve();
        let geometry =
            GlyphGeometry::new(&path, Transform::IDENTITY, &Limits::default(), BUDGET).unwrap();
        let mut polygon = PathBuilder::new();
        for contour in geometry.contours() {
            let mut points = contour.points().iter();
            polygon.move_to(*points.next().unwrap());
            for point in points {
                polygon.line_to(*point).unwrap();
            }
            if contour.is_closed() {
                polygon.close();
            }
        }
        let rasterize = |path: &Path| {
            crate::Mask::from_path(
                crate::IntRect::new(0, 0, 8, 8).unwrap(),
                path.clone(),
                Transform::IDENTITY,
                crate::FillRule::NonZero,
                Limits {
                    pixels: 64,
                    ..Limits::default()
                },
            )
            .unwrap()
        };
        // The tiny curve's area rounds to zero. Reconstructing only its
        // polygon vertices loses that fact and creates a floor-cell mark.
        assert_eq!(rasterize(&path).coverage()[4 * 8 + 4], 0);
        assert_eq!(rasterize(&polygon.finish()).coverage()[4 * 8 + 4], 255);
        let mut viewport = PathBuilder::new();
        viewport.rect(0.0, 0.0, 8.0, 8.0);
        let clip = rasterize(&viewport.finish());
        let mask = crate::Rasterizer::default()
            .clipped_glyph(
                &clip,
                std::sync::Arc::new(geometry),
                Point::default(),
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert!(mask.coverage().iter().all(|&value| value == 0));
    }

    #[test]
    fn point_classification_precedes_subdivision_under_a_singular_map() {
        let mut path = PathBuilder::new();
        path.rect(1.0, 1.0, 2.0, 2.0);
        let geometry =
            GlyphGeometry::new(&path.finish(), linear([0.0; 4]), &Limits::default(), BUDGET)
                .unwrap();
        assert!(
            geometry.contours()[0]
                .points()
                .iter()
                .all(|p| *p == Point::default())
        );
        assert!(geometry.contours()[0].is_degenerate());
    }

    #[test]
    fn a_line_after_close_reopens_at_the_source_start() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 2.0));
        path.line_to(Point::new(3.0, 4.0)).unwrap();
        path.close();
        path.line_to(Point::new(5.0, 6.0)).unwrap();
        let geometry = GlyphGeometry::new(
            &path.finish(),
            linear([0.0, 2.0, -3.0, 0.0]),
            &Limits::default(),
            BUDGET,
        )
        .unwrap();
        assert_eq!(geometry.contours().len(), 2);
        assert_eq!(
            geometry.contours()[1].points(),
            &[Point::new(-6.0, 2.0), Point::new(-18.0, 10.0)]
        );
        assert!(!geometry.contours()[1].is_closed());
    }

    #[test]
    fn segment_and_numeric_limits_are_checked_before_admission() {
        let path = tiny_curve();
        let limits = Limits {
            path_segments: 1,
            ..Limits::default()
        };
        assert_eq!(
            GlyphGeometry::new(&path, Transform::IDENTITY, &limits, BUDGET)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        // Scaled up, the same curve needs more than eight flattened segments.
        let limits = Limits {
            path_segments: 8,
            ..Limits::default()
        };
        assert_eq!(
            GlyphGeometry::new(&path, linear([1e6, 0.0, 0.0, 1e6]), &limits, BUDGET)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        assert_eq!(
            GlyphGeometry::new(&path, linear([f64::NAN, 0.0, 0.0, 1.0]), &limits, BUDGET)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::InvalidArgument
        );
        assert_eq!(
            GlyphGeometry::new(&path, linear([1e12, 0.0, 0.0, 1e12]), &limits, BUDGET)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
    }

    #[test]
    fn curved_polygon_follows_the_analytic_curve_within_tolerance() {
        // B(t) = (3t² - 2t³, 3t(1-t)). Check its analytic samples without
        // prescribing the shared flattener's algorithm or vertex parameters.
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(0.0, 1.0),
            Point::new(1.0, 1.0),
            Point::new(1.0, 0.0),
        )
        .unwrap();
        let path = path.finish();
        let scale = 1.0 / 32.0;
        for map in [
            [scale, 0.0, 0.0, scale],
            [-scale, 0.0, 0.0, scale],
            [0.0, scale, -scale, 0.0],
        ] {
            let geometry =
                GlyphGeometry::new(&path, linear(map), &Limits::default(), BUDGET).unwrap();
            let [a, b, c, d] = map;
            let vertices = geometry.contours()[0].points();
            assert_eq!(vertices.first(), Some(&Point::default()));
            assert_eq!(vertices.last(), Some(&Point::new(a, b)));
            assert!(!geometry.contours()[0].is_degenerate());
            for step in 0..=1024 {
                let t = f64::from(step) / 1024.0;
                let x = t * t * (3.0 - 2.0 * t);
                let y = 3.0 * t * (1.0 - t);
                let point = Point::new(a * x + c * y, b * x + d * y);
                let error = vertices
                    .windows(2)
                    .map(|pair| {
                        let dx = pair[1].x - pair[0].x;
                        let dy = pair[1].y - pair[0].y;
                        let square = dx * dx + dy * dy;
                        let u = if square == 0.0 {
                            0.0
                        } else {
                            (((point.x - pair[0].x) * dx + (point.y - pair[0].y) * dy) / square)
                                .clamp(0.0, 1.0)
                        };
                        (point.x - pair[0].x - u * dx).hypot(point.y - pair[0].y - u * dy)
                    })
                    .fold(f64::INFINITY, f64::min);
                assert!(error <= crate::flatten::TOLERANCE);
            }
        }
    }

    #[test]
    fn point_and_trailing_open_contours_remain_distinguishable() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.25, 2.5));
        path.line_to(Point::new(1.25, 2.5)).unwrap();
        path.close();
        path.move_to(Point::new(3.0, 4.0));
        let geometry = GlyphGeometry::new(
            &path.finish(),
            Transform::IDENTITY,
            &Limits::default(),
            BUDGET,
        )
        .unwrap();
        assert_eq!(geometry.contours().len(), 2);
        assert!(geometry.contours()[0].is_degenerate());
        assert!(geometry.contours()[0].is_closed());
        assert!(geometry.contours()[1].is_degenerate());
        assert!(!geometry.contours()[1].is_closed());
        assert_eq!(geometry.contours()[1].points(), &[Point::new(3.0, 4.0)]);
    }

    #[test]
    fn aggregate_contour_capacities_obey_the_byte_budget() {
        let mut path = PathBuilder::new();
        for x in 0..100 {
            path.rect(f64::from(x), 0.0, 0.5, 0.5);
        }
        let path = path.finish();
        assert_eq!(
            GlyphGeometry::new(&path, Transform::IDENTITY, &Limits::default(), 1024)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        let geometry =
            GlyphGeometry::new(&path, Transform::IDENTITY, &Limits::default(), BUDGET).unwrap();
        assert!(geometry.retained_bytes() <= BUDGET);
        assert!(geometry.retained_bytes() >= 400 * size_of::<Point>());
    }

    #[test]
    fn flattened_vertices_and_empty_geometry_respect_limits() {
        let path = tiny_curve();
        let limits = Limits {
            path_segments: 3,
            ..Limits::default()
        };
        assert_eq!(
            GlyphGeometry::new(&path, linear([1e6, 0.0, 0.0, 1e6]), &limits, BUDGET)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        assert_eq!(
            GlyphGeometry::new(&Path::default(), Transform::IDENTITY, &Limits::default(), 0)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        let geometry = GlyphGeometry::new(
            &Path::default(),
            Transform::IDENTITY,
            &Limits::default(),
            BUDGET,
        )
        .unwrap();
        assert_eq!(geometry.contours(), []);
        assert_eq!(geometry.vertices(), 0);
    }
    #[test]
    fn translated_point_intersections_and_spans_keep_geometric_coverage() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.25, 0.25));
        path.close();
        let glyph = std::sync::Arc::new(
            GlyphGeometry::new(
                &path.finish(),
                Transform::IDENTITY,
                &Limits::default(),
                BUDGET,
            )
            .unwrap(),
        );
        let mut path = PathBuilder::new();
        path.rect(10.5, 20.5, 1.0, 1.0);
        let clip = crate::Mask::from_path(
            crate::IntRect::new(10, 20, 2, 2).unwrap(),
            path.finish(),
            Transform::IDENTITY,
            crate::FillRule::NonZero,
            Limits {
                pixels: 4,
                ..Limits::default()
            },
        )
        .unwrap();
        let mask = crate::Rasterizer::default()
            .clipped_glyph(&clip, glyph, Point::new(10.5, 20.5), Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(mask.coverage(), [64]);
        assert_eq!(mask.area(), crate::IntRect::new(10, 20, 1, 1).unwrap());
        let spans: Vec<_> = mask
            .spans()
            .map(|span| (span.row(), span.column(), span.coverage()))
            .collect();
        assert_eq!(spans, [(0, 0, &[64][..])]);
    }
}
