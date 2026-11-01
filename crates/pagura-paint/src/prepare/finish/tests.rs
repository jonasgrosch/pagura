use super::*;
use pagura_color::{CalculatorFunction, DeviceN, Function, Indexed};
use pagura_core::{Limits, Name};

fn precedence_scene() -> crate::testing::Build {
    use crate::testing::{Build, mark, rect_path};
    use pagura_color::{ExponentialFunction, Function};
    use pagura_core::Position;
    use pagura_interpret::{
        FontKey, GlyphSource, GlyphsNode, GroupId, MarkInfo, Node, OutlineKey, Pen, PenId,
        Placement, StateId, StrokeNode, Transfer,
    };
    use pagura_raster::{Adjustment, Point, Stroke, Transform};

    let mut b = Build::new(1024.0, 4.0);
    let function = Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.0],
            c1: vec![1.0],
            exponent: 1.0,
        })
        .unwrap(),
    );
    let state = b.state(|s| s.transfer = Some(Arc::new(Transfer::All(function))));
    let ink = b.rgb([0.2, 0.3, 0.4]);
    b.rect(
        GroupId(0),
        [0.5, 0.0, 1023.5, 4.0],
        ink,
        MarkInfo {
            position: Position::Buffer(2),
            ..mark(state)
        },
    );
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources: vec![GlyphSource {
                outline: rect_path([0.0, 0.0, 0.5, 0.5]),
                key: OutlineKey {
                    font: FontKey(1),
                    glyph: 2,
                },
                linear: Transform::IDENTITY,
            }],
            placements: vec![Placement {
                source: 0,
                origin: Point::new(2.0, 1.0),
            }],
            positions: Vec::new(),
            paint: ink,
            mark: MarkInfo {
                position: Position::Buffer(6),
                ..mark(StateId(0))
            },
        }),
    );
    b.scene.pens.push(Arc::new(Pen {
        stroke: Stroke::default(),
        transform: Transform::scale(0.0, 0.0),
        adjustment: Adjustment::None,
    }));
    b.node(
        GroupId(0),
        Node::Stroke(StrokeNode {
            path: rect_path([1.0, 1.0, 3.0, 3.0]),
            pen: PenId(0),
            paint: ink,
            mark: MarkInfo {
                position: Position::Buffer(9),
                ..mark(StateId(0))
            },
        }),
    );
    b
}

#[test]
fn precedence_scratch_is_protected_before_walk_and_returned_after_drop() {
    use crate::prepare::{ShapeBuilder, planner::Planner};
    use pagura_core::{Position, Store};
    use pagura_raster::Area;

    let b = precedence_scene();
    let limits = Limits::default();
    let warm = Store::new(1 << 20);
    crate::prepare(&b.scene, &b.request(b.page(), 1), &warm, &limits).unwrap_err();
    assert!(warm.retained_bytes() > 0);
    for height in [1, 4, 256] {
        let request = b.request(b.page(), height);
        for kept in [false, true] {
            for tight in [true, false] {
                let cold = Store::new(1 << 20);
                let store = if kept { &warm } else { &cold };
                let mut budget = Budget::new(&limits);
                let mut planner = Planner::new(
                    &b.scene,
                    &request,
                    (store, &limits),
                    Area {
                        region: b.page(),
                        band_height: height,
                    },
                    ShapeBuilder::new(store, &limits),
                    &mut budget,
                )
                .unwrap();
                let original = planner.page(&mut budget).unwrap_err();
                assert_eq!(original.position, Position::Buffer(9));
                let before = budget.bytes.used();
                if tight {
                    budget.bytes = Meter::new(before, "render memory budget exceeded");
                    budget.bytes.charge(before).unwrap();
                }
                let found = first(planner, original, &mut budget);
                assert_eq!(
                    found.position,
                    Position::Buffer(2),
                    "height={height}, kept={kept}, tight={tight}"
                );
                assert!(budget.bytes.used() < before);
            }
        }
    }
}

