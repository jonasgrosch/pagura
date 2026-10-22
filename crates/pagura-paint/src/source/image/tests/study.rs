//! The sampler study ([`super::super::proto`]) against the sampler family,
//! on generated images and on the images of the slowest benchmark pages,
//! and against the reference renderer. Timing runs in release:
//!
//! ```text
//! cargo test --release -p pagura-paint --lib study_page_timing -- --ignored --nocapture
//! ```

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::format_push_string,
    clippy::manual_midpoint,
    clippy::needless_range_loop,
    clippy::too_many_lines,
    reason = "a measured study"
)]

use super::{Case, MaskUse, base, bytes, case, converter, image, placements, sampler, window};
use crate::SpanColors;
use crate::source::PaintContext;
use crate::source::image::proto::{Placement, Proto, Texeler, Values};
use crate::source::image::texels::{Colors, Opacity};
use crate::source::image::{
    Description, ImageSampler, MaskDescription, Orientation, colors, decode, decode_request,
    placement, planes_suit, rgb_target,
};
use pagura_color::{ColorSpace, Conversion, Converter, Process, Target};
use pagura_core::{Limits, Meter, Store};
use pagura_image::{Decoded, Layout, Reduce, Samples};
use pagura_interpret::{Composite, ImageMask, Node};
use pagura_raster::{DeviceRect, Point, Transform};
use std::sync::Arc;
use std::time::Instant;

/// The colours of a case's samples for a study sampler.
fn study_colors(case: &Case, image: &pagura_image::DecodedImage, target: usize) -> Colors {
    let description = Description {
        space: case.space.as_ref(),
        decode: &case.decode,
        key: None,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let space = case.space.as_ref().unwrap();
    colors(&description, image, None, converter(base(space), target).0).unwrap()
}

/// Whether a case's texels are affine codes into `target` channels.
fn codes(case: &Case, target: usize) -> Option<Vec<usize>> {
    let unit = case.decode.iter().all(|&d| d == [0.0, 1.0]);
    match (case.space.as_ref()?, target, unit) {
        (ColorSpace::DeviceRgb, 3, true) => Some(vec![0, 1, 2]),
        (ColorSpace::DeviceGray, 3, true) => Some(vec![0, 0, 0]),
        _ => None,
    }
}

/// The study sampler of `case` under `unit_to_device`.
fn study<'c>(
    case: &Case,
    unit_to_device: Transform,
    target: usize,
    colors: &'c Colors,
) -> Proto<'c> {
    let inverse = unit_to_device.inverse().unwrap();
    let orientation = Orientation::of(unit_to_device);
    let turn = |image: &pagura_image::DecodedImage| match orientation {
        Orientation::Rows => image.clone(),
        Orientation::Transposed => image.transposed(&Limits::default()).unwrap(),
    };
    let image = turn(&case.image);
    let (x0, _, x1, _) = window(unit_to_device);
    let geometry = placement(&image, inverse, orientation).unwrap();
    let values = match codes(case, target) {
        Some(component) => Values::Codes {
            component,
            maximum: 255.0,
        },
        None => Values::Texels(Texeler::new(colors, usize::from(image.components))),
    };
    let mask = case.mask.as_ref().map(|(mask, decode, _)| {
        let mask = turn(mask);
        let geometry = placement(&mask, inverse, orientation).unwrap();
        let opacity = Opacity::new(&mask, *decode);
        (
            Arc::new(Decoded::from(mask)),
            Placement::new(geometry, (x0, x1)),
            opacity,
        )
    });
    let mask = mask
        .as_ref()
        .map(|(image, placement, opacity)| (Arc::clone(image), clone(placement), opacity));
    Proto::new(
        Arc::new(Decoded::from(image)),
        Placement::new(geometry, (x0, x1)),
        values,
        target,
        mask,
    )
}

