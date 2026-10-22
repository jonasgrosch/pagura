//! The executor on hand-built plans, for what scenes cannot carry before
//! the paint sources and functions exist: varying paint with alpha,
//! transfer selection, spot channels with overprint preview, soft-mask
//! transfer functions, black generation, and narrow stores of group
//! results. Expected values are clause-11 formulas evaluated here.

use super::{Destination, Executor, Sizes, Tops, Workspace};
use crate::format::Format;
use crate::plan::{
    Compositing, Conversion, Cover, Curve, Glyph, GroupPlan, GroupUse, Item, ItemKind, Layout,
    Mark, MaskPlan, Output, Plan, Run, SpotPlan, Start,
};
use crate::prepare::Shapes;
use crate::source::{PaintContext, Pixels, Source, SpanColors};
use crate::testing::Box4;
use pagura_color::{
    ColorSpace, Colorant, Conversion as ColorConversion, Converter, ExponentialFunction, Function,
    Painted, Process, Separation, Target,
};
use pagura_core::{Limits, Meter, Position, Store};
use pagura_interpret::{BlendMode, Composite, MaskKind, Shading, ShadingFunction, ShadingKind};
use pagura_raster::{Area, DeviceRect, PathBuilder, Pixmap, Point, Rgba8, Transform};
use std::sync::Arc;

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

/// `x × factor`.
fn scaled(factor: f64) -> Curve {
    Curve::new(&exponential(&[0.0], &[factor], 1.0)).unwrap()
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
    };
    let mut functions = Meter::new(usize::MAX, "test functions");
    let source = Source::shading(
        &shading,
        Transform::IDENTITY,
        &context,
        64 * 64,
        &mut functions,
    );
    let source = source.unwrap();
    assert!(matches!(source, Source::Shading(_)));
    source
}

/// The colours and alpha `paint` gives pixel `(x, y)`.
fn sample(paint: &Source, (x, y): (u32, u32)) -> (Vec<f64>, f64) {
    let mut out = SpanColors::default();
    let pixels = Pixels {
        x,
        y,
        len: 1,
        boundary: None,
    };
    paint.fill(pixels, &mut out, &mut Vec::new());
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
        let parts = crate::prepare::planner::Parts {
            groups,
            items: self.items,
            masks: self.masks,
            paints: self.paints,
            conversions: self.conversions,
            transfers: Vec::new(),
        };
        let rows = self.height.min(256) as usize;
        let stack: Tops = crate::prepare::planner::needs(&parts, rows);
        let selections = !self.output.transfers.is_empty();
        let plan = Plan {
            area,
            groups: parts.groups,
            items: parts.items,
            masks: parts.masks,
            paints: parts.paints,
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
        };
        let mut workspace = Workspace::new(&plan, plan.sizes, None).unwrap();
        let mut free = Meter::new(usize::MAX, "test rasterizer");
        let mut rasterizer = pagura_raster::BandRasterizer::new(area, &mut free).unwrap();
        let shapes: Shapes = crate::testing::path_shapes(area, &self.paths);
        let mut target =
            Pixmap::new(self.width, self.height, Rgba8::TRANSPARENT, usize::MAX).unwrap();
        workspace.restart();
        for band in area.bands() {
            let target = Destination::Pixmap(&mut target);
            let executing = (&plan, &mut workspace, (&shapes, &mut rasterizer));
            Executor::band(executing, band, target);
        }
        // Rows collected without a surface are the surface's rows.
        let mut rows = Vec::new();
        workspace.restart();
        for band in area.bands() {
            let target = Destination::Rows(&mut rows);
            let executing = (&plan, &mut workspace, (&shapes, &mut rasterizer));
            Executor::band(executing, band, target);
        }
        assert_eq!(rows, target.data());
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
    f.conversions.push(Conversion {
        from: Process::Rgb,
        to: Process::Cmyk,
        black: Some(scaled(0.5)),
        removal: Some(scaled(0.0)),
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
            crate::prepare::planner::runs::mark_alone(&mut glyphs);
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
