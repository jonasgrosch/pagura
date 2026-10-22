//! The executor on hand-built plans, for what scenes cannot carry before
//! the paint sources and functions exist: varying paint with alpha,
//! transfer selection, spot channels with overprint preview, soft-mask
//! transfer functions, black generation, and narrow stores of group
//! results. Expected values are clause-11 formulas evaluated here.

use super::{Destination, Executor, Sizes, Tops, Workspace};
use crate::format::Format;
use crate::paint::{Convert, Curve, PaintSpans};
use crate::plan::{
    Compositing, Conversion, Cover, Glyph, GroupPlan, GroupUse, Item, ItemKind, Layout, Mark,
    MaskPlan, Output, Plan, Run, SpotPlan, Start,
};
use crate::prepare::Producer;
use crate::source::{Source, SpanColors};
use crate::testing::{Box4, rect_producer};
use pagura_color::{Painted, Process};
use pagura_core::Position;
use pagura_interpret::{BlendMode, MaskKind};
use pagura_raster::{Area, BandCoverage, DeviceRect, Pixmap, Rgba8};
use std::sync::Arc;

/// Colours that vary along a row, with alpha that varies too.
#[derive(Debug)]
struct Gradient {
    channels: usize,
    alpha: bool,
}

impl Gradient {
    fn color(x: u32, k: usize) -> f64 {
        (f64::from(x) / 16.0 + 0.1 * k as f64).fract()
    }

    fn alpha(x: u32, y: u32) -> f64 {
        f64::from((x + y) % 5 + 1) / 5.0
    }
}

impl PaintSpans for Gradient {
    fn constant(&self) -> Option<&[f64]> {
        None
    }

    fn painted(&self) -> Painted {
        Painted::default()
    }

    fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        out.channels = self.channels;
        out.colors.clear();
        out.alpha.clear();
        for column in (x..).take(len) {
            out.colors
                .extend((0..self.channels).map(|k| Self::color(column, k)));
            if self.alpha {
                out.alpha.push(Self::alpha(column, y));
            }
        }
        out.painted = Painted((1 << self.channels) - 1);
    }
}

#[derive(Debug)]
struct Square;

impl Curve for Square {
    fn map(&self, x: f64) -> f64 {
        x * x
    }
    fn cost(&self) -> usize {
        1
    }
}

#[derive(Debug)]
struct Scaled(f64);

impl Curve for Scaled {
    fn map(&self, x: f64) -> f64 {
        x * self.0
    }
    fn cost(&self) -> usize {
        1
    }
}

/// A spot ink with this CMYK composition per unit of tint.
#[derive(Debug)]
struct Ink([f64; 4]);

impl Convert for Ink {
    fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        for (out, ink) in output.iter_mut().zip(self.0) {
            *out = 1.0 - components[0] * ink;
        }
        Painted(0b1111)
    }
    fn cost(&self) -> usize {
        2
    }
}

fn solid(color: &[f64], painted: u64) -> Box<dyn PaintSpans> {
    Box::new(Source::Solid {
        color: color.into(),
        painted: Painted(painted),
    })
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
    shapes: Vec<Box4>,
    paints: Vec<Box<dyn PaintSpans>>,
    masks: Vec<MaskPlan>,
    conversions: Vec<Conversion>,
    output: Output,
    /// Shapes of [`Speckles`] coverage, used instead of `shapes` when set.
    speckled: Option<Vec<DeviceRect>>,
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
            shapes: Vec::new(),
            paints: Vec::new(),
            masks: Vec::new(),
            conversions: Vec::new(),
            output: Output {
                layout,
                background: [255; 4],
                spots: Vec::new(),
                transfers: Vec::new(),
            },
            speckled: None,
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
        paint: Box<dyn PaintSpans>,
        compositing: Compositing,
        select: u16,
    ) {
        let shape = u32::try_from(self.shapes.len()).unwrap();
        self.shapes.push(rect);
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
        let mut free = pagura_core::Meter::new(usize::MAX, "test rasterizer");
        let mut rasterizer = pagura_raster::BandRasterizer::new(area, &mut free).unwrap();
        let producer: Box<dyn Producer> = match self.speckled {
            Some(shapes) => Box::new(Speckles { area, shapes }),
            None => rect_producer(area, self.shapes, Vec::new()),
        };
        let mut target =
            Pixmap::new(self.width, self.height, Rgba8::TRANSPARENT, usize::MAX).unwrap();
        workspace.restart();
        for band in area.bands() {
            let target = Destination::Pixmap(&mut target);
            let executing = (&plan, &mut workspace, (producer.as_ref(), &mut rasterizer));
            Executor::band(executing, band, target);
        }
        // Rows collected without a surface are the surface's rows.
        let mut rows = Vec::new();
        workspace.restart();
        for band in area.bands() {
            let target = Destination::Rows(&mut rows);
            let executing = (&plan, &mut workspace, (producer.as_ref(), &mut rasterizer));
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
            let gradient = Gradient {
                channels: 3,
                alpha: format != Format::N16,
            };
            f.mark(0, [0.0, 0.0, 12.0, 6.0], Box::new(gradient), opaque(), 0);
            let bytes = f.render(band_height);
            for y in 0..6 {
                for x in 0..12 {
                    let a = if format == Format::N16 {
                        1.0
                    } else {
                        Gradient::alpha(x, y)
                    };
                    let c = [0, 1, 2].map(|k| Gradient::color(x, k) * a + (1.0 - a));
                    assert_close(&bytes, 12, (x, y), [c[0], c[1], c[2], 1.0]);
                }
            }
        }
    }
}

