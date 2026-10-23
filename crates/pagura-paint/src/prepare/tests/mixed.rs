//! Current mixed-glyph budgets and Scratch ownership are request invariant.

use super::assert_within_charges;
use crate::testing::{Build, mark};
use crate::{Charges, PreparedScene, Scratch};
use pagura_core::{ErrorKind, Limits, Position, Result, Store};
use pagura_interpret::{
    FontKey, GlyphSource, GlyphsNode, GroupId, MarkInfo, Node, OutlineKey, Placement, StateId,
};
use pagura_raster::{DeviceRect, GlyphEdges, PathBuilder, Point, Transform};
use std::sync::Arc;

const HEIGHTS: [u32; 5] = [1, 3, 16, 32, 256];

fn scene(reversed: bool) -> Build {
    let mut path = PathBuilder::new();
    if reversed {
        path.move_to(Point::new(0.0, 96.0));
        path.line_to(Point::new(12.0, 96.0)).unwrap();
        path.line_to(Point::new(12.0, 0.0)).unwrap();
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
        path.line_to(Point::new(12.0, 0.0)).unwrap();
        path.line_to(Point::new(12.0, 96.0)).unwrap();
        path.line_to(Point::new(0.0, 96.0)).unwrap();
    }
    path.close();
    path.move_to(Point::new(2.25, 35.25));
    path.close();
    let outline = Arc::new(path.finish());
    assert!(
        GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &Limits::default())
            .unwrap()
            .curved()
    );
    let mut b = Build::new(24.0, 112.0);
    let ink = b.rgb([0.1, 0.2, 0.3]);
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources: vec![GlyphSource {
                outline,
                key: OutlineKey {
                    font: FontKey(1),
                    glyph: 1,
                },
                linear: Transform::IDENTITY,
            }],
            placements: vec![Placement {
                source: 0,
                origin: Point::new(3.137, 2.271),
            }],
            positions: vec![(0, Position::Buffer(71))],
            paint: ink,
            mark: MarkInfo {
                position: Position::Buffer(7),
                ..mark(StateId(0))
            },
        }),
    );
    b
}

#[derive(Debug, Clone, Copy)]
enum Budget {
    Bytes,
    Work,
    Raster,
}

impl Budget {
    fn limits(self, value: usize) -> Limits {
        let mut limits = Limits::default();
        match self {
            Self::Bytes => limits.render_bytes = value,
            Self::Work => limits.render_work = value,
            Self::Raster => limits.raster_work = value,
        }
        limits
    }

    /// Finds a current adjacent refusal/acceptance pair, then verifies it;
    /// no historical algorithm's charge is an expected value.
    fn boundary(self, b: &Build, region: DeviceRect, store: &Store) -> usize {
        let limits = Limits::default();
        let (mut lo, mut hi) = (
            0,
            match self {
                Self::Bytes => limits.render_bytes,
                Self::Work => limits.render_work,
                Self::Raster => limits.raster_work,
            },
        );
        let request = b.request(region, 32);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match crate::prepare(&b.scene, &request, store, &self.limits(mid)) {
                Ok(_) => hi = mid,
                Err(error) => {
                    assert_eq!(error.kind, ErrorKind::Limit);
                    lo = mid + 1;
                }
            }
        }
        assert!(lo > 0);
        crate::prepare(&b.scene, &request, store, &self.limits(lo)).unwrap();
        assert!(crate::prepare(&b.scene, &request, store, &self.limits(lo - 1)).is_err());
        lo
    }
}

fn render(
    b: &Build,
    request: &crate::RenderRequest,
    store: &Store,
    limits: &Limits,
    scratch: &mut Scratch,
) -> Result<(Vec<u8>, Charges)> {
    let prepared = crate::prepare(&b.scene, request, store, limits)?;
    let charges = prepared.charges();
    let mut rendering = prepared.with_scratch(scratch)?;
    let mut rows = Vec::new();
    rendering.render(&mut rows);
    assert_within_charges(&rendering, charges);
    Ok((rows, charges))
}

fn prime(prepared: &PreparedScene, scratch: &mut Scratch) {
    prepared
        .with_scratch(scratch)
        .unwrap()
        .render(&mut Vec::new());
    assert!(scratch.retained_bytes() > 0);
}

