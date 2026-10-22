use super::*;
use crate::Point;
use pagura_core::Limits;

#[test]
fn retained_union_rows_charge_before_allocating() {
    let line = Line {
        a: Point::new(0.0, 0.0),
        b: Point::new(1.0, 1.0),
    };
    let mut resolved = Resolved::default();
    let mut refused = Meter::new(0, RENDER_BYTES);
    assert!(resolved.push(0, &[line], &mut refused).is_err());
    assert_eq!(
        (resolved.lines.capacity(), resolved.rows.capacity()),
        (0, 0)
    );
    let mut bytes = Meter::new(size_of::<Line>() + size_of::<(u32, u32)>(), RENDER_BYTES);
    resolved.push(0, &[line], &mut bytes).unwrap();
    assert_eq!(bytes.used(), resolved.retained_bytes());
    assert!(resolved.push(1, &[line], &mut bytes).is_err());
    assert_eq!((resolved.lines.len(), resolved.rows.len()), (1, 1));
}

#[test]
fn empty_resolved_rows_also_need_storage() {
    let mut resolved = Resolved::default();
    assert!(
        resolved
            .push(0, &[], &mut Meter::new(0, RENDER_BYTES))
            .is_err()
    );
    assert_eq!(resolved.rows.capacity(), 0);
}

#[test]
fn round_cap_overlap_resolves_only_uncertified_rows() {
    let mut path = crate::PathBuilder::new();
    path.move_to(Point::new(8.0, 5.0));
    path.line_to(Point::new(8.0, 105.0)).unwrap();
    let path = path.finish();
    let stroke = Stroke {
        width: 4.0,
        cap: crate::LineCap::Round,
        ..Stroke::default()
    };
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 16,
                y1: 128,
            },
            band_height: 32,
        },
        limits: &limits,
    };
    let shape = Shape::stroke(
        &path,
        &stroke,
        Transform::IDENTITY,
        Transform::IDENTITY,
        Adjustment::None,
        &options,
    )
    .unwrap()
    .unwrap();
    // The canonical full circles overlap the body near its ends. Their
    // uncertain rows still resolve, but the long middle is signed area.
    assert_ne!(shape.resolved.rows, []);
    assert!(shape.resolved.rows.len() < shape.bounds.height() as usize / 4);
    assert!(
        shape
            .resolved
            .rows
            .iter()
            .all(|&(y, _)| !(9..=100).contains(&y))
    );
}