#[test]
fn the_last_opaque_mark_selects_its_transfer() {
    let mut f = Fixture::new(4, 2, RGB, Format::F64);
    f.output.transfers = vec![[
        Some(Arc::new(Square) as Arc<dyn Curve>),
        None,
        Some(Arc::new(Square)),
    ]];
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
            convert: Box::new(Ink([0.0, 0.8, 0.3, 0.0])),
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
        transfer: Some(Arc::new(Square)),
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
        black: Some(Arc::new(Scaled(0.5))),
        removal: Some(Arc::new(Scaled(0.0))),
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

/// Coverage with holes: within a shape's bounds each pixel is uncovered,
/// fully or partly covered by a hash of its place, and partial spans keep
/// single zero bytes inside.
#[derive(Debug)]
struct Speckles {
    area: Area,
    shapes: Vec<DeviceRect>,
}

fn speckle(shape: u32, x: u32, y: u32) -> u8 {
    let h = (shape.wrapping_mul(2_654_435_761) ^ x.wrapping_mul(40_503) ^ y.wrapping_mul(9_973))
        .wrapping_mul(2_246_822_519);
    match (h >> 13) % 6 {
        0 => 0,
        1 | 2 => 255,
        _ => u8::try_from((h >> 3) % 254 + 1).unwrap(),
    }
}

impl Producer for Speckles {
    fn rasterize(
        &self,
        _rasterizer: &mut pagura_raster::BandRasterizer,
        shape: Option<u32>,
        _clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) {
        out.clear();
        let mut cells = Vec::new();
        let Some((key, bounds)) = shape.and_then(|s| Some((s, *self.shapes.get(s as usize)?)))
        else {
            out.finish();
            return;
        };
        for y in self.area.band_rows(band) {
            if y < bounds.y0 || y >= bounds.y1 {
                continue;
            }
            out.push_row(y);
            let mut full: Option<(u32, u32)> = None;
            cells.clear();
            let mut start = 0;
            for x in bounds.x0..=bounds.x1 {
                let c = if x < bounds.x1 { speckle(key, x, y) } else { 0 };
                if c != 255
                    && let Some((at, len)) = full.take()
                {
                    out.push_full(at, len);
                }
                let gap = c == 0 && cells.last() == Some(&0);
                if (c == 255 || gap || x == bounds.x1) && !cells.is_empty() {
                    out.push_partial(start, &cells);
                    cells.clear();
                }
                if c == 255 {
                    full = Some(full.map_or((x, 1), |(at, len)| (at, len + 1)));
                } else if !gap && x < bounds.x1 {
                    if cells.is_empty() {
                        start = x;
                    }
                    cells.push(c);
                }
            }
        }
        out.finish();
    }

    fn rasterize_run(
        &self,
        rasterizer: &mut pagura_raster::BandRasterizer,
        shapes: &[u32],
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) -> usize {
        crate::testing::merge_run(shapes, out, |shape, one| {
            self.rasterize(rasterizer, Some(shape), clip, band, one);
        })
    }
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
        let shapes = self.speckled.get_or_insert_with(Vec::new);
        let first = u32::try_from(shapes.len()).unwrap();
        shapes.extend_from_slice(bounds);
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