fn clone(placement: &Placement) -> Placement {
    Placement {
        geometry: placement.geometry.clone(),
        x0: placement.x0,
        feet: placement.feet.clone(),
        upright: placement.upright,
        sliver: placement.sliver,
        rows: placement.rows,
        edges: placement.edges,
    }
}

/// The largest difference between two spans' premultiplied channels and
/// alphas, in levels, and the channels whose N16 words differ.
fn compare(old: &SpanColors, new: &SpanColors) -> (f64, usize) {
    let n = old.channels;
    let (mut worst, mut words) = (0.0f64, 0);
    let word = |v: f64| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    for i in 0..new.colors.len() / n.max(1) {
        let a0 = old.alpha.get(i).copied().unwrap_or(1.0);
        let a1 = new.alpha.get(i).copied().unwrap_or(1.0);
        worst = worst.max((a0 - a1).abs() * 255.0);
        words += usize::from(word(a0) != word(a1));
        for c in 0..n {
            let (v0, v1) = (old.colors[i * n + c], new.colors[i * n + c]);
            worst = worst.max((v0 * a0 - v1 * a1).abs() * 255.0);
            words += usize::from(word(v0) != word(v1));
        }
    }
    (worst, words)
}

/// The area of device pixel (`x`, `y`) inside the unit square placed by
/// `unit_to_device`, by clipping the pixel against the square's sides.
fn coverage(unit_to_device: Transform, (x, y): (u32, u32)) -> f64 {
    let inverse = unit_to_device.inverse().unwrap();
    let (x, y) = (f64::from(x), f64::from(y));
    let mut polygon: Vec<Point> = [(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)]
        .iter()
        .map(|&(x, y)| Point::new(x, y) * inverse)
        .collect();
    for (axis, bound, sign) in [(0, 0.0, 1.0), (0, 1.0, -1.0), (1, 0.0, 1.0), (1, 1.0, -1.0)] {
        let side = |p: Point| sign * (if axis == 0 { p.x } else { p.y } - bound);
        let mut next = Vec::new();
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            let (da, db) = (side(a), side(b));
            if da >= 0.0 {
                next.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                next.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        }
        polygon = next;
        if polygon.is_empty() {
            return 0.0;
        }
    }
    let twice: f64 = (0..polygon.len())
        .map(|i| {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            a.x * b.y - a.y * b.x
        })
        .sum();
    (twice.abs() * 0.5) * unit_to_device.determinant().abs()
}

/// Agreement of the study with the sampler family on one case.
fn agree(case: &Case, target: usize) -> (f64, usize) {
    let colors = study_colors(case, &case.image, target);
    let (mut worst, mut words) = (0.0f64, 0);
    for unit_to_device in placements(case.image.width, case.image.height) {
        let old = sampler(case, unit_to_device, target);
        let mut new = study(case, unit_to_device, target, &colors);
        let (x0, y0, x1, y1) = window(unit_to_device);
        let len = (x1 - x0) as usize;
        let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
        let (mut placement_worst, mut placement_words) = (0.0f64, 0);
        for y in y0..y1 {
            old.span(x0, y, len, &mut a);
            new.span(x0, y, len, &mut b);
            // Pixels whose coverage byte is 0 store nothing.
            for span in [&mut a, &mut b] {
                if span.alpha.is_empty() {
                    span.alpha.resize(len, 1.0);
                }
            }
            for (i, x) in (x0..x1).enumerate() {
                if coverage(unit_to_device, (x, y)) * 255.0 < 0.5 {
                    let n = a.channels;
                    a.colors[i * n..i * n + n].fill(0.0);
                    b.colors[i * n..i * n + n].fill(0.0);
                    if let Some(alpha) = a.alpha.get_mut(i) {
                        *alpha = 0.0;
                    }
                    b.alpha[i] = 0.0;
                }
            }
            let (w, d) = compare(&a, &b);
            if w >= 1e-9 {
                for i in 0..len {
                    let n = a.channels;
                    let (ca, cb) = (&a.colors[i * n..i * n + n], &b.colors[i * n..i * n + n]);
                    if ca != cb || a.alpha.get(i) != b.alpha.get(i) {
                        eprintln!(
                            "x {} old {ca:?} {:?} new {cb:?} {:?}",
                            x0 + i as u32,
                            a.alpha.get(i),
                            b.alpha.get(i)
                        );
                    }
                }
            }
            assert!(w < 1e-9, "{unit_to_device:?} row {y}: {w} levels");
            worst = worst.max(w);
            words += d;
            placement_worst = placement_worst.max(w);
            placement_words += d;
        }
        if std::env::var("PAGURA_STUDY_VERBOSE").is_ok() {
            eprintln!(
                "  {unit_to_device:?}: {placement_worst:.1e} levels, {placement_words} words"
            );
        }
    }
    (worst, words)
}

