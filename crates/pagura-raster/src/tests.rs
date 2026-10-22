//! The band rasterizer against the independent generated vectors
//! (`corpus/generated/raster`), and stroke and fill invariants.

use crate::testing::{fill, raster, stroke as band_stroke, window};
use crate::{
    Adjustment, Area, ClipShape, DeviceRect, FillRule, LineCap, LineJoin, Path, PathBuilder, Point,
    Precision, Shape, ShapeOptions, Stroke, Transform,
};
use pagura_core::{ErrorKind, Limits};

/// One vector: the path, its fill rule, its stroke if it is stroked, its
/// clipping paths and the budgets it is rasterized under.
struct Vector {
    path: Path,
    rule: FillRule,
    stroke: Option<Stroke>,
    clips: Vec<Path>,
    limits: Limits,
}

impl Vector {
    fn read(name: &str) -> Self {
        let text =
            String::from_utf8(pagura_testkit::fixture("raster", &format!("{name}.path"))).unwrap();
        let (path, rule, stroke, clips) = vector_geometry(&text);
        // The resolver's work bound for hundreds of coincident edges in one
        // cell exceeds the default raster work budget.
        let raster_work = if name.starts_with("quad-union") {
            usize::MAX
        } else {
            Limits::default().raster_work
        };
        Self {
            path,
            rule,
            stroke,
            clips,
            limits: Limits {
                raster_work,
                ..Limits::default()
            },
        }
    }

    /// The fixture's expected coverage of ordinary scan conversion, or, for
    /// a fill whose region is empty with a boundary, the coverage of its
    /// hairline outline, which every fill paints there (10.7.4).
    fn expected(&self, ordinary: &[u8]) -> Vec<u8> {
        let mut classified = self.path.clone();
        classified
            .classify_fill(self.rule, &self.limits, &mut 0)
            .unwrap();
        if self.stroke.is_some()
            || classified.fill_region(self.rule) != Some(crate::FillRegion::EmptyBoundary)
        {
            return ordinary.to_vec();
        }
        let outline =
            crate::empty_fill_outline(&classified, Transform::IDENTITY, self.rule, &self.limits)
                .unwrap()
                .unwrap();
        let hairline = (&outline, Transform::IDENTITY, FillRule::NonZero);
        fill(window(8, 8), hairline, &[], &self.limits).unwrap()
    }

    /// The coverage over `region` as a stroke or fill (through the stroker's
    /// pieces), and for a stroke also as a fill of its component outline.
    fn coverage(&self, region: DeviceRect) -> Vec<Vec<u8>> {
        let limits = &self.limits;
        let area = Area {
            region,
            band_height: 3,
        };
        let options = ShapeOptions {
            area,
            precision: Precision::Exact,
            limits,
        };
        let clips: Vec<(&Path, FillRule)> = self.clips.iter().map(|c| (c, self.rule)).collect();
        let clip = (!clips.is_empty())
            .then(|| ClipShape::new(&clips, Transform::IDENTITY, &options).unwrap());
        let bytes = |shape: Option<Shape>| {
            shape.map_or_else(
                || vec![0; (region.width() * region.height()) as usize],
                |s| raster(&area, Some(&s), clip.as_ref()).0,
            )
        };
        let identity = Transform::IDENTITY;
        let Some(stroke) = &self.stroke else {
            let shape =
                Shape::fill_with_clip(&self.path, identity, self.rule, clip.as_ref(), &options);
            return vec![bytes(shape.unwrap())];
        };
        let stroked = Shape::stroke_with_clip(
            &self.path,
            stroke,
            (identity, identity),
            Adjustment::None,
            clip.as_ref(),
            &options,
        );
        let (outline, transform) =
            crate::stroke_outline(&self.path, identity, stroke, limits).unwrap();
        let filled = Shape::fill_with_clip(
            &outline,
            transform,
            FillRule::NonZero,
            clip.as_ref(),
            &options,
        );
        vec![bytes(stroked.unwrap()), bytes(filled.unwrap())]
    }
}

