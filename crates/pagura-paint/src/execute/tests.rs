//! The executor on hand-built plans, for what scenes cannot carry before
//! the paint sources and functions exist: varying paint with alpha,
//! transfer selection, spot channels with overprint preview, soft-mask
//! transfer functions, black generation, and narrow stores of group
//! results. Expected values are clause-11 formulas evaluated here.

use super::{Extents, Rendering, Sizes, StoreOut, Tops, Workspace, each_span};
use crate::format::Format;
use crate::plan::{
    Compositing, Conversion, Cover, Curve, Glyph, GroupPlan, GroupUse, Item, ItemKind, Layout,
    Mark, MaskPlan, Output, Plan, Run, SpotPlan, Start,
};
use crate::prepare::Shapes;
use crate::source::{PaintContext, Pixels, Source, SpanColors};
use crate::testing::Box4;
use pagura_color::{
    ColorSpace, Colorant, Conversion as ColorConversion, Converter, Encoder, ExponentialFunction,
    Function, Painted, Process, Separation, Target,
};
use pagura_core::{Limits, Meter, Position, Store};
use pagura_interpret::{BlendMode, Composite, MaskKind, Shading, ShadingFunction, ShadingKind};
use pagura_raster::{Area, DeviceRect, PathBuilder, Point, Transform};
use std::sync::Arc;

#[test]
fn output_band_capacity_uses_actual_edge_rows_and_checked_products() {
    for (region, expected) in [
        (
            DeviceRect {
                x0: 2,
                y0: 31,
                x1: 5,
                y1: 33,
            },
            12,
        ),
        (
            DeviceRect {
                x0: 2,
                y0: 31,
                x1: 5,
                y1: 65,
            },
            384,
        ),
        (
            DeviceRect {
                x0: 2,
                y0: 5,
                x1: 5,
                y1: 15,
            },
            120,
        ),
        (
            DeviceRect {
                x0: 2,
                y0: u32::MAX - 3,
                x1: 5,
                y1: u32::MAX,
            },
            36,
        ),
        (DeviceRect::default(), 0),
    ] {
        let area = Area {
            region,
            band_height: 32,
        };
        assert_eq!(
            super::output_band_bytes(area).unwrap(),
            expected,
            "{area:?}"
        );
    }
    let enormous = Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: u32::MAX,
            y1: u32::MAX,
        },
        band_height: u32::MAX,
    };
    assert_eq!(
        super::output_band_bytes(enormous).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

/// `C0 + x^N (C1 − C0)` of one input.
fn exponential(c0: &[f64], c1: &[f64], exponent: f64) -> Arc<Function> {
    Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: c0.to_vec(),
            c1: c1.to_vec(),
            exponent,
        })
        .unwrap(),
    )
}

/// `x²`.
fn square() -> Curve {
    Curve::new(&exponential(&[0.0], &[1.0], 2.0)).unwrap()
}

/// A spot ink with this CMYK composition per unit of tint, converted into
/// CMYK.
fn ink(cmyk: [f64; 4]) -> Converter {
    let space = ColorSpace::Separation(Arc::new(Separation {
        colorant: Colorant::Named(pagura_core::Name::from(b"Spot")),
        alternate: Arc::new(ColorSpace::DeviceCmyk),
        tint: exponential(&[0.0; 4], &cmyk, 1.0),
    }));
    Converter::new(
        &space,
        &Target::new(Process::Cmyk),
        &ColorConversion::default(),
    )
    .unwrap()
}

/// An anti-aliased axial shading across the page into `channels` process
/// channels, not extended, so that colours vary and pixels near the axis
/// ends are partly covered.
fn gradient(channels: usize) -> Source {
    let (space, process, c0, c1): (_, _, &[f64], &[f64]) = match channels {
        1 => (ColorSpace::DeviceGray, Process::Gray, &[0.1], &[0.9]),
        _ => (
            ColorSpace::DeviceRgb,
            Process::Rgb,
            &[0.1, 0.6, 0.9],
            &[0.9, 0.2, 0.4],
        ),
    };
    let shading = Shading {
        object: None,
        space: Arc::new(space),
        background: None,
        bbox: None,
        antialias: true,
        kind: ShadingKind::Axial {
            start: Point::new(1.6, 0.3),
            end: Point::new(10.3, 4.1),
            domain: [0.0, 1.0],
            extend: [false, false],
        },
        function: Some(ShadingFunction::Single(exponential(c0, c1, 1.0))),
    };
    let target = Target::new(process);
    let store = Store::new(1 << 16);
    let context = PaintContext {
        transform: Transform::IDENTITY,
        target: &target,
        composite: &Composite::default(),
        store: &store,
        limits: &Limits::default(),
        region: page_rect(64, 64),
        upright: 0.0,
    };
    let mut functions = Meter::new(usize::MAX, "test functions");
    let source = Source::shading(
        &shading,
        Transform::IDENTITY,
        &context,
        64 * 64,
        (
            &mut functions,
            &Meter::new(Limits::default().render_bytes, "test bytes"),
        ),
    );
    let source = source.unwrap();
    assert!(matches!(source, Source::Shading(_)));
    source
}

