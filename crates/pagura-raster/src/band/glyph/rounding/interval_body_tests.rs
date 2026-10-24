//! Original-edge and budget equivalence of interval materialization.
use super::{edge_error, interval_body, materialized_interval_body, outward};
use crate::Point;
use crate::band::edges::{Line, Polygon};
use crate::band::union::Piece;
use pagura_core::{Error, Limits, Meter};

const LIVE_BYTES: usize = 37;

#[derive(Clone, Copy)]
enum Factory {
    BoundaryTriangle,
    SignedZeroLoop,
    BodyWithCells,
    SpareTriangle,
    Empty,
    HorizontalPair,
    OpenPair,
}

fn append(polygon: &mut Polygon, points: &[Point], bytes: &mut Meter) {
    polygon
        .begin(&Limits::default(), points.len(), bytes)
        .expect("fixture contour reservation");
    polygon.points.extend_from_slice(points);
    polygon.end();
}

fn build(factory: Factory) -> Polygon {
    let mut polygon = Polygon::default();
    let mut bytes = Meter::new(usize::MAX, "fixture bytes");
    match factory {
        Factory::BoundaryTriangle => append(
            &mut polygon,
            &[
                Point::new(0.25, 1.0),
                Point::new(0.75, 2.0),
                Point::new(0.9, 1.0),
            ],
            &mut bytes,
        ),
        Factory::SignedZeroLoop => append(
            &mut polygon,
            &[
                Point::new(-0.0, -0.0),
                Point::new(0.0, 0.0),
                Point::new(2.0, 0.0),
                Point::new(2.0, 1.0),
                Point::new(0.0, 1.0),
                Point::new(-0.0, -0.0),
            ],
            &mut bytes,
        ),
        Factory::BodyWithCells => {
            append(
                &mut polygon,
                &[
                    Point::new(0.0, 0.0),
                    Point::new(2.0, 0.0),
                    Point::new(2.0, 1.0),
                    Point::new(0.0, 1.0),
                ],
                &mut bytes,
            );
            polygon
                .cell(Point::new(4.25, -0.0), &Limits::default(), &mut bytes)
                .expect("canonical point cell");
        }
        Factory::SpareTriangle => {
            // Reservation precedes Polygon.end's duplicate compaction. This
            // forces actual P>A without fabricating an uncharged capacity.
            let mut points = [Point::new(0.25, 0.0); 82];
            points[80] = Point::new(0.75, 1.0);
            points[81] = Point::new(0.9, 0.0);
            append(&mut polygon, &points, &mut bytes);
        }
        Factory::Empty => {}
        Factory::HorizontalPair => append(
            &mut polygon,
            &[Point::new(-0.0, 0.5), Point::new(2.0, 0.5)],
            &mut bytes,
        ),
        Factory::OpenPair => append(
            &mut polygon,
            &[Point::new(-0.0, -0.0), Point::new(1.0, 1.0)],
            &mut bytes,
        ),
    }
    assert_eq!(bytes.used(), polygon.heap_bytes());
    polygon
}

#[derive(Debug, PartialEq, Eq)]
struct PieceBits {
    endpoints: [u64; 4],
    interval: [u64; 2],
    direction: i32,
    group: u8,
}