/// Checks the vector `name` against its expected coverage within
/// `tolerance` levels, and every pixel rendered alone against the whole.
fn vector(name: &str, tolerance: u8) {
    let expected = pagura_testkit::fixture("raster", &format!("{name}.pgm"));
    let expected = expected.strip_prefix(b"P5\n8 8\n255\n").unwrap();
    let vector = Vector::read(name);
    let expected = vector.expected(expected);
    let whole = vector.coverage(window(8, 8));
    for (route, whole) in ["shape", "outline"].iter().zip(&whole) {
        assert_vector_samples(name, route, whole, &expected, tolerance);
    }
    for y in 0..8 {
        for x in 0..8 {
            let pixel = vector.coverage(DeviceRect {
                x0: x,
                y0: y,
                x1: x + 1,
                y1: y + 1,
            });
            for (route, (pixel, whole)) in ["shape", "outline"].iter().zip(pixel.iter().zip(&whole))
            {
                assert_eq!(
                    pixel[0],
                    whole[(y * 8 + x) as usize],
                    "{name} {route} tile {x},{y}"
                );
            }
        }
    }
}

fn vector_geometry(text: &str) -> (Path, FillRule, Option<Stroke>, Vec<Path>) {
    let mut builder = PathBuilder::new();
    let mut rule = FillRule::NonZero;
    let mut stroke = None;
    let mut clips = Vec::new();
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let n = |i: usize| words[i].parse::<f64>().unwrap();
        match words[0] {
            "nonzero" => rule = FillRule::NonZero,
            "evenodd" => rule = FillRule::EvenOdd,
            "stroke" => {
                stroke = Some(Stroke {
                    width: n(1),
                    cap: match words[2] {
                        "butt" => LineCap::Butt,
                        "round" => LineCap::Round,
                        _ => LineCap::ProjectingSquare,
                    },
                    join: match words[3] {
                        "round" => LineJoin::Round,
                        "bevel" => LineJoin::Bevel,
                        _ => LineJoin::Miter,
                    },
                    miter_limit: n(4),
                    ..Stroke::default()
                });
            }
            "clip" => clips.push(builder.finish()),
            "dash" => {
                let stroke = stroke.as_mut().unwrap();
                stroke.dash_phase = n(1);
                stroke.dash_array = words.iter().skip(2).map(|v| v.parse().unwrap()).collect();
            }
            "m" => {
                builder.move_to(Point::new(n(1), n(2)));
            }
            "l" => {
                builder.line_to(Point::new(n(1), n(2))).unwrap();
            }
            "c" => {
                builder
                    .cubic_to(
                        Point::new(n(1), n(2)),
                        Point::new(n(3), n(4)),
                        Point::new(n(5), n(6)),
                    )
                    .unwrap();
            }
            "h" => {
                builder.close();
            }
            _ => unreachable!(),
        }
    }
    (builder.finish(), rule, stroke, clips)
}

fn assert_vector_samples(name: &str, backend: &str, actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(actual.len(), expected.len(), "{name} {backend}");
    for (i, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "{name} {backend} pixel {i}: {actual} vs {expected}"
        );
    }
}

#[test]
fn independent_exact_polygon_oracles() {
    for name in [
        "triangle",
        "concave",
        "fractional-rectangle",
        "self-intersection",
        "overlap-nonzero",
        "overlap-evenodd",
        "square-dot",
        "zero-square-oblique",
        "join-miter",
        "join-bevel",
        "miter-limit",
        "closed-dash-seam",
    ] {
        vector(name, 1);
    }
}

#[test]
fn independent_curved_and_stroke_oracles() {
    for name in [
        "cap-butt",
        "cap-round",
        "cap-square",
        "round-dot",
        "cubic",
        "join-round",
        "dash-odd-phase",
        "zero-on-dash",
    ] {
        vector(name, 3);
    }
}