#[test]
fn study_agrees_with_the_sampler_family() {
    let rgb = case(image((37, 29), 3, 8, bytes(37 * 29 * 3, 8, 5)));
    let gray = case(image((23, 31), 1, 8, bytes(23 * 31, 8, 9)));
    let mut masked = rgb.clone();
    masked.mask = Some((
        image((37, 29), 1, 8, bytes(37 * 29, 8, 21)),
        [0.0, 1.0],
        MaskUse::Soft(None),
    ));
    let mut report = String::new();
    for (name, case) in [("rgb", &rgb), ("gray", &gray), ("masked", &masked)] {
        for target in [3, 4] {
            let (worst, words) = agree(case, target);
            report += &format!("{name} into {target}: {worst:.2e} levels, {words} words\n");
        }
    }
    eprintln!("{report}");
}

/// An image the page draws: the node's image, its unit square to device
/// pixels, its device bounds, and the target of its group.
struct Drawn {
    image: Arc<pagura_interpret::Image>,
    unit_to_device: Transform,
    rect: DeviceRect,
    target: Target,
    composite: Arc<Composite>,
}

fn drawn(name: &str, dpi: f64) -> (Vec<Drawn>, Arc<pagura_document::Document>) {
    let pages = super::timing::pages(name);
    let (_, document, index) = pages.into_iter().next().unwrap();
    let page = document.page(index).unwrap();
    let scene = pagura_interpret::Interpreter::new(&document)
        .scene(page)
        .unwrap();
    let device = scene.page.device_transform(dpi / 72.0);
    let (width, height) = scene.page.device_size(dpi / 72.0).unwrap();
    let process = scene
        .groups
        .first()
        .and_then(|g| g.attributes.space)
        .unwrap_or(Process::Rgb);
    let mut out = Vec::new();
    for node in &scene.nodes {
        let Node::Image(node) = node else { continue };
        let unit_to_device = node.transform.then(device);
        let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
            .map(|(x, y)| Point::new(x, y) * unit_to_device);
        let lo = |f: fn(&Point) -> f64| corners.iter().map(f).fold(f64::INFINITY, f64::min);
        let hi = |f: fn(&Point) -> f64| corners.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as u32;
        let rect = DeviceRect {
            x0: clamp(lo(|p| p.x).floor(), width),
            y0: clamp(lo(|p| p.y).floor(), height),
            x1: clamp(hi(|p| p.x).ceil(), width),
            y1: clamp(hi(|p| p.y).ceil(), height),
        };
        if rect.x0 >= rect.x1 || rect.y0 >= rect.y1 {
            continue;
        }
        out.push(Drawn {
            image: Arc::clone(&node.image),
            unit_to_device,
            rect,
            target: Target::new(process),
            composite: Arc::clone(&scene.states[node.mark.state.index()]),
        });
    }
    (out, document)
}

/// What the study needs to sample one drawn image, decoded through `store`.
struct Prepared {
    image: Arc<Decoded>,
    mask: Option<(Arc<Decoded>, [f64; 2])>,
    colors: Colors,
    codes: Option<Vec<usize>>,
    orientation: Orientation,
}