/// The colours and alpha `paint` gives pixel `(x, y)`.
fn sample(paint: &Source, (x, y): (u32, u32)) -> (Vec<f64>, f64) {
    let mut out = SpanColors::default();
    let pixels = Pixels::new(x, y, 1);
    paint.fill(
        pixels,
        &mut crate::source::PaintScratch::default(),
        &mut out,
        &mut Vec::new(),
    );
    (out.colors, out.alpha.first().copied().unwrap_or(1.0))
}

fn solid(color: &[f64], painted: u64) -> Source {
    Source::Solid {
        color: color.into(),
        painted: Painted(painted),
    }
}

fn opaque() -> Compositing {
    Compositing {
        alpha: 1.0,
        blend: BlendMode::Normal,
        alpha_is_shape: false,
        mask: None,
        overprint: 0,
    }
}

const RGB: Layout = Layout {
    process: Process::Rgb,
    spots: 0,
};

/// A plan under construction over a `width` by `height` page.
struct Fixture {
    width: u32,
    height: u32,
    groups: Vec<GroupPlan>,
    items: Vec<Item>,
    /// Shape outlines in device space, keyed in order.
    paths: Vec<pagura_raster::Path>,
    paints: Vec<Source>,
    masks: Vec<MaskPlan>,
    conversions: Vec<Conversion>,
    output: Output,
}

fn page_rect(width: u32, height: u32) -> DeviceRect {
    DeviceRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    }
}

impl Fixture {
    fn new(width: u32, height: u32, layout: Layout, format: Format) -> Self {
        Self {
            width,
            height,
            groups: vec![GroupPlan {
                bounds: page_rect(width, height),
                items: Vec::new(),
                order: Vec::new(),
                layout,
                format,
                knockout: false,
                start: Start::Transparent,
            }],
            items: Vec::new(),
            paths: Vec::new(),
            paints: Vec::new(),
            masks: Vec::new(),
            conversions: Vec::new(),
            output: Output {
                layout,
                background: [255; 4],
                spots: Vec::new(),
                transfers: Vec::new(),
            },
        }
    }

    fn bounds(rect: Box4) -> DeviceRect {
        crate::prepare::bounds::device(Some(rect), page_rect(u32::MAX, u32::MAX))
    }

    fn push(&mut self, group: u32, bounds: DeviceRect, kind: ItemKind) {
        let id = u32::try_from(self.items.len()).unwrap();
        self.items.push(Item {
            bounds,
            bands: [0, u32::MAX],
            serial: id,
            position: Position::Unknown,
            work: 0,
            kind,
        });
        self.groups[group as usize].items.push(id);
    }

    fn mark(
        &mut self,
        group: u32,
        rect: Box4,
        paint: Source,
        compositing: Compositing,
        select: u16,
    ) {
        let shape = u32::try_from(self.paths.len()).unwrap();
        self.paths.push((*crate::testing::rect_path(rect)).clone());
        let paint_index = u32::try_from(self.paints.len()).unwrap();
        self.paints.push(paint);
        let kind = ItemKind::Mark(Mark {
            cover: Cover {
                shape: Some(shape),
                clip: None,
            },
            paint: paint_index,
            scratch: paint_index,
            compositing,
            select,
            records: true,
            boundary: crate::plan::Boundary::Absent,
            words: None,
        });
        self.push(group, Self::bounds(rect), kind);
    }

    fn group(&mut self, plan: GroupPlan) -> u32 {
        self.groups.push(plan);
        u32::try_from(self.groups.len() - 1).unwrap()
    }