#[test]
fn stroke_transform_hairline_dash_and_miter() {
    let limits = Limits::default();
    let mut builder = PathBuilder::new();
    builder
        .move_to(Point::new(1.0, 2.0))
        .line_to(Point::new(3.0, 2.0))
        .unwrap();
    let path = builder.finish();
    let transform = Transform::scale(2.0, 1.0);
    let square = Stroke {
        width: 2.0,
        cap: LineCap::ProjectingSquare,
        ..Stroke::default()
    };
    let coverage = band_stroke(window(8, 8), &path, &square, transform, &limits).unwrap();
    // User-space square cap extends 1 unit, hence 2 device pixels in x.
    assert_eq!(coverage[8..16], [255; 8]);
    let hairline = Stroke {
        width: 0.0,
        ..Stroke::default()
    };
    let coverage = band_stroke(window(8, 8), &path, &hairline, transform, &limits).unwrap();
    assert_eq!(coverage.iter().map(|&c| u32::from(c)).sum::<u32>(), 4 * 256);
    let stroke = Stroke {
        width: 2.0,
        dash_array: vec![1.0, 1.0],
        ..Stroke::default()
    };
    let (outline, _) =
        crate::stroke::outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
    let coverage = fill(
        window(8, 8),
        (&outline, Transform::IDENTITY, FillRule::NonZero),
        &[],
        &limits,
    )
    .unwrap();
    assert_eq!(coverage.iter().map(|&n| u32::from(n)).sum::<u32>(), 2 * 255);
    for join in [LineJoin::Miter, LineJoin::Bevel, LineJoin::Round] {
        let mut p = PathBuilder::new();
        p.move_to(Point::new(1.0, 6.0))
            .line_to(Point::new(4.0, 2.0))
            .unwrap()
            .line_to(Point::new(7.0, 6.0))
            .unwrap();
        let (outline, _) = crate::stroke::outline(
            &p.finish(),
            Transform::IDENTITY,
            &Stroke {
                join,
                ..Stroke::default()
            },
            &limits,
        )
        .unwrap();
        assert!(!outline.is_empty());
    }
}

#[test]
fn invalid_geometry_and_strokes_return_errors() {
    let limits = Limits::default();
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(f64::NAN, 0.0));
    let error = fill(
        window(1, 1),
        (&builder.finish(), Transform::IDENTITY, FillRule::NonZero),
        &[],
        &limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    let stroke = Stroke {
        dash_array: vec![0.0],
        ..Stroke::default()
    };
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(0.0, 0.0));
    builder.line_to(Point::new(1.0, 0.0)).unwrap();
    assert!(
        band_stroke(
            window(1, 1),
            &builder.finish(),
            &stroke,
            Transform::IDENTITY,
            &limits
        )
        .is_err()
    );
}