impl From<&Piece> for PieceBits {
    fn from(piece: &Piece) -> Self {
        Self {
            endpoints: [
                piece.line.a.x.to_bits(),
                piece.line.a.y.to_bits(),
                piece.line.b.x.to_bits(),
                piece.line.b.y.to_bits(),
            ],
            interval: [piece.lo.to_bits(), piece.hi.to_bits()],
            direction: piece.direction,
            group: piece.group,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BodyBits {
    pieces: Vec<PieceBits>,
    capacity: usize,
    arithmetic: u64,
}

#[derive(Debug, PartialEq)]
struct Outcome {
    result: Result<BodyBits, Error>,
    work: usize,
    bytes: usize,
}

fn evaluate(factory: Factory, streamed: bool, work_limit: usize, byte_limit: usize) -> Outcome {
    // Build each mode independently; no Clone or shared allocation history.
    let polygon = build(factory);
    let mut work = Meter::new(work_limit, "body work");
    let mut bytes = Meter::new(byte_limit, "body bytes");
    bytes
        .charge(LIVE_BYTES + polygon.heap_bytes())
        .expect("inherited owners fit");
    let result = if streamed {
        interval_body(polygon, &mut work, &mut bytes)
    } else {
        materialized_interval_body(polygon, &mut work, &mut bytes)
    };
    Outcome {
        result: result.map(|(pieces, arithmetic)| BodyBits {
            pieces: pieces.iter().map(PieceBits::from).collect(),
            capacity: pieces.capacity(),
            arithmetic: arithmetic.to_bits(),
        }),
        work: work.used(),
        bytes: bytes.used(),
    }
}

fn compare(factory: Factory, work: usize, bytes: usize) -> Outcome {
    let baseline = evaluate(factory, false, work, bytes);
    let streamed = evaluate(factory, true, work, bytes);
    assert_eq!(streamed, baseline);
    streamed
}

#[test]
fn omitted_unit_row_pieces_still_contribute_ordered_arithmetic() {
    let polygon = build(Factory::BoundaryTriangle);
    assert!(polygon.heap_bytes() <= polygon.vertices() * size_of::<Line>());
    let expected = polygon
        .sides()
        .filter(|line| !line.is_horizontal())
        .fold(0.0, |sum, line| {
            assert!(Piece::new(line, 0.0, 0).is_none());
            outward(sum + edge_error(line, line.interpolation_error(), [0.0, 0.0, 1.0, 1.0]))
        });
    assert!(
        expected > 0.0,
        "the boundary uncertainty meets the unit row"
    );
    let result = compare(Factory::BoundaryTriangle, usize::MAX, usize::MAX);
    let body = result.result.expect("body succeeds");
    assert_eq!(body.pieces, Vec::<PieceBits>::new());
    assert_eq!(body.capacity, 2);
    assert_eq!(body.arithmetic, expected.to_bits());
    assert_eq!(result.work, 2);
}

#[test]
fn signed_zero_duplicates_and_implicit_closing_keep_original_bits() {
    for factory in [Factory::SignedZeroLoop, Factory::OpenPair] {
        let result = compare(factory, usize::MAX, usize::MAX);
        let body = result.result.expect("body succeeds");
        let last = body.pieces.last().expect("closing edge exists");
        assert_eq!(last.endpoints[2], (-0.0_f64).to_bits());
        assert_eq!(last.endpoints[3], (-0.0_f64).to_bits());
        assert_eq!(last.direction, -1);
    }
}

#[test]
fn canonical_point_cells_follow_body_and_keep_signed_multiplicity() {
    let result = compare(Factory::BodyWithCells, usize::MAX, usize::MAX);
    let body = result.result.expect("body succeeds");
    assert_eq!(body.pieces.len(), 4);
    let sides: Vec<_> = body
        .pieces
        .iter()
        .map(|p| (p.endpoints[0], p.direction, p.group))
        .collect();
    assert_eq!(
        sides,
        vec![
            (2.0_f64.to_bits(), 1, 0),
            (0.0_f64.to_bits(), -1, 0),
            (5.0_f64.to_bits(), 1, 0),
            (4.0_f64.to_bits(), -1, 0)
        ]
    );
}

#[test]
fn polygon_spare_capacity_uses_the_old_materialized_fallback() {
    let polygon = build(Factory::SpareTriangle);
    assert!(polygon.heap_bytes() > polygon.vertices() * size_of::<Line>());
    let result = compare(Factory::SpareTriangle, usize::MAX, usize::MAX);
    let body = result.result.expect("fallback succeeds");
    assert_eq!(
        result.bytes,
        LIVE_BYTES + body.capacity * size_of::<Piece>()
    );
}

#[test]
fn byte_boundaries_keep_error_order_used_bytes_and_piece_capacity() {
    for factory in [
        Factory::BoundaryTriangle,
        Factory::BodyWithCells,
        Factory::SpareTriangle,
        Factory::Empty,
        Factory::HorizontalPair,
    ] {
        let polygon = build(factory);
        let retained = polygon.heap_bytes();
        let lines = polygon.vertices() * size_of::<Line>();
        let count = polygon.sides().filter(|line| !line.is_horizontal()).count();
        let pieces = count * size_of::<Piece>();
        let peak = LIVE_BYTES + (retained + lines).max(lines + pieces);
        for limit in [
            LIVE_BYTES + retained,
            LIVE_BYTES + retained + lines,
            peak.saturating_sub(1),
            peak,
            peak + 1,
        ] {
            if limit < LIVE_BYTES + retained {
                continue;
            }
            let result = compare(factory, usize::MAX, limit);
            if limit >= peak {
                assert!(result.result.is_ok());
            }
        }
    }
}

#[test]
fn work_refusal_keeps_byte_commitments_and_succeeds_at_exact_count() {
    for factory in [
        Factory::BoundaryTriangle,
        Factory::BodyWithCells,
        Factory::SpareTriangle,
    ] {
        let count = build(factory)
            .sides()
            .filter(|line| !line.is_horizontal())
            .count();
        assert!(count > 0);
        let failed = compare(factory, count - 1, usize::MAX);
        assert_eq!(failed.work, 0);
        let error = failed.result.expect_err("original work precharge refuses");
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        let exact = compare(factory, count, usize::MAX);
        assert!(exact.result.is_ok());
        assert_eq!(exact.work, count);
    }
}

#[test]
fn admitted_nonzero_sized_fresh_vec_reports_exact_reservation_on_every_target() {
    for count in [0, 1, 2, 3, 31, 64, 65, 257] {
        let mut bytes = Meter::new(usize::MAX, "exact-capacity assumption");
        let mut lines = Vec::<Line>::new();
        bytes
            .reserve(&mut lines, count)
            .expect("fresh Line reserve");
        assert_eq!(lines.capacity(), count);
        assert_eq!(bytes.used(), count * size_of::<Line>());
        let mut pieces = Vec::<Piece>::new();
        bytes
            .reserve(&mut pieces, count)
            .expect("fresh Piece reserve");
        assert_eq!(pieces.capacity(), count);
        assert_eq!(
            bytes.used(),
            count * (size_of::<Line>() + size_of::<Piece>())
        );
    }
}