    fn use_group(
        &mut self,
        parent: u32,
        group: u32,
        compositing: Compositing,
        conversion: Option<u32>,
    ) {
        let bounds = self.groups[group as usize].bounds;
        let kind = ItemKind::Group(GroupUse {
            group,
            cover: None,
            compositing,
            conversion,
        });
        self.push(parent, bounds, kind);
    }

    fn render(self, band_height: u32) -> Vec<u8> {
        let region = page_rect(self.width, self.height);
        let area = Area {
            region,
            band_height,
        };
        let mut groups = self.groups;
        for group in &mut groups {
            group.order = (0..u32::try_from(group.items.len()).unwrap()).collect();
        }
        // Every paint takes its own storage, as marks name them.
        let slots = crate::plan::Slots {
            of: (0..u32::try_from(self.paints.len()).unwrap()).collect(),
            layouts: self.paints.iter().map(Source::scratch_layout).collect(),
            empty: None,
        };
        let parts = crate::prepare::planner::Parts {
            groups,
            items: self.items,
            masks: self.masks,
            paints: self.paints,
            slots,
            conversions: self.conversions,
            transfers: Vec::new(),
        };
        let rows = self.height.min(256) as usize;
        let stack: Tops = crate::prepare::stacks::needs(
            &parts,
            rows,
            &mut Meter::new(usize::MAX, "test stack analysis"),
        )
        .unwrap();
        let selections = !self.output.transfers.is_empty();
        let plan = Plan {
            _names: crate::plan::TargetNames::default(),
            area,
            groups: parts.groups,
            items: parts.items,
            masks: parts.masks,
            paints: parts.paints,
            slots: parts.slots,
            conversions: parts.conversions,
            output: self.output,
            stack,
            sizes: Sizes {
                width: self.width as usize,
                rows,
                channels: 40,
                stride: 43,
                selections,
            },
            scratch: 0,
        };
        let mut bytes = Meter::new(usize::MAX, "test workspace");
        let mut workspace = Workspace::new(&plan, plan.sizes, None, &mut bytes).unwrap();
        let mut free = Meter::new(usize::MAX, "test rasterizer");
        let mut rasterizer = pagura_raster::BandRasterizer::new(area, &mut free).unwrap();
        let shapes: Shapes = crate::testing::path_shapes(area, &self.paths);
        let mut rendering = Rendering {
            plan: &plan,
            shapes: &shapes,
            workspace: &mut workspace,
            rasterizer: &mut rasterizer,
        };
        let mut rows = Vec::new();
        rendering.render(&mut rows);
        // A second render in the same storage writes the same rows.
        let mut again = Vec::new();
        rendering.render(&mut again);
        assert_eq!(rows, again);
        rows
    }
}

fn assert_close(bytes: &[u8], width: u32, (x, y): (u32, u32), expected: [f64; 4]) {
    let start = ((y * width + x) * 4) as usize;
    for (channel, (&byte, value)) in bytes[start..start + 4].iter().zip(expected).enumerate() {
        assert!(
            (f64::from(byte) - value * 255.0).abs() <= 1.0,
            "({x}, {y}) channel {channel}: {byte} against {}",
            value * 255.0
        );
    }
}

#[test]
fn varying_paint_with_alpha_composites_in_every_format() {
    for format in [Format::F64, Format::F32, Format::N16] {
        for band_height in [1, 3, 8] {
            let mut f = Fixture::new(12, 6, RGB, format);
            f.mark(0, [0.0, 0.0, 12.0, 6.0], gradient(3), opaque(), 0);
            let bytes = f.render(band_height);
            let paint = gradient(3);
            let mut partial = 0;
            for y in 0..6 {
                for x in 0..12 {
                    let (color, a) = sample(&paint, (x, y));
                    partial += usize::from(a > 0.0 && a < 1.0);
                    let c = [0, 1, 2].map(|k| color[k] * a + (1.0 - a));
                    assert_close(&bytes, 12, (x, y), [c[0], c[1], c[2], 1.0]);
                }
            }
            assert!(partial > 0, "the gradient has partly covered pixels");
        }
    }
}

#[test]
fn the_last_opaque_mark_selects_its_transfer() {
    let mut f = Fixture::new(4, 2, RGB, Format::F64);
    f.output.transfers = vec![[Some(square()), None, Some(square())]];
    f.mark(
        0,
        [0.0, 0.0, 4.0, 2.0],
        solid(&[0.5, 0.6, 0.7], 0b111),
        opaque(),
        1,
    );
    f.mark(
        0,
        [2.0, 0.0, 4.0, 2.0],
        solid(&[0.3, 0.4, 0.5], 0b111),
        opaque(),
        0,
    );
    let bytes = f.render(32);
    assert_close(&bytes, 4, (1, 1), [0.25, 0.6, 0.49, 1.0]);
    assert_close(&bytes, 4, (3, 0), [0.3, 0.4, 0.5, 1.0]);
}