fn prepare(drawn: &Drawn, context: &PaintContext<'_>) -> Prepared {
    let image = drawn.image.as_ref();
    let plain = matches!(image.mask, ImageMask::None) && !image.interpolate;
    let planes = planes_suit(
        (image.space.as_deref(), &image.decode, plain),
        drawn.unit_to_device,
        context,
    );
    let orientation = Orientation::of(drawn.unit_to_device);
    let decoded = decode(
        &image.source,
        drawn.unit_to_device,
        (planes, orientation),
        context,
    )
    .unwrap();
    let mask = match &image.mask {
        ImageMask::Soft { mask, .. } => Some((
            decode_request(
                &mask.source,
                (Reduce::Full, Layout::Native, orientation),
                context,
            )
            .unwrap(),
            mask.decode.first().copied().unwrap_or([0.0, 1.0]),
        )),
        _ => None,
    };
    let space = image.space.as_deref().unwrap();
    let convert: Box<dyn super::super::texels::Convert> =
        Box::new(Converter::new(space, context.target, &Conversion::default()).unwrap());
    let description = Description {
        space: Some(space),
        decode: &image.decode,
        key: None,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let colors = colors(&description, &decoded.image, None, convert).unwrap();
    let unit = image.decode.iter().all(|&d| d == [0.0, 1.0]);
    let codes = match (space, context.target.channels(), unit) {
        (ColorSpace::DeviceRgb, 3, true) if rgb_target(space, context) => Some(vec![0, 1, 2]),
        (ColorSpace::DeviceGray, 3, true) => Some(vec![0, 0, 0]),
        _ => None,
    };
    Prepared {
        image: decoded,
        mask,
        colors,
        codes,
        orientation,
    }
}

fn proto<'c>(drawn: &Drawn, prepared: &'c Prepared, opacity: Option<&Opacity>) -> Proto<'c> {
    let inverse = drawn.unit_to_device.inverse().unwrap();
    let columns = (drawn.rect.x0, drawn.rect.x1);
    let geometry = placement(&prepared.image.image, inverse, prepared.orientation).unwrap();
    let values = match &prepared.codes {
        Some(component) => Values::Codes {
            component: component.clone(),
            maximum: f64::from((1u16 << prepared.image.image.bits_per_component) - 1),
        },
        None => Values::Texels(Texeler::new(
            &prepared.colors,
            usize::from(prepared.image.image.components),
        )),
    };
    let mask = prepared
        .mask
        .as_ref()
        .zip(opacity)
        .map(|((image, _), opacity)| {
            let geometry = placement(&image.image, inverse, prepared.orientation).unwrap();
            (
                Arc::clone(image),
                Placement::new(geometry, columns),
                opacity,
            )
        });
    let fuse = mask.as_ref().is_some_and(|(image, _, _)| {
        (image.image.width, image.image.height)
            == (prepared.image.image.width, prepared.image.image.height)
    }) && prepared.codes.is_none()
        && std::env::var("PAGURA_STUDY_SEPARATE_MASK").is_err();
    let mut proto = Proto::new(
        Arc::clone(&prepared.image),
        Placement::new(geometry, columns),
        values,
        drawn.target.channels(),
        mask,
    );
    proto.fuse = fuse;
    proto
}

/// Stores a span as the executor's narrow page does at full coverage.
fn store(row: &mut [[u16; 4]], span: &SpanColors) {
    let colors = crate::kernel::Colors::Strided {
        values: &span.colors,
        stride: span.channels,
    };
    if span.channels == 3 && span.alpha.iter().all(|&a| a == 1.0) {
        crate::kernel::overwrite_n16(row, &span.colors);
    } else if span.alpha.is_empty() {
        crate::kernel::dynamic_n16(row, &colors, &vec![1.0; row.len()]);
    } else {
        crate::kernel::dynamic_n16(row, &colors, &span.alpha);
    }
}

