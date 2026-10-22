//! Stroke preparation: singular pens and resolved cells under clips.

use super::*;
use crate::Stroke;

#[test]
fn singular_pens_are_unsupported() {
    let limits = Limits::default();
    let area = area(0, 0, 8, 8, 4);
    let path = polygon(&[
        Point::new(1.0, 1.0),
        Point::new(5.0, 5.0),
        Point::new(1.0, 5.0),
    ]);
    let error = Shape::stroke(
        &path,
        &Stroke::default(),
        Transform::scale(0.0, 1.0),
        Transform::IDENTITY,
        Adjustment::None,
        &public(area, &limits, Precision::Exact),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        pagura_core::ErrorKind::Unsupported(crate::features::SINGULAR_PEN)
    );
}

/// A stroke whose union needs resolved cells keeps them when an
/// approximate clip is multiplied in: the Comment note icon's frame and
/// lines within its form's box.
#[test]
fn multiplied_clips_keep_resolved_cells() {
    let limits = Limits::default();
    let area = area(0, 0, 8, 8, 8);
    let path = {
        let mut b = PathBuilder::new();
        b.move_to(Point::new(0.5, 0.5));
        for (x, y) in [(7.5, 0.5), (7.5, 7.5), (0.5, 7.5)] {
            b.line_to(Point::new(x, y)).unwrap();
        }
        b.close();
        for line in [
            [(2.0, 5.5), (6.0, 5.5)],
            [(2.0, 4.0), (6.0, 4.0)],
            [(2.0, 2.5), (4.5, 2.5)],
            [(2.0, 0.5), (1.0, 0.0)],
        ] {
            b.move_to(Point::new(line[0].0, line[0].1));
            b.line_to(Point::new(line[1].0, line[1].1)).unwrap();
        }
        b.finish()
    };
    let pen = Transform::scale(2.0, 2.0);
    let device = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 8.0);
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    let mut bbox = PathBuilder::new();
    bbox.rect(0.0, 0.0, 8.0, 8.0);
    let bbox = bbox.finish();
    let coverage = |precision| {
        let clip = ClipShape::new(
            &[(&bbox, FillRule::NonZero)],
            device,
            &public(area, &limits, precision),
        )
        .unwrap();
        let o = Prep {
            area,
            precision,
            limits: &limits,
            clip: Some(&clip),
        };
        let s = Shape::stroke_in(&path, &stroke, (pen, device), Adjustment::None, &o)
            .unwrap()
            .unwrap();
        raster(&area, Some(&s), Some(&clip)).0
    };
    assert_eq!(coverage(Precision::Approximate), coverage(Precision::Exact));
}