#[test]
fn spot_channels_overprint_and_preview_through_their_alternate() {
    let layout = Layout {
        process: Process::Cmyk,
        spots: 1,
    };
    for overprint in [true, false] {
        let mut f = Fixture::new(2, 1, layout, Format::F64);
        f.output.spots = vec![SpotPlan {
            convert: ink([0.0, 0.8, 0.3, 0.0]),
            components: 1,
            inputs: vec![(0, 0)],
        }];
        f.mark(
            0,
            [0.0, 0.0, 2.0, 1.0],
            solid(&[1.0, 1.0, 1.0, 0.5, 1.0], 0b1111),
            opaque(),
            0,
        );
        let spot = Compositing {
            overprint: if overprint { layout.mask() } else { 0 },
            ..opaque()
        };
        f.mark(
            0,
            [0.0, 0.0, 2.0, 1.0],
            solid(&[1.0, 1.0, 1.0, 1.0, 0.4], 0b1_0000),
            spot,
            0,
        );
        let bytes = f.render(32);
        // The spot's tint 0.6 adds the ink's CMYK with saturation.
        let equivalent = [1.0, 1.0 - 0.6 * 0.8, 1.0 - 0.6 * 0.3, 1.0];
        let process = if overprint {
            [1.0, 1.0, 1.0, 0.5]
        } else {
            [1.0; 4]
        };
        let mixed = [0, 1, 2, 3].map(|i| (process[i] + equivalent[i] - 1.0_f64).max(0.0));
        let rgb = [0, 1, 2].map(|i| (mixed[i] + mixed[3] - 1.0_f64).max(0.0));
        assert_close(&bytes, 2, (1, 0), [rgb[0], rgb[1], rgb[2], 1.0]);
    }
}

#[test]
fn soft_mask_transfer_functions_map_values() {
    let mut f = Fixture::new(3, 2, RGB, Format::F64);
    let group = f.group(GroupPlan {
        bounds: page_rect(3, 2),
        items: Vec::new(),
        order: Vec::new(),
        layout: RGB,
        format: Format::F64,
        knockout: false,
        start: Start::Transparent,
    });
    let half = Compositing {
        alpha: 0.5,
        ..opaque()
    };
    f.mark(
        group,
        [0.0, 0.0, 3.0, 2.0],
        solid(&[0.0, 0.0, 0.0], 0b111),
        half,
        0,
    );
    f.masks.push(MaskPlan {
        group,
        kind: MaskKind::Alpha,
        isolated: true,
        backdrop: [0.0; 4],
        transfer: Some(square()),
        extent: page_rect(3, 2),
        outside: 0.0,
    });
    let masked = Compositing {
        mask: Some(0),
        ..opaque()
    };
    f.mark(
        0,
        [0.0, 0.0, 3.0, 2.0],
        solid(&[1.0, 0.0, 0.0], 0b111),
        masked,
        0,
    );
    let bytes = f.render(1);
    assert_close(&bytes, 3, (2, 1), [1.0, 0.75, 0.75, 1.0]);
}

#[test]
fn black_generation_converts_group_results_into_cmyk() {
    let cmyk = Layout {
        process: Process::Cmyk,
        spots: 0,
    };
    let mut f = Fixture::new(2, 2, cmyk, Format::F64);
    let group = f.group(GroupPlan {
        bounds: page_rect(2, 2),
        items: Vec::new(),
        order: Vec::new(),
        layout: RGB,
        format: Format::F64,
        knockout: false,
        start: Start::Transparent,
    });
    f.mark(
        group,
        [0.0, 0.0, 2.0, 2.0],
        solid(&[0.25, 0.5, 0.75], 0b111),
        opaque(),
        0,
    );
    let mut separation = ColorConversion::default();
    separation.black_generation = Some(exponential(&[0.0], &[0.5], 1.0));
    separation.undercolor_removal = Some(exponential(&[0.0], &[0.0], 1.0));
    f.conversions.push(Conversion {
        from: Process::Rgb,
        to: Process::Cmyk,
        encoder: Encoder::new(Process::Cmyk, &separation),
    });
    f.use_group(0, group, opaque(), Some(0));
    let bytes = f.render(32);
    // Inks 0.75, 0.5, 0.25; black 0.125 of the common 0.25, nothing removed.
    let additive = [0.25, 0.5, 0.75, 0.875];
    let rgb = [0, 1, 2].map(|i| (additive[i] + additive[3] - 1.0_f64).max(0.0));
    assert_close(&bytes, 2, (1, 1), [rgb[0], rgb[1], rgb[2], 1.0]);
}