#[test]
fn activation_order_keeps_source_positions_with_equal_first_bands() {
    use crate::format::Format;
    use crate::plan::{Compositing, GroupPlan, GroupUse, ItemKind, Start};
    use pagura_interpret::BlendMode;
    use pagura_raster::DeviceRect;

    let mut parts = Parts {
        groups: vec![GroupPlan {
            bounds: DeviceRect::default(),
            items: vec![4, 0, 3, 1, 2],
            order: Vec::new(),
            layout: Layout {
                process: Process::Rgb,
                spots: 0,
            },
            format: Format::N16,
            knockout: false,
            start: Start::Transparent,
        }],
        items: [2, 0, 2, 0, 0]
            .map(|first| Item {
                bounds: DeviceRect::default(),
                bands: [first, first],
                serial: 0,
                position: pagura_core::Position::default(),
                work: 0,
                kind: ItemKind::Group(GroupUse {
                    stencil: None,
                    group: 0,
                    cover: None,
                    compositing: Compositing {
                        alpha: 1.0,
                        blend: BlendMode::Normal,
                        alpha_is_shape: false,
                        mask: None,
                        overprint: 0,
                    },
                    conversion: None,
                }),
            })
            .into(),
        ..Parts::default()
    };
    order(&mut parts).unwrap();
    assert_eq!(parts.groups[0].items, [4, 0, 3, 1, 2]);
    assert_eq!(parts.groups[0].order, [0, 2, 3, 1, 4]);
    let capacity = parts.groups[0].order.capacity();
    order(&mut parts).unwrap();
    assert_eq!(parts.groups[0].order, [0, 2, 3, 1, 4]);
    assert_eq!(parts.groups[0].order.capacity(), capacity);
}

#[test]
fn spot_plans_share_their_definition_and_charge_live_conversion_storage() {
    let names = [Name::new(b"First"), Name::new(b"Second")];
    let tint = Function::calculator(
        CalculatorFunction {
            domain: vec![[0.0, 1.0]; 2],
            range: vec![[0.0, 1.0]],
            program: Arc::from(b"{ add 0.5 mul }".as_slice()),
        },
        &Limits::default(),
    )
    .unwrap();
    let space = Arc::new(ColorSpace::DeviceN(Arc::new(DeviceN {
        colorants: names.iter().cloned().map(Colorant::Named).collect(),
        alternate: Arc::new(ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::new(ColorSpace::DeviceRgb),
            high: 255,
            lookup: vec![128; 256 * 3].into(),
        }))),
        tint: Arc::new(tint),
    })));
    let definitions = names.map(|name| (name, Arc::clone(&space)));
    let mut bytes = Meter::new(1 << 20, "spot memory");
    bytes.charge(31).unwrap();
    let plans = spot_plans(&definitions, &Arc::<[Name]>::from([]), &mut bytes).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].inputs, [(0, 0), (1, 1)]);
    let retained = plans.capacity() * size_of::<SpotPlan>()
        + plans[0].inputs.capacity() * size_of::<(usize, usize)>()
        + plans[0].convert.owned_bytes();
    assert_eq!(bytes.used(), 31 + retained);
    let mut tiny = Meter::new(256 * 3 * size_of::<f64>() - 1, "spot memory");
    assert_eq!(
        spot_plans(&definitions, &Arc::<[Name]>::from([]), &mut tiny)
            .unwrap_err()
            .message,
        "spot memory"
    );
}

#[test]
fn transfer_check_reservation_fits_identically_for_every_band_height() {
    use pagura_raster::{Area, DeviceRect};
    let region = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 1024,
        y1: 4,
    };
    let mut expected = None;
    for band_height in [1, 4, 256] {
        let area = Area {
            region,
            band_height,
        };
        let mut bytes = Meter::new(1 << 20, "render memory budget exceeded");
        let reserved = reserve_transfer_check(area, &mut bytes).unwrap();
        assert_eq!(*expected.get_or_insert(reserved), reserved);
        assert_eq!(bytes.used(), reserved);
        let mut tiny = Meter::new(reserved - 1, "render memory budget exceeded");
        assert_eq!(
            reserve_transfer_check(area, &mut tiny).unwrap_err().message,
            "render memory budget exceeded"
        );
        assert_eq!(tiny.used(), 0);
    }
}