#[test]
fn mixed_glyph_tight_budgets_preserve_errors_charges_and_scratch_invariance() {
    let large = Build::new(192.0, 256.0);
    let large = large
        .prepare(&large.request(large.page(), 256), &Limits::default())
        .unwrap();
    for reversed in [false, true] {
        let b = scene(reversed);
        for first_row in [0, 48] {
            let region = DeviceRect {
                y0: first_row,
                ..b.page()
            };
            let warm = Store::new(1 << 20);
            crate::prepare(&b.scene, &b.request(region, 32), &warm, &Limits::default()).unwrap();
            assert!(warm.retained_bytes() > 0);
            let normal = render(
                &b,
                &b.request(region, 32),
                &warm,
                &Limits::default(),
                &mut Scratch::default(),
            )
            .unwrap();
            for budget in [Budget::Bytes, Budget::Work, Budget::Raster] {
                let boundary = budget.boundary(&b, region, &warm);
                for value in [boundary - 1, boundary] {
                    let limits = budget.limits(value);
                    let mut expected = None;
                    for band_height in HEIGHTS {
                        let request = b.request(region, band_height);
                        let mut fitted = None;
                        for kept in [false, true] {
                            for reused in [false, true] {
                                let cold = Store::new(1 << 20);
                                let store = if kept { &warm } else { &cold };
                                let mut scratch = Scratch::default();
                                if reused {
                                    prime(&large, &mut scratch);
                                }
                                let before = scratch.retained_bytes();
                                let actual = render(&b, &request, store, &limits, &mut scratch);
                                if value == boundary {
                                    let (pixels, _) = actual.as_ref().unwrap();
                                    assert_eq!(pixels, &normal.0);
                                    if let Some(fitted) = fitted {
                                        assert_eq!(scratch.retained_bytes(), fitted);
                                    } else {
                                        fitted = Some(scratch.retained_bytes());
                                    }
                                } else {
                                    assert_eq!(actual.as_ref().unwrap_err().kind, ErrorKind::Limit);
                                    assert_eq!(scratch.retained_bytes(), before);
                                }
                                if let Some(expected) = &expected {
                                    assert_eq!(
                                        &actual, expected,
                                        "{budget:?}={value}, height={band_height}, warm={kept}, reused={reused}, reverse={reversed}, row={first_row}"
                                    );
                                } else {
                                    expected = Some(actual);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn source_proof_scene() -> Build {
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
    path.move_to(Point::new(32.0, 7.0));
    path.line_to(Point::new(38.0, 7.0)).unwrap();
    path.line_to(Point::new(38.0, 0.0)).unwrap();
    path.cubic_to(
        Point::new(36.0, 4.0),
        Point::new(34.0, 4.0),
        Point::new(32.0, 0.0),
    )
    .unwrap();
    path.close();
    // The disjoint quadratic lobes have opposite signs, so the source
    // cannot use the common-sign analytic body. Its initial polygon
    // retains original-control leaf evidence, even for this small region.
    let outline = Arc::new(path.finish());
    assert!(
        !GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &Limits::default())
            .unwrap()
            .curved()
    );
    let mut b = Build::new(8.0, 10.0);
    let ink = b.rgb([0.1, 0.2, 0.3]);
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources: vec![GlyphSource {
                outline,
                key: OutlineKey {
                    font: FontKey(17),
                    glyph: 7,
                },
                linear: Transform::IDENTITY,
            }],
            placements: vec![Placement {
                source: 0,
                origin: Point::new(0.375, 0.25),
            }],
            positions: vec![(0, Position::Buffer(71))],
            paint: ink,
            mark: MarkInfo {
                position: Position::Buffer(7),
                ..mark(StateId(0))
            },
        }),
    );
    b
}

#[derive(Debug, Clone, Copy)]
enum GlyphHistory {
    Cold,
    Primed,
}

#[test]
fn source_leaf_evidence_keeps_fill_work_boundaries_independent_of_store_history() {
    let b = source_proof_scene();
    let region = b.page();
    let defaults = Limits::default();
    let warm = Store::new(1 << 20);
    let request = b.request(region, 32);
    let normal = render(&b, &request, &warm, &defaults, &mut Scratch::default()).unwrap();
    assert!(warm.retained_bytes() > 0);
    // Prepare only during the search. Start at the actual reserved render
    // work and double if certification needs more per-fill preparation.
    // Both searches make strict progress over a finite integer interval.
    let mut lo = 0;
    let mut hi = normal.1.render_work().max(1).min(defaults.raster_work);
    loop {
        match crate::prepare(&b.scene, &request, &warm, &Budget::Raster.limits(hi)) {
            Ok(_) => break,
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Limit);
                let next = hi.saturating_mul(2).min(defaults.raster_work);
                assert!(next > hi, "the known default preparation must fit");
                hi = next;
            }
        }
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match crate::prepare(&b.scene, &request, &warm, &Budget::Raster.limits(mid)) {
            Ok(_) => hi = mid,
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Limit);
                lo = mid + 1;
            }
        }
    }
    assert!(lo > 0);
    for value in [lo - 1, lo] {
        let limits = Budget::Raster.limits(value);
        let mut expected = None;
        for band_height in [1, 32] {
            for capacity in [0, 1 << 20] {
                for history in [GlyphHistory::Cold, GlyphHistory::Primed] {
                    let store = Store::new(capacity);
                    if matches!(history, GlyphHistory::Primed) {
                        crate::prepare(&b.scene, &request, &store, &defaults).unwrap();
                        if capacity == 0 {
                            assert_eq!(store.retained_bytes(), 0);
                        } else {
                            assert!(store.retained_bytes() > 0);
                        }
                    }
                    let actual = render(
                        &b,
                        &b.request(region, band_height),
                        &store,
                        &limits,
                        &mut Scratch::default(),
                    );
                    if value == lo {
                        assert_eq!(actual.as_ref().unwrap(), &normal);
                    } else {
                        let error = actual.as_ref().unwrap_err();
                        assert_eq!(error.kind, ErrorKind::Limit);
                        assert_eq!(error.position, Position::Buffer(71));
                    }
                    if let Some(expected) = &expected {
                        assert_eq!(
                            &actual, expected,
                            "raster_work={value}, band={band_height}, capacity={capacity}, history={history:?}"
                        );
                    } else {
                        expected = Some(actual);
                    }
                }
            }
        }
    }
}