#[test]
fn narrow_parents_take_group_results_as_one_rounded_store() {
    for (page, child) in [(Format::F32, Format::F32), (Format::N16, Format::F64)] {
        let mut f = Fixture::new(5, 3, RGB, page);
        f.mark(
            0,
            [0.0, 0.0, 5.0, 3.0],
            solid(&[0.2, 0.7, 0.4], 0b111),
            opaque(),
            0,
        );
        let group = f.group(GroupPlan {
            bounds: page_rect(5, 3),
            items: Vec::new(),
            order: Vec::new(),
            layout: RGB,
            format: child,
            knockout: false,
            start: Start::Transparent,
        });
        let translucent = Compositing {
            alpha: 0.6,
            ..opaque()
        };
        f.mark(
            group,
            [1.0, 0.0, 5.0, 3.0],
            solid(&[0.9, 0.1, 0.3], 0b111),
            translucent,
            0,
        );
        f.mark(
            group,
            [0.0, 1.0, 3.0, 3.0],
            solid(&[0.1, 0.3, 0.9], 0b111),
            translucent,
            0,
        );
        f.use_group(
            0,
            group,
            Compositing {
                alpha: 0.7,
                ..opaque()
            },
            None,
        );
        let bytes = f.render(2);
        // At (2, 2) both members overlap: group colour by source-over.
        let (a, b) = ([0.9, 0.1, 0.3], [0.1, 0.3, 0.9]);
        let group_alpha = 0.6 + 0.6 * 0.4;
        let premultiplied = [0, 1, 2].map(|i| 0.6 * b[i] + 0.4 * 0.6 * a[i]);
        let m = 0.7;
        let back = [0.2, 0.7, 0.4];
        let out = [0, 1, 2].map(|i| (1.0 - m * group_alpha) * back[i] + m * premultiplied[i]);
        let start = ((2 * 5 + 2) * 4) as usize;
        for (channel, (&byte, value)) in bytes[start..start + 3].iter().zip(out).enumerate() {
            assert!(
                (f64::from(byte) - value * 255.0).abs() <= 1.0,
                "{page:?} channel {channel}: {byte} against {}",
                value * 255.0
            );
        }
        assert_eq!(bytes[start + 3], 255);
    }
}

/// A shape with holes within `bounds`: unit cells, each left out, filled
/// or partly filled by a hash of its place, so that coverage is full,
/// partial and zero within a row.
fn speckled(shape: u32, bounds: DeviceRect) -> pagura_raster::Path {
    let mut path = PathBuilder::new();
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            let h = (shape.wrapping_mul(2_654_435_761)
                ^ x.wrapping_mul(40_503)
                ^ y.wrapping_mul(9_973))
            .wrapping_mul(2_246_822_519);
            let inset = match (h >> 13) % 6 {
                0 if (x, y) != (bounds.x0, bounds.y0) => continue,
                0..=2 => 0.0,
                _ => f64::from((h >> 3) % 7 + 1) / 16.0,
            };
            let (x, y) = (f64::from(x), f64::from(y));
            path.rect(x + inset, y, 1.0 - inset, 1.0 - inset / 2.0);
        }
    }
    path.finish()
}