#[test]
fn large_compound_triangles_and_caller_budgets() {
    let mut builder = PathBuilder::new();
    for y in 0..200 {
        for x in 0..250 {
            let (x, y) = (f64::from(x * 2), f64::from(y * 2));
            builder
                .move_to(Point::new(x, y))
                .line_to(Point::new(x + 1.0, y))
                .unwrap()
                .line_to(Point::new(x, y + 1.0))
                .unwrap()
                .close();
        }
    }
    let path = builder.finish();
    let triangles = (&path, Transform::IDENTITY, FillRule::NonZero);
    let coverage = fill(window(500, 400), triangles, &[], &Limits::default()).unwrap();
    for (y, row) in coverage.as_chunks::<500>().0.iter().enumerate() {
        for (x, &value) in row.iter().enumerate() {
            assert_eq!(value, if x % 2 == 0 && y % 2 == 0 { 128 } else { 0 });
        }
    }
    for limits in [
        Limits {
            path_segments: 10,
            ..Limits::default()
        },
        Limits {
            raster_work: 0,
            ..Limits::default()
        },
    ] {
        let error = fill(window(500, 400), triangles, &[], &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}

#[test]
fn independent_crossing_and_clip_sets() {
    for i in 0..16 {
        vector(&format!("crossing-set-{i}"), 1);
    }
}

#[test]
fn independent_winding_and_collapsing_contours() {
    for rule in ["nonzero", "evenodd"] {
        for name in [
            "boundary-triangle",
            "opposite-chains",
            "crossed-rules",
            "tiny-bowtie",
            "overlapping-contours",
            "coincident-opposite",
            "touching-contours",
            "same-clip",
            "nested-clips",
            "crossing-clips",
            "star-in-cell",
            "double-crossing-chains",
            "thin-rule",
            "coincident-same",
            "collinear",
            "huge-endpoints",
            "rounding-threshold",
            "translated-half-tie",
        ] {
            vector(&format!("winding-{name}-{rule}"), 1);
        }
        for case in 0..16 {
            vector(&format!("winding-random-{case}-{rule}"), 1);
        }
        for axis in ["horizontal", "vertical"] {
            vector(&format!("dense-micro-slivers-{axis}-{rule}"), 0);
        }
        for count in [127, 128, 129, 512, 513] {
            for axis in ["horizontal", "vertical"] {
                for direction in ["forward", "reverse"] {
                    vector(&format!("collapse-{axis}-{direction}-{count}-{rule}"), 0);
                }
            }
        }
    }
}

#[test]
fn independent_quadrilateral_component_oracles() {
    for count in [128, 129] {
        for rule in ["nonzero", "evenodd"] {
            vector(&format!("quad-union-{count}-{rule}"), 0);
        }
    }
}

#[test]
fn independent_convex_miter_stroke_oracles() {
    for compound in ["opposed", "reversed", "filled", "fallback"] {
        vector(&format!("convex-miter-compound-{compound}"), 0);
    }
    for shape in [
        "rectangle",
        "triangle",
        "octagon",
        "closed-hole",
        "reversed-side",
        "icosagon",
        "thin",
        "wide",
        "sixteen-thin",
        "sixteen-wide",
        "sixteen-filled",
        "sixteen-near-miter",
        "sixteen-reversed-inner",
    ] {
        for order in ["forward", "reverse"] {
            vector(&format!("convex-miter-{shape}-{order}"), 0);
        }
    }
}

#[test]
fn convex_strokes_preserve_reflected_oracles_at_large_origins() {
    let text = String::from_utf8(pagura_testkit::fixture(
        "raster",
        "convex-miter-icosagon-forward.path",
    ))
    .unwrap();
    let (path, _, stroke, _) = vector_geometry(&text);
    let stroke = stroke.unwrap();
    let expected = pagura_testkit::fixture("raster", "convex-miter-icosagon-forward.pgm");
    let expected = expected.strip_prefix(b"P5\n8 8\n255\n").unwrap();
    for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
        let transform = Transform::scale(-1.0, 1.0).then(Transform::translate(
            f64::from(origin.0) + 8.0,
            f64::from(origin.1),
        ));
        let limits = Limits::default();
        let (outline, transform) =
            crate::stroke_outline(&path, transform, &stroke, &limits).unwrap();
        let region = DeviceRect {
            x0: origin.0,
            y0: origin.1,
            x1: origin.0 + 8,
            y1: origin.1 + 8,
        };
        let coverage = fill(
            region,
            (&outline, transform, FillRule::NonZero),
            &[],
            &limits,
        )
        .unwrap();
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    coverage[y * 8 + x],
                    expected[y * 8 + 7 - x],
                    "{origin:?} {x},{y}"
                );
            }
        }
    }
}

#[test]
fn sixteen_sided_oracles_exercise_boundary_and_reversed_inner_fallback() {
    for shape in ["thin", "wide", "filled", "near-miter", "reversed-inner"] {
        for order in ["forward", "reverse"] {
            let text = String::from_utf8(pagura_testkit::fixture(
                "raster",
                &format!("convex-miter-sixteen-{shape}-{order}.path"),
            ))
            .unwrap();
            let (path, _, stroke, _) = vector_geometry(&text);
            let (outline, _) = crate::stroke_outline(
                &path,
                Transform::IDENTITY,
                &stroke.unwrap(),
                &Limits::default(),
            )
            .unwrap();
            assert_eq!(
                outline.boundary.is_some(),
                shape != "reversed-inner",
                "{shape} {order}"
            );
        }
    }
}