#[test]
fn an_earlier_transfer_boundary_precedes_late_finalization_limits() {
    use crate::prepare::{ShapeBuilder, planner::Planner};
    use crate::testing::rect_path;
    use pagura_core::{ErrorKind, Position, Store};
    use pagura_interpret::Node;
    use pagura_raster::Area;

    for fractional in [true, false] {
        let mut b = precedence_scene();
        b.scene.groups[0].children.pop(); // all walked content now succeeds
        if !fractional {
            let Node::Fill(fill) = &mut b.scene.nodes[0] else {
                unreachable!()
            };
            fill.path = rect_path([0.0, 0.0, 1024.0, 4.0]);
            // Keep the second mark whole too: fractional default coverage
            // after a selected transfer would itself be unsupported.
            let Node::Glyphs(glyphs) = &mut b.scene.nodes[1] else {
                unreachable!()
            };
            glyphs.sources[0].outline = rect_path([0.0, 0.0, 1.0, 1.0]);
        }
        let limits = Limits::default();
        let warm = Store::new(1 << 20);
        let _ = crate::prepare(&b.scene, &b.request(b.page(), 1), &warm, &limits);
        for height in [1, 4, 256] {
            let request = b.request(b.page(), height);
            for kept in [false, true] {
                let cold = Store::new(1 << 20);
                let store = if kept { &warm } else { &cold };
                let mut budget = Budget::new(&limits);
                let mut planner = Planner::new(
                    &b.scene,
                    &request,
                    (store, &limits),
                    Area {
                        region: b.page(),
                        band_height: height,
                    },
                    ShapeBuilder::new(store, &limits),
                    &mut budget,
                )
                .unwrap();
                planner.page(&mut budget).unwrap();
                let before = budget.bytes.used();
                budget.bytes = Meter::new(before, "render memory budget exceeded");
                budget.bytes.charge(before).unwrap();
                let error = complete(planner, &mut budget).err().unwrap();
                if fractional {
                    assert_eq!(
                        (error.kind, error.position),
                        (
                            ErrorKind::Unsupported(
                                pagura_interpret::features::TRANSPARENCY_TRANSFER
                            ),
                            Position::Buffer(2)
                        )
                    );
                } else {
                    assert_eq!(error.kind, ErrorKind::Limit);
                }
            }
        }
    }
}

#[test]
fn a_later_work_refusal_cannot_consume_an_earlier_marks_validation() {
    use crate::prepare::{ShapeBuilder, planner::Planner};
    use pagura_core::{ErrorKind, Position, Store};
    use pagura_raster::Area;

    let b = precedence_scene();
    let mut one = precedence_scene();
    one.scene.groups[0].children.truncate(1);
    let default = Limits::default();
    let request = one.request(one.page(), 1);
    let store = Store::new(1 << 20);
    let mut budget = Budget::new(&default);
    let mut first_only = Planner::new(
        &one.scene,
        &request,
        (&store, &default),
        Area {
            region: one.page(),
            band_height: 1,
        },
        ShapeBuilder::new(&store, &default),
        &mut budget,
    )
    .unwrap();
    first_only.page(&mut budget).unwrap();
    let first_work = budget.work.used();
    let warm = Store::new(1 << 20);
    crate::prepare(&b.scene, &b.request(b.page(), 1), &warm, &default).unwrap_err();
    for height in [1, 4, 256] {
        let request = b.request(b.page(), height);
        for kept in [false, true] {
            let cold = Store::new(1 << 20);
            let store = if kept { &warm } else { &cold };
            for work in [first_work - 1, first_work] {
                let limits = Limits {
                    render_work: work,
                    ..default
                };
                let mut budget = Budget::new(&limits);
                let mut planner = Planner::new(
                    &b.scene,
                    &request,
                    (store, &limits),
                    Area {
                        region: b.page(),
                        band_height: height,
                    },
                    ShapeBuilder::new(store, &limits),
                    &mut budget,
                )
                .unwrap();
                let original = planner.page(&mut budget).unwrap_err();
                assert_eq!(original.kind, ErrorKind::Limit);
                if work == first_work {
                    assert_eq!(original.position, Position::Buffer(6));
                    assert_eq!(
                        first(planner, original, &mut budget).position,
                        Position::Buffer(2)
                    );
                } else {
                    assert_eq!(
                        first(planner, original, &mut budget).position,
                        Position::Buffer(2)
                    );
                }
            }
        }
    }
}