impl Fixture {
    /// One item per placement, or one run of them, over `bounds` with one
    /// solid paint.
    fn glyphs(
        &mut self,
        group: u32,
        bounds: &[DeviceRect],
        (color, compositing): (&[f64], Compositing),
        run: Option<bool>,
    ) {
        let first = u32::try_from(self.paths.len()).unwrap();
        for (i, &b) in bounds.iter().enumerate() {
            let shape = first + u32::try_from(i).unwrap();
            self.paths.push(speckled(shape, b));
        }
        let paint = u32::try_from(self.paints.len()).unwrap();
        self.paints.push(solid(color, 0b111));
        let words = (compositing.is_opaque() && color.len() == 3).then(|| {
            let word = crate::kernel::word;
            [word(color[0]), word(color[1]), word(color[2]), u16::MAX]
        });
        let mark = |shape: Option<u32>| Mark {
            cover: Cover { shape, clip: None },
            paint,
            scratch: paint,
            compositing,
            select: 0,
            records: false,
            boundary: crate::plan::Boundary::Absent,
            words,
        };
        let Some(alone) = run else {
            for (i, &b) in bounds.iter().enumerate() {
                let shape = first + u32::try_from(i).unwrap();
                self.push(group, b, ItemKind::Mark(mark(Some(shape))));
            }
            return;
        };
        let mut glyphs: Vec<Glyph> = bounds
            .iter()
            .enumerate()
            .map(|(i, &b)| Glyph {
                shape: first + u32::try_from(i).unwrap(),
                bounds: b,
                bands: [0, u32::MAX],
                alone: false,
            })
            .collect();
        if alone {
            crate::prepare::planner::runs::mark_alone(
                &mut glyphs,
                &mut Meter::new(usize::MAX, "test bytes"),
            )
            .unwrap();
        }
        let union = bounds
            .iter()
            .fold(DeviceRect::default(), |a, &b| crate::plan::union(a, b));
        let kind = ItemKind::Run(Run {
            mark: mark(None),
            glyphs,
        });
        self.push(group, union, kind);
    }
}

#[test]
fn runs_store_as_one_item_per_placement_with_holes_in_coverage() {
    let mut seed = 17_u32;
    let mut next = |bound: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 8) % bound
    };
    let bounds: Vec<DeviceRect> = (0..40)
        .map(|_| {
            let (x, y) = (next(27), next(13));
            DeviceRect {
                x0: x,
                y0: y,
                x1: x + 2 + next(5),
                y1: y + 2 + next(5),
            }
        })
        .collect();
    let styles = [
        opaque(),
        Compositing {
            alpha: 0.6,
            ..opaque()
        },
        Compositing {
            blend: BlendMode::Multiply,
            ..opaque()
        },
    ];
    let color = [0.8, 0.25, 0.1];
    for format in [Format::F64, Format::F32, Format::N16] {
        for style in styles {
            if format != Format::F64 && style.blend != BlendMode::Normal {
                continue;
            }
            for band_height in [1, 4, 7, 32] {
                let render = |run: Option<bool>, knockout: bool| {
                    let mut f = Fixture::new(32, 20, RGB, format);
                    let back = DeviceRect {
                        x0: 0,
                        y0: 0,
                        x1: 32,
                        y1: 20,
                    };
                    f.glyphs(0, &[back], (&[0.1, 0.5, 0.9], opaque()), None);
                    if knockout {
                        let group = f.group(GroupPlan {
                            bounds: back,
                            items: Vec::new(),
                            order: Vec::new(),
                            layout: RGB,
                            format: Format::F64,
                            knockout: true,
                            start: Start::Backdrop,
                        });
                        f.glyphs(group, &bounds, (&color, style), run);
                        f.use_group(0, group, opaque(), None);
                    } else {
                        f.glyphs(0, &bounds, (&color, style), run);
                    }
                    f.render(band_height)
                };
                for knockout in [false, true] {
                    let expected = render(None, knockout);
                    for alone in [true, false] {
                        assert!(
                            render(Some(alone), knockout) == expected,
                            "{format:?} {style:?} band height {band_height} knockout {knockout} alone {alone}"
                        );
                    }
                }
            }
        }
    }
}