fn best(runs: usize, mut run: impl FnMut()) -> f64 {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

/// The slowest image pages of the scoreboard: the study against today's
/// sampler, sampling and storing every row of each image's bounds into an
/// N16 row, paired and interleaved; and agreement of the two.
#[test]
#[ignore = "timing; needs the benchmark corpus"]
fn study_page_timing() {
    let runs = super::timing::setting("PAGURA_IMAGE_RUNS", "5")
        .parse()
        .unwrap_or(5);
    let pages = super::timing::setting(
        "PAGURA_STUDY_PAGES",
        "0000651.pdf:1@72,0000721.pdf:1@72,0000095.pdf:1@300,0000200.pdf:2@72,0000200.pdf:2@300,0000008.pdf:0@300",
    );
    eprintln!(
        "page | old ms | new ms | new words ms | old/new | conversions old→new | worst levels | words differing"
    );
    for entry in pages.split(',') {
        let (name, dpi) = entry.split_once('@').unwrap();
        let dpi: f64 = dpi.parse().unwrap();
        let (images, document) = drawn(name, dpi);
        let limits = document.limits();
        for drawn in &images {
            if drawn.image.space.is_none() {
                continue;
            }
            let store_ = Store::new(usize::MAX);
            let context = PaintContext {
                transform: Transform::IDENTITY,
                target: &drawn.target,
                composite: &drawn.composite,
                store: &store_,
                limits: &limits,
                reduced_image_decode: false,
                region: drawn.rect,
            };
            let rect = drawn.rect;
            let len = (rect.x1 - rect.x0) as usize;
            let pixels = len * (rect.y1 - rect.y0) as usize;
            let prepared = prepare(drawn, &context);
            let opacity = prepared
                .mask
                .as_ref()
                .map(|(mask, decode)| Opacity::new(&mask.image, *decode));
            let old_sampler = || {
                let mut work = Meter::new(usize::MAX, "work");
                ImageSampler::prepare(
                    &drawn.image,
                    drawn.unit_to_device,
                    &context,
                    pixels,
                    &mut work,
                )
                .unwrap()
            };
            // Agreement, and the conversions of one pass.
            let old = old_sampler();
            let mut new = proto(drawn, &prepared, opacity.as_ref());
            let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
            let (mut worst, mut differing) = (0.0f64, 0);
            for y in rect.y0..rect.y1 {
                old.span(rect.x0, y, len, &mut a);
                new.span(rect.x0, y, len, &mut b);
                // Pixels whose coverage byte is 0 store nothing.
                for span in [&mut a, &mut b] {
                    if span.alpha.is_empty() {
                        span.alpha.resize(len, 1.0);
                    }
                }
                let edge = y < rect.y0 + 2 || y + 2 >= rect.y1 || drawn.unit_to_device.b != 0.0;
                for (i, x) in (rect.x0..rect.x1).enumerate() {
                    let near = edge || i < 2 || i + 2 >= len;
                    if near && coverage(drawn.unit_to_device, (x, y)) * 255.0 < 0.5 {
                        let n = a.channels;
                        a.colors[i * n..i * n + n].fill(0.0);
                        b.colors[i * n..i * n + n].fill(0.0);
                        a.alpha[i] = 0.0;
                        b.alpha[i] = 0.0;
                    }
                }
                let (w, d) = compare(&a, &b);
                if w > 1e-6 && std::env::var("PAGURA_STUDY_VERBOSE").is_ok() {
                    let n = a.channels;
                    for i in 0..len {
                        let (ca, cb) = (&a.colors[i * n..i * n + n], &b.colors[i * n..i * n + n]);
                        let (aa, ab) = (
                            a.alpha.get(i).copied().unwrap_or(1.0),
                            b.alpha.get(i).copied().unwrap_or(1.0),
                        );
                        if ca
                            .iter()
                            .zip(cb)
                            .any(|(x, y)| (x * aa - y * ab).abs() > 1e-9)
                            || (aa - ab).abs() > 1e-9
                        {
                            let x = rect.x0 + i as u32;
                            let device = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)
                                * drawn.unit_to_device.inverse().unwrap();
                            eprintln!("({x}, {y}) unit {device:?} old {ca:?} {aa} new {cb:?} {ab}");
                        }
                    }
                }
                worst = worst.max(w);
                differing += d;
            }
            let conversions = new.conversions();
            let mut row = vec![[0u16; 4]; len];
            let mut old_ms = f64::INFINITY;
            let mut new_ms = f64::INFINITY;
            let mut words_ms = f64::INFINITY;
            let only_new = std::env::var("PAGURA_STUDY_ONLY_NEW").is_ok();
            for _ in 0..runs {
                if !only_new {
                    old_ms = old_ms.min(best(1, || {
                        let sampler = old_sampler();
                        let mut span = SpanColors::default();
                        for y in rect.y0..rect.y1 {
                            sampler.span(rect.x0, y, len, &mut span);
                            store(&mut row, &span);
                        }
                        std::hint::black_box(&row);
                    }));
                }
                new_ms = new_ms.min(best(1, || {
                    let mut sampler = proto(drawn, &prepared, opacity.as_ref());
                    let mut span = SpanColors::default();
                    for y in rect.y0..rect.y1 {
                        sampler.span(rect.x0, y, len, &mut span);
                        store(&mut row, &span);
                    }
                    std::hint::black_box(&row);
                }));
                words_ms = words_ms.min(best(1, || {
                    let mut sampler = proto(drawn, &prepared, opacity.as_ref());
                    let mut span = SpanColors::default();
                    for y in rect.y0..rect.y1 {
                        if !sampler.words(rect.x0, y, &mut row) {
                            sampler.span(rect.x0, y, len, &mut span);
                            store(&mut row, &span);
                        }
                    }
                    std::hint::black_box(&row);
                }));
            }
            eprintln!(
                "{entry} {}x{} {:?} | {old_ms:.2} | {new_ms:.2} | {words_ms:.2} | {:.2}x | → {conversions} | {worst:.2e} | {differing}",
                prepared.image.image.width,
                prepared.image.image.height,
                drawn.target.process,
                old_ms / new_ms.min(words_ms),
            );
        }
    }
}

