//! Stroke preparation: singular pens.

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
        &public(area, &limits),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        pagura_core::ErrorKind::Unsupported(crate::features::SINGULAR_PEN)
    );
}