/// Rows stored as the rasterizer emits them change the page as the same
/// rows collected in a band coverage and stored span by span, and mark at
/// least those spans' columns: N16 words, N16 dynamic stores and F32 levels
/// at a constant alpha.
#[test]
fn emitted_rows_store_as_band_coverage_spans() {
    use super::{Dynamic, Levels};
    use crate::kernel::{self, Colors};
    #[expect(clippy::cast_possible_truncation, reason = "random words")]
    let word = |next: &mut dyn FnMut() -> u64| [0; 4].map(|_: u16| next() as u16);
    let words = [1_000, 20_000, 40_000, 65_535];
    emitted_rows_store_as((words, [0; 4]), word, |pixels, cover| {
        kernel::store_n16(pixels, words, cover.coverage());
    });
    let color = [0.1, 0.6, 0.85];
    for constant in [1.0, 0.6, 0.0] {
        // The opacities a span's coverage and constant alpha give it.
        let alpha = |cover: super::Cover<'_>| -> Vec<f64> {
            let compositing = Compositing {
                alpha: constant,
                blend: BlendMode::Normal,
                alpha_is_shape: false,
                mask: None,
                overprint: 0,
            };
            let (mut alpha, mut shape) = (vec![0.0; cover.len()], vec![0.0; cover.len()]);
            super::opacity((None, None), cover.coverage(), &mut alpha, &mut shape);
            super::apply(&compositing, None, &mut alpha, &mut shape);
            alpha
        };
        let colors = Colors::Constant(&color);
        let dynamic = Dynamic(kernel::targets(&color, 65_535.0), constant);
        emitted_rows_store_as((dynamic, [0; 4]), word, |pixels, cover| {
            kernel::dynamic_n16(pixels, &colors, &alpha(cover));
        });
        #[expect(clippy::cast_precision_loss, reason = "random levels")]
        emitted_rows_store_as(
            (Levels(kernel::levels(&color), constant), [0.0; 4]),
            |next| [0.0; 4].map(|_: f32| (next() % 255_001) as f32 / 1_000.0),
            |pixels, cover| kernel::store_f32(pixels, &colors, &alpha(cover)),
        );
    }
}

/// Stores of the background colour change nothing where no store reached
/// a pixel, which holds the background already: only stored columns take
/// them, and the rest stay unmarked for output to write as background.
#[test]
fn background_stores_reach_only_stored_columns() {
    use pagura_raster::RowOut;
    let rect = DeviceRect {
        x0: 2,
        y0: 0,
        x1: 12,
        y1: 2,
    };
    let white = [65_535; 4];
    let mut page = vec![white; 20];
    page[13] = [0, 0, 0, 65_535];
    let mut meter = Meter::new(usize::MAX, "budget");
    let mut extents = Extents::new(2, &mut meter).unwrap();
    extents.mark(1, 3, 4);
    let mut out = StoreOut::new(&mut page, Some(&mut extents), rect, (white, white));
    out.push_row(0);
    out.put_run(0, 14, u8::MAX);
    out.push_row(1);
    out.put_row(4, &[128; 6]);
    assert_eq!(out.stored, 1);
    let mut expected = vec![white; 20];
    // floor((65535·128 + 0·127 + 127) / 255)
    expected[13] = [32_896, 32_896, 32_896, 65_535];
    assert_eq!(page, expected);
    assert_eq!((extents.take(0), extents.take(1)), (None, Some((3, 4))));
}