/// The study against the reference renderer on a window of fully covered
/// pixels of each image of the pages: within a millionth of a level.
#[test]
#[ignore = "needs the benchmark corpus"]
fn study_against_the_reference() {
    let pages = super::timing::setting(
        "PAGURA_STUDY_PAGES",
        "0000651.pdf:1@72,0000721.pdf:1@72,0000095.pdf:1@300,0000200.pdf:2@72,0000200.pdf:2@300,0000008.pdf:0@300",
    );
    for entry in pages.split(',') {
        let (name, dpi) = entry.split_once('@').unwrap();
        let dpi: f64 = dpi.parse().unwrap();
        let (images, document) = drawn(name, dpi);
        let limits = document.limits();
        for drawn in &images {
            if drawn.image.space.is_none() {
                continue;
            }
            // The reference page is RGB.
            let drawn = &Drawn {
                image: Arc::clone(&drawn.image),
                unit_to_device: drawn.unit_to_device,
                rect: drawn.rect,
                target: Target::new(Process::Rgb),
                composite: Arc::new(Composite::default()),
            };
            let store_ = Store::new(usize::MAX);
            let context = PaintContext {
                transform: Transform::IDENTITY,
                target: &drawn.target,
                composite: &drawn.composite,
                store: &store_,
                limits: &limits,
                reduced_image_decode: false,
                region: drawn.rect,
            };
            let prepared = prepare(drawn, &context);
            let opacity = prepared
                .mask
                .as_ref()
                .map(|(mask, decode)| Opacity::new(&mask.image, *decode));
            let mut new = proto(drawn, &prepared, opacity.as_ref());
            let rect = drawn.rect;
            let (cx, cy) = ((rect.x0 + rect.x1) / 2, (rect.y0 + rect.y1) / 2);
            let window = DeviceRect {
                x0: cx - 24,
                y0: cy - 24,
                x1: cx + 24,
                y1: cy + 24,
            };
            let mut build =
                crate::testing::Build::new(f64::from(rect.x1) + 2.0, f64::from(rect.y1) + 2.0);
            let root = build.scene.root;
            build.node(
                root,
                Node::Image(pagura_interpret::ImageNode {
                    image: Arc::clone(&drawn.image),
                    transform: drawn.unit_to_device,
                    stencil: None,
                    mark: crate::testing::mark(pagura_interpret::StateId(0)),
                }),
            );
            let mut request = pagura_reference::Request::new(Transform::IDENTITY, window);
            request.background = pagura_raster::Rgba8::WHITE;
            let reference = pagura_reference::render(&build.scene, &request, &limits).unwrap();
            let len = (window.x1 - window.x0) as usize;
            let mut work = Meter::new(usize::MAX, "work");
            let old = ImageSampler::prepare(
                &drawn.image,
                drawn.unit_to_device,
                &context,
                len * len,
                &mut work,
            )
            .unwrap();
            let mut span = SpanColors::default();
            let (mut worst, mut worst_old) = (0.0f64, 0.0f64);
            for y in window.y0..window.y1 {
                for (sampler, worst) in [(None, &mut worst), (Some(&old), &mut worst_old)] {
                    match sampler {
                        None => new.span(window.x0, y, len, &mut span),
                        Some(old) => old.span(window.x0, y, len, &mut span),
                    }
                    for (i, x) in (window.x0..window.x1).enumerate() {
                        if coverage(drawn.unit_to_device, (x, y)) < 1.0 - 1e-12 {
                            continue;
                        }
                        let exact = reference.exact(x, y).unwrap();
                        let alpha = span.alpha.get(i).copied().unwrap_or(1.0);
                        for c in 0..3 {
                            let value = span.colors[i * 3 + c] * alpha + (1.0 - alpha);
                            *worst = worst.max((value - exact[c]).abs() * 255.0);
                        }
                    }
                }
            }
            eprintln!(
                "{entry} {:?}: against the reference, study {worst:.2e} levels, sampler {worst_old:.2e}",
                new.placement.geometry.mode()
            );
            assert!(worst < 1e-6, "{entry}: {worst} levels");
        }
    }
}

#[test]
fn quarter_turned_and_rotated_contours_match_polygon_areas() {
    // A contour integral of ones is the area of the footprint within the
    // image.
    let mut sums = Vec::new();
    let ones = vec![1u8; 9 * 7];
    let mut rows = crate::source::image::proto::Bytes {
        bytes: &ones,
        width: 9,
        lanes: 1,
    };
    let corners = [
        Point::new(-1.25, 0.5),
        Point::new(3.5, -0.75),
        Point::new(5.0, 4.2),
        Point::new(0.25, 5.45),
    ];
    let edges = [0, 1, 2, 3].map(|i| {
        let (p, q) = (corners[i], corners[(i + 1) % 4]);
        crate::source::image::proto::Edge {
            dx: q.x - p.x,
            dy: q.y - p.y,
            step_x: 1.0 / (q.x - p.x).abs(),
            step_y: 1.0 / (q.y - p.y).abs(),
        }
    });
    let (out, area) = crate::source::image::proto::contour::<1, _>(
        &mut rows,
        (9, 7),
        &corners,
        &edges,
        &mut sums,
    );
    assert!((area - out[0]).abs() < 1e-12, "{area} vs {}", out[0]);
    let _ = Samples::Bytes(Vec::new());
}
