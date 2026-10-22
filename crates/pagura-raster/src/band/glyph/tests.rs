use super::*;
use crate::PathBuilder;
use pagura_core::ErrorKind;

fn arch() -> Path {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.cubic_to(
        Point::new(4.0 / 3.0, 8.0 / 3.0),
        Point::new(8.0 / 3.0, 8.0 / 3.0),
        Point::new(4.0, 0.0),
    )
    .unwrap();
    path.line_to(Point::new(4.0, 4.0)).unwrap();
    path.line_to(Point::new(0.0, 4.0)).unwrap();
    path.close();
    path.finish()
}

#[test]
fn relative_edges_share_the_retained_spline_allowance() {
    let mut bytes = crate::testing::bytes();
    let spline = curve::spline(&arch(), Transform::IDENTITY, &Limits::default(), &mut bytes)
        .unwrap()
        .unwrap();
    let source = spline.retained_bytes();
    let line_count = spline
        .pieces
        .iter()
        .filter(|p| matches!(p, Piece::Line(line) if !line.is_horizontal()))
        .count();
    let curve_count = spline
        .pieces
        .iter()
        .filter(|p| matches!(p, Piece::Curve(_)))
        .count();
    assert!(line_count > 0 && curve_count > 0);
    let required = line_count * size_of::<Line>() + curve_count * size_of::<Mono>();
    let mut tight = Meter::new(source + required - 1, "relative bytes");
    tight.charge(source).unwrap();
    assert_eq!(
        Relative::new(&spline, &mut tight).unwrap_err().message,
        "relative bytes"
    );
    let mut exact = Meter::new(source + required, "relative bytes");
    exact.charge(source).unwrap();
    let relative = Relative::new(&spline, &mut exact).unwrap();
    assert_eq!(
        (relative.lines.len(), relative.curves.len()),
        (line_count, curve_count)
    );
    assert_eq!(exact.used(), source + required);
}

#[test]
fn store_geometry_counts_all_retained_storage_independently_of_render_bytes() {
    let outline = arch();
    let linear = [1.0, 0.0, 0.0, 1.0];
    let limits = Limits {
        render_bytes: 0,
        ..Limits::default()
    };
    let edges = GlyphEdges::new(&outline, linear, &limits).unwrap();
    let Kind::Curve {
        spline,
        relative,
        outline: copy,
        ..
    } = &edges.kind
    else {
        panic!("the arch should certify");
    };
    let retained = size_of::<GlyphEdges>()
        + spline.retained_bytes()
        + relative.lines.capacity() * size_of::<Line>()
        + relative.curves.capacity() * size_of::<Mono>()
        + size_of::<Path>()
        + 2 * size_of::<usize>()
        + copy.retained_bytes();
    assert_eq!(edges.retained_bytes(), retained);
    assert_eq!(**copy, outline);
    let limits = Limits {
        decoded_bytes: size_of::<GlyphEdges>() + size_of::<usize>() + size_of::<Piece>() - 1,
        ..limits
    };
    let error = GlyphEdges::new(&outline, linear, &limits).unwrap_err();
    assert_eq!(
        (error.kind, error.message),
        (ErrorKind::Limit, "glyph geometry byte budget exceeded")
    );
}