fn emitted_rows_store_as<S: super::Stores<Pixel: std::fmt::Debug>>(
    (color, background): (S, S::Pixel),
    mut pixel: impl FnMut(&mut dyn FnMut() -> u64) -> S::Pixel,
    store: impl Fn(&mut [S::Pixel], super::Cover<'_>),
) {
    use pagura_raster::{BandCoverage, RowOut};
    let rect = DeviceRect {
        x0: 3,
        y0: 10,
        x1: 43,
        y1: 16,
    };
    let (width, rows) = (40_usize, 6_usize);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut meter = Meter::new(usize::MAX, "budget");
    for case in 0..200 {
        let initial: Vec<S::Pixel> = (0..width * rows).map(|_| pixel(&mut next)).collect();
        let (mut direct, mut spans) = (initial.clone(), initial);
        let mut emitted = Extents::new(rows, &mut meter).unwrap();
        let mut collected = Extents::new(rows, &mut meter).unwrap();
        let mut coverage = BandCoverage::default();
        coverage.clear();
        // Random pixels hold no background: every pixel takes its stores.
        let mut out = StoreOut::new(&mut direct, Some(&mut emitted), rect, (color, background));
        // Rows from above the frame to below it, with gaps, full and
        // partial runs and cells, some reaching past its sides.
        for y in 8..18 {
            out.push_row(y);
            coverage.push_row(y);
            let mut x = u32::try_from(next() % 6).unwrap();
            while x < 50 {
                let len = 1 + u32::try_from(next() % 8).unwrap();
                #[expect(clippy::cast_possible_truncation, reason = "random bytes")]
                match next() % 4 {
                    0 => {}
                    1 => {
                        out.put_run(x, len, u8::MAX);
                        coverage.put_run(x, len, u8::MAX);
                    }
                    2 => {
                        let value = 1 + (next() % 254) as u8;
                        out.put_run(x, len, value);
                        coverage.put_run(x, len, value);
                    }
                    _ => {
                        let values: Vec<u8> = (0..len)
                            .map(|_| match next() % 4 {
                                0 => 0,
                                1 => u8::MAX,
                                _ => next() as u8,
                            })
                            .collect();
                        out.put_row(x, &values);
                        coverage.put_row(x, &values);
                    }
                }
                x += len;
            }
        }
        coverage.finish();
        each_span(&coverage, rect, |(x, y), cover| {
            let row = (y - rect.y0) as usize;
            let column = x - rect.x0;
            let start = row * width + column as usize;
            store(&mut spans[start..start + cover.len()], cover);
            collected.mark(row, column, column + u32::try_from(cover.len()).unwrap());
        });
        assert_eq!(direct, spans, "{case}");
        for row in 0..rows {
            if let Some((lo, hi)) = collected.take(row) {
                let (a, b) = emitted.take(row).unwrap();
                assert!(a <= lo && hi <= b, "{case} row {row}");
            }
        }
    }
}

/// A row of a frame backdrop holds, pixel by pixel, the colour and alpha
/// [`Read::pixel`] gives inside the frame's rect and zeros elsewhere: every
/// buffer format, layouts with fewer and more channels than the source, and
/// rows that start left of, inside and right of the rect.
#[test]
fn backdrop_rows_read_as_their_pixels_do() {
    use super::{Backdrop, Buffer, Frame, PageBuffer, Read};
    let unit = |i: usize| f64::from(u32::try_from(i * 37 % 101).unwrap()) / 101.0;
    let rect = DeviceRect {
        x0: 3,
        y0: 2,
        x1: 9,
        y1: 5,
    };
    let layout = |process, spots| Layout { process, spots };
    let (gray, rgb, cmyk) = (
        layout(Process::Gray, 0),
        layout(Process::Rgb, 0),
        layout(Process::Cmyk, 1),
    );
    let pixels = 18;
    let wide: Vec<f64> = (0..pixels * cmyk.stride() + 7).map(unit).collect();
    let levels: Vec<[f32; 4]> = (0..pixels + 3)
        .map(|i| [0, 1, 2, 3].map(|c| f32::from(u16::try_from((i * 4 + c) * 29 % 256).unwrap())))
        .collect();
    let pages = [
        (
            PageBuffer::F64((0..pixels * rgb.stride()).map(unit).collect()),
            rgb,
        ),
        (
            PageBuffer::N16(
                (0..pixels)
                    .map(|i| {
                        [0, 1, 2, 3].map(|c| u16::try_from((i * 4 + c) * 7919 % 65536).unwrap())
                    })
                    .collect(),
            ),
            rgb,
        ),
        (PageBuffer::F32(levels.clone()), rgb),
    ];
    for (page, page_layout) in &pages {
        let read = Read {
            page,
            wide: &wide,
            levels: &levels,
        };
        for (buffer, source) in [
            (Buffer::Page, *page_layout),
            (Buffer::Wide(5), cmyk),
            (Buffer::Wide(0), gray),
            (Buffer::Levels(2), rgb),
        ] {
            let backdrop = Backdrop::Frame {
                buffer,
                rect,
                layout: source,
            };
            let frame = Frame {
                group: 0,
                buffer,
                rect,
                layout: source,
                knockout: false,
                initial: Backdrop::Transparent,
                started: None,
            };
            for target in [gray, rgb, cmyk] {
                let (n, stride) = (target.channels(), target.stride());
                for ((x, len), y) in [(0, 14), (5, 2), (8, 4), (10, 3)]
                    .into_iter()
                    .flat_map(|span| (1..6).map(move |y| (span, y)))
                {
                    let mut row = vec![f64::NAN; len * stride];
                    backdrop.row(read, (x, y), target, &mut row);
                    let mut expected = vec![0.0; len * stride];
                    for (column, pixel) in (x..).zip(expected.chunks_exact_mut(stride)) {
                        if let Some(index) = frame.index(column, y) {
                            let colors = &mut pixel[..n.min(source.channels())];
                            pixel[n] = read.pixel(buffer, index, source, colors);
                        }
                    }
                    let bits = |v: &[f64]| v.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                    assert_eq!(
                        bits(&row),
                        bits(&expected),
                        "{buffer:?} {source:?} into {target:?} at ({x}, {y}) × {len}"
                    );
                }
            }
        }
    }
}