// The sixteen-sided fixture at a width whose boundary once rounded
// differently from its components, with each point moved by `offset`.
fn sixteen_sided(offset: Point) -> (Path, Stroke) {
    let text = String::from_utf8(pagura_testkit::fixture(
        "raster",
        "convex-miter-sixteen-thin-forward.path",
    ))
    .unwrap();
    let (path, _, stroke, _) = vector_geometry(&text);
    let mut moved = PathBuilder::new();
    for segment in path.segments() {
        match *segment {
            crate::Segment::MoveTo(p) => {
                moved.move_to(Point::new(p.x + offset.x, p.y + offset.y));
            }
            crate::Segment::LineTo(p) => {
                moved
                    .line_to(Point::new(p.x + offset.x, p.y + offset.y))
                    .unwrap();
            }
            crate::Segment::Close => {
                moved.close();
            }
            crate::Segment::CubicTo { .. } => panic!("polygon"),
        }
    }
    let stroke = Stroke {
        width: 1.09375,
        ..stroke.unwrap()
    };
    (moved.finish(), stroke)
}

// The component construction alone, independently of the boundary proof.
fn components(path: &Path, transform: Transform, stroke: &Stroke) -> Path {
    let limits = Limits::default();
    let stretch = transform.a.hypot(transform.b) + transform.c.hypot(transform.d);
    let mut components = PathBuilder::new();
    for contour in crate::flatten::flatten(path, transform, &limits).unwrap() {
        for run in crate::stroke::dash(contour.points, contour.closed, stroke, &limits).unwrap() {
            crate::stroke::draw(
                &mut components,
                &run,
                stroke.width / 2.0,
                crate::flatten::TOLERANCE / stretch,
                stroke,
                &limits,
            )
            .unwrap();
        }
    }
    components.finish()
}

#[test]
fn boundary_strokes_cover_their_components_under_uniform_scales() {
    let limits = Limits::default();
    for phase in 0..256 {
        let (path, stroke) = sixteen_sided(Point::new(
            f64::from(phase) / 97.0,
            f64::from(phase) / 113.0,
        ));
        for scale in [0.75, 1.0, 1.25] {
            let transform = Transform::scale(scale, scale);
            let (outline, pen) = crate::stroke_outline(&path, transform, &stroke, &limits).unwrap();
            assert!(outline.boundary.is_some());
            let components = components(&path, transform, &stroke);
            assert_eq!(outline.segments(), components.segments());
            let boundary = outline.boundary.as_deref().unwrap();
            let window = window(12, 12);
            let fill = |path| fill(window, (path, pen, FillRule::NonZero), &[], &limits).unwrap();
            assert_eq!(fill(boundary), fill(&components), "{phase} {scale}");
        }
    }
}

#[test]
fn boundary_strokes_clean_repeated_closure_and_exclude_dashes_and_affine_pens() {
    let text = String::from_utf8(pagura_testkit::fixture(
        "raster",
        "convex-miter-sixteen-thin-forward.path",
    ))
    .unwrap();
    let (path, _, stroke, _) = vector_geometry(&text);
    let mut repeated = PathBuilder::new();
    let crate::Segment::MoveTo(first) = path.segments()[0] else {
        panic!("start")
    };
    for segment in path.segments() {
        match *segment {
            crate::Segment::MoveTo(p) => {
                repeated.move_to(p);
            }
            crate::Segment::LineTo(p) => {
                repeated.line_to(p).unwrap();
            }
            crate::Segment::Close => {
                repeated.line_to(first).unwrap();
                repeated.close();
            }
            crate::Segment::CubicTo { .. } => panic!("polygon"),
        }
    }
    let path = repeated.finish();
    let stroke = stroke.unwrap();
    let limits = Limits::default();
    let (boundary, _) =
        crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
    assert!(boundary.boundary.is_some());
    for transform in [
        Transform::scale(1.0, 2.0),
        Transform {
            c: 0.5,
            ..Transform::IDENTITY
        },
    ] {
        let (outline, _) = crate::stroke_outline(&path, transform, &stroke, &limits).unwrap();
        assert!(outline.boundary.is_none());
    }
    let dashed = Stroke {
        dash_array: vec![1000.0, 1.0],
        ..stroke
    };
    let (outline, _) = crate::stroke_outline(&path, Transform::IDENTITY, &dashed, &limits).unwrap();
    assert!(outline.boundary.is_none());
}
