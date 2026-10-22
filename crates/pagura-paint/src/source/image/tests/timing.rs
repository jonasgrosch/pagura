//! Timing of a page's images through decode requests and the sampler
//! family, beside the old routes' whole-page renders. Run in release:
//!
//! ```text
//! cargo test --release -p pagura-paint --lib image_page_timing -- --ignored --nocapture
//! ```
//!
//! `PAGURA_IMAGE_PAGES` names pages as `file:page`, comma separated, with
//! files relative to the benchmark corpus (default: the ten image-heavy
//! benchmark pages); `PAGURA_IMAGE_DPI` the resolutions (default
//! `72,150,300`); `PAGURA_IMAGE_RUNS` the samples of which the best is kept
//! (default 7); `PAGURA_IMAGE_ROUTES` the routes (default
//! `old,exact,reduced`).
//!
//! The new routes decode every image of the page with a fresh store, then
//! prepare its sampler and fill every span of its device bounds within the
//! page. The old routes render the whole page through `pagura-render`'s
//! warm context, which compiles (and decodes) on every call.

use super::converter;
use crate::SpanColors;
use crate::source::PaintContext;
use crate::source::image::{
    Description, ImageSampler, MaskDescription, Orientation, Prepared, decode as decode_image,
    planes_suit,
};
use pagura_color::{ColorSpace, Process, Target};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, Limits, ObjectId, Store, Value};
use pagura_document::{Document, OpenOptions, Page, Rotation};
use pagura_image::{Decoded, ImageKind, ImageSource};
use pagura_interpret::Composite;
use pagura_raster::{DeviceRect, Point, Transform};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// The image-heavy pages of the benchmark manifest.
const IMAGE_PAGES: &str = "0000008.pdf:0,0000008.pdf:1,0000009.pdf:0,0000009.pdf:1,\
    0000095.pdf:0,0000095.pdf:1,0000200.pdf:2,0000200.pdf:3,0000603.pdf:0,0000603.pdf:1";

/// One image drawn by a page: the XObject and its unit square to user space.
struct Draw {
    id: ObjectId,
    matrix: Transform,
}

/// What the content walk carries.
struct Walk<'a> {
    document: &'a Document,
    draws: Vec<Draw>,
}

impl Walk<'_> {
    /// The image XObjects drawn by `content` under `ctm`, following forms.
    /// An inline image, or anything this small walker does not understand,
    /// ends the walk of that stream.
    fn content(&mut self, content: &[u8], resources: &Dictionary, ctm: Transform, depth: usize) {
        let mut parser = Parser::new(content, Origin::Buffer, self.document.limits());
        let mut operands: Vec<Value> = Vec::new();
        let mut saved = Vec::new();
        let mut ctm = ctm;
        loop {
            parser.skip_whitespace();
            let Some(next) = parser.peek() else {
                return;
            };
            if !(next.is_ascii_alphabetic() || next == b'\'' || next == b'"') {
                match parser.parse() {
                    Ok(value) => operands.push(value),
                    Err(_) => return,
                }
                continue;
            }
            let Ok(word) = parser.word() else {
                return;
            };
            match word {
                b"true" | b"false" | b"null" => {
                    operands.push(Value::Null);
                    continue;
                }
                b"q" => saved.push(ctm),
                b"Q" => ctm = saved.pop().unwrap_or(ctm),
                b"cm" => {
                    if let Some(matrix) = matrix(&operands) {
                        ctm = matrix.then(ctm);
                    }
                }
                b"Do" => {
                    if let Some(Value::Name(name)) = operands.last() {
                        self.xobject(resources, name.as_bytes(), ctm, depth);
                    }
                }
                b"BI" => return,
                _ => {}
            }
            operands.clear();
        }
    }

    fn xobject(&mut self, resources: &Dictionary, name: &[u8], ctm: Transform, depth: usize) {
        let document = self.document;
        let Some(id) = resources
            .get(b"XObject")
            .and_then(|v| document.resolve_dictionary(v).ok())
            .and_then(|xobjects| xobjects.get(name))
            .and_then(Value::as_reference)
        else {
            return;
        };
        let Some(object) = document.object(id).ok() else {
            return;
        };
        let Some(dictionary) = object.dictionary() else {
            return;
        };
        let subtype = dictionary.get(b"Subtype").and_then(Value::as_name);
        match subtype.map(pagura_core::Name::as_bytes) {
            Some(b"Image") => self.draws.push(Draw { id, matrix: ctm }),
            Some(b"Form") if depth < 8 => {
                let form = dictionary
                    .get(b"Matrix")
                    .and_then(|v| document.resolve(v).ok())
                    .and_then(Value::as_array)
                    .and_then(matrix)
                    .unwrap_or(Transform::IDENTITY);
                let own = dictionary
                    .get(b"Resources")
                    .and_then(|v| document.resolve_dictionary(v).ok())
                    .unwrap_or(resources);
                if let Ok(data) = document.stream_data(object) {
                    self.content(&data, own, form.then(ctm), depth + 1);
                }
            }
            _ => {}
        }
    }
}

fn matrix(operands: &[Value]) -> Option<Transform> {
    let [a, b, c, d, e, f] = operands else {
        return None;
    };
    let n = Value::as_number;
    Some(Transform::new(n(a)?, n(b)?, n(c)?, n(d)?, n(e)?, n(f)?))
}

/// The page's user space to device pixels at `scale`, as the old renderer
/// places it.
fn device_transform(page: &Page, scale: f64) -> Transform {
    let r = page.crop_box();
    let rotation = match page.rotation() {
        Rotation::None => Transform::new(1.0, 0.0, 0.0, -1.0, -r.x0(), r.y1()),
        Rotation::Clockwise90 => Transform::new(0.0, 1.0, 1.0, 0.0, -r.y0(), -r.x0()),
        Rotation::Clockwise180 => Transform::new(-1.0, 0.0, 0.0, 1.0, r.x1(), -r.y0()),
        Rotation::Clockwise270 => Transform::new(0.0, -1.0, -1.0, 0.0, r.y1(), r.x1()),
    };
    let factor = scale * page.user_unit();
    rotation.then(Transform::scale(factor, factor))
}

/// The device pixels of the unit square under `unit_to_device` within a
/// `width` × `height` page.
fn bounds(unit_to_device: Transform, (width, height): (u32, u32)) -> Option<DeviceRect> {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .map(|(x, y)| unit_to_device.apply(Point::new(x, y)));
    let low = |pick: fn(&Point) -> f64| corners.iter().map(pick).fold(f64::INFINITY, f64::min);
    let high = |pick: fn(&Point) -> f64| corners.iter().map(pick).fold(f64::NEG_INFINITY, f64::max);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the page"
    )]
    let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as u32;
    let rect = DeviceRect {
        x0: clamp(low(|p| p.x).floor(), width),
        y0: clamp(low(|p| p.y).floor(), height),
        x1: clamp(high(|p| p.x).ceil(), width),
        y1: clamp(high(|p| p.y).ceil(), height),
    };
    (rect.x0 < rect.x1 && rect.y0 < rect.y1).then_some(rect)
}

/// An image of a page, ready to decode and sample.
pub(super) struct Placed {
    pub(super) source: ImageSource,
    pub(super) space: Option<ColorSpace>,
    pub(super) unit_to_device: Transform,
    pub(super) rect: DeviceRect,
}

/// The images `page` draws at `scale` device pixels per unit, with device
/// colour spaces or as stencils, and their device bounds.
pub(super) fn placed(document: &Document, page: &Page, scale: f64) -> Vec<Placed> {
    let mut walk = Walk {
        document,
        draws: Vec::new(),
    };
    for stream in document.content_streams(page).unwrap_or_default() {
        if let Ok(data) = document.stream_data(stream) {
            walk.content(&data, page.resources(), Transform::IDENTITY, 0);
        }
    }
    let device = device_transform(page, scale);
    let options = pagura_render::RenderOptions {
        scale,
        ..pagura_render::RenderOptions::default()
    };
    let Ok(size) = pagura_render::device_size(page, &options) else {
        return Vec::new();
    };
    walk.draws
        .into_iter()
        .filter_map(|draw| {
            let source =
                ImageSource::from_xobject(document, document.object(draw.id).ok()?).ok()?;
            let space = match source.kind() {
                ImageKind::Stencil { .. } => None,
                _ => Some(match source.color_space()?.as_name()?.as_bytes() {
                    b"DeviceGray" => ColorSpace::DeviceGray,
                    b"DeviceRGB" => ColorSpace::DeviceRgb,
                    b"DeviceCMYK" => ColorSpace::DeviceCmyk,
                    _ => return None,
                }),
            };
            let unit_to_device = draw.matrix.then(device);
            let rect = bounds(unit_to_device, size)?;
            Some(Placed {
                source,
                space,
                unit_to_device,
                rect,
            })
        })
        .collect()
}

/// Decodes `placed` for painting through a fresh store.
pub(super) fn decode_once(placed: &Placed, limits: &Limits, reduced: bool) -> Arc<Decoded> {
    let target = Target::new(Process::Rgb);
    let composite = Composite::default();
    let store = Store::new(usize::MAX);
    let context = PaintContext {
        transform: Transform::IDENTITY,
        target: &target,
        composite: &composite,
        store: &store,
        limits,
        reduced_image_decode: reduced,
        region: placed.rect,
    };
    let decode = decode_array(placed, usize::from(placed.source.components()));
    let plain = matches!(placed.source.mask(), pagura_image::SourceMask::None)
        && !placed.source.interpolate();
    let planes = planes_suit(
        (placed.space.as_ref(), &decode, plain),
        placed.unit_to_device,
        &context,
    );
    let orientation = Orientation::of(placed.unit_to_device, MaskDescription::None);
    decode_image(
        &placed.source,
        placed.unit_to_device,
        (planes, orientation),
        &context,
    )
    .unwrap()
}

/// The decode array of `placed`, one interval per component.
fn decode_array(placed: &Placed, components: usize) -> Vec<[f64; 2]> {
    placed.source.decode_array().map_or_else(
        || vec![[0.0, 1.0]; components],
        |values| values.as_chunks::<2>().0.to_vec(),
    )
}

/// The sampler of `placed` over `decoded`, into RGB.
pub(super) fn sampler(placed: &Placed, decoded: Arc<Decoded>) -> ImageSampler {
    let components = usize::from(decoded.image.components);
    let decode = decode_array(placed, components);
    let description = Description {
        space: placed.space.as_ref(),
        decode: &decode,
        key: None,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let convert = placed.space.as_ref().map(|space| converter(space, 3));
    let (prepared, _) = Prepared::new(
        &description,
        (decoded, None),
        (
            placed.unit_to_device.inverse().unwrap(),
            (placed.rect.x0, placed.rect.x1),
            Orientation::of(placed.unit_to_device, MaskDescription::None),
        ),
        convert,
    )
    .unwrap();
    ImageSampler {
        prepared: Some(Box::new(prepared)),
        channels: if placed.space.is_some() { 3 } else { 0 },
    }
}

/// Prepares the sampler of `placed` over `decoded` and fills every span of
/// its bounds, returning a checksum.
fn sample_once(placed: &Placed, decoded: Arc<Decoded>) -> f64 {
    let sampler = sampler(placed, decoded);
    let rect = placed.rect;
    let len = usize::try_from(rect.x1 - rect.x0).unwrap();
    let mut out = SpanColors::default();
    let mut checksum = 0.0;
    for y in rect.y0..rect.y1 {
        sampler.span(rect.x0, y, len, &mut out);
        checksum += out.colors.iter().step_by(97).sum::<f64>();
    }
    checksum
}

fn best(runs: usize, mut run: impl FnMut()) -> f64 {
    run();
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

/// Best decode and sampling times of a page's images, in milliseconds.
fn new_route(images: &[Placed], limits: &Limits, reduced: bool, runs: usize) -> (f64, f64) {
    let only = std::env::var("PAGURA_IMAGE_STAGE").unwrap_or_default();
    let decoding = if only == "sample" {
        f64::NAN
    } else {
        best(runs, || {
            for placed in images {
                std::hint::black_box(decode_once(placed, limits, reduced));
            }
        })
    };
    if only == "decode" {
        return (decoding, f64::NAN);
    }
    let decoded: Vec<_> = images
        .iter()
        .map(|placed| decode_once(placed, limits, reduced))
        .collect();
    let sampling = best(runs, || {
        for (placed, decoded) in images.iter().zip(&decoded) {
            std::hint::black_box(sample_once(placed, Arc::clone(decoded)));
        }
    });
    (decoding, sampling)
}

/// The best time of the old renderer's whole page under `accuracy`.
fn old_route(
    document: &Arc<Document>,
    index: usize,
    scale: f64,
    accuracy: pagura_render::Accuracy,
    runs: usize,
) -> f64 {
    let context = pagura_render::RenderContext::new(Arc::clone(document));
    let page = context.document().page(index).unwrap();
    let options = pagura_render::RenderOptions {
        scale,
        accuracy,
        ..pagura_render::RenderOptions::default()
    };
    best(runs, || {
        std::hint::black_box(context.render_page(page, &options).unwrap());
    })
}

pub(super) fn setting(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// The pages a run names, opened: `(name, document, page index)`.
pub(super) fn pages(default: &str) -> Vec<(String, Arc<Document>, usize)> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let corpus = PathBuf::from(home).join(".cache/pagura-corpora/safedocs-cc-main-2021-31/pdf");
    setting("PAGURA_IMAGE_PAGES", default)
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (file, index) = entry.rsplit_once(':').unwrap_or((entry, "0"));
            let bytes = std::fs::read(corpus.join(file))
                .or_else(|_| std::fs::read(file))
                .ok()?;
            let document = Document::open(bytes, &OpenOptions::default()).ok()?;
            Some((entry.to_owned(), Arc::new(document), index.parse().ok()?))
        })
        .collect()
}

#[test]
#[ignore = "timing; needs the benchmark corpus"]
fn image_page_timing() {
    let dpis: Vec<f64> = setting("PAGURA_IMAGE_DPI", "72,150,300")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let runs = setting("PAGURA_IMAGE_RUNS", "7").parse().unwrap_or(7);
    let routes = setting("PAGURA_IMAGE_ROUTES", "old,exact,reduced");
    let routes: Vec<&str> = routes.split(',').map(str::trim).collect();
    eprintln!(
        "page dpi | old exact | old competitive | exact: decode + sample = total (÷ old exact) \
         | reduced: decode + sample = total (÷ old competitive), milliseconds"
    );
    for (name, document, index) in pages(IMAGE_PAGES) {
        let page = document.page(index).unwrap();
        let limits = document.limits();
        for &dpi in &dpis {
            let scale = dpi / 72.0;
            let images = placed(&document, page, scale);
            let mut line = format!("{name} {dpi} ({} images)", images.len());
            let (mut old_exact, mut old_competitive) = (f64::NAN, f64::NAN);
            if routes.contains(&"old") {
                old_exact = old_route(
                    &document,
                    index,
                    scale,
                    pagura_render::Accuracy::Exact,
                    runs,
                );
                old_competitive = old_route(
                    &document,
                    index,
                    scale,
                    pagura_render::Accuracy::Competitive,
                    runs,
                );
                let _ = write!(line, " | {old_exact:.2} | {old_competitive:.2}");
            }
            for (route, reduced, old) in [
                ("exact", false, old_exact),
                ("reduced", true, old_competitive),
            ] {
                if routes.contains(&route) {
                    let (decoding, sampling) = new_route(&images, &limits, reduced, runs);
                    let total = decoding + sampling;
                    let _ = write!(
                        line,
                        " | {route} {decoding:.2} + {sampling:.2} = {total:.2} ({:.3})",
                        total / old
                    );
                }
            }
            eprintln!("{line}");
        }
    }
}

/// Decode times of each image of the pages in the interleaved and native
/// layouts and with drawn-size cells.
#[test]
#[ignore = "timing; needs the benchmark corpus"]
fn image_decode_timing() {
    use pagura_image::{DecodeRequest, Layout, Reduce};
    let runs = setting("PAGURA_IMAGE_RUNS", "7").parse().unwrap_or(7);
    for (name, document, index) in pages(IMAGE_PAGES) {
        let page = document.page(index).unwrap();
        let limits = document.limits();
        for placed in placed(&document, page, 1.0) {
            let source = &placed.source;
            let mut line = format!("{name} {}x{}", source.width(), source.height());
            for (label, reduce, layout) in [
                ("interleaved", Reduce::Full, Layout::Interleaved),
                ("native", Reduce::Full, Layout::Native),
                ("cells 6", Reduce::Drawn { cells: 6 }, Layout::Interleaved),
                ("cells 3", Reduce::Drawn { cells: 3 }, Layout::Interleaved),
            ] {
                let request = DecodeRequest::new(reduce, None, layout);
                let time = best(runs, || {
                    std::hint::black_box(
                        source.decode(&request, &limits, source.recovery()).unwrap(),
                    );
                });
                let _ = write!(line, " | {label} {time:.2}");
                if let Ok(decoded) = source.decode(&request, &limits, source.recovery())
                    && let pagura_image::Samples::Planar(planar) = &decoded.image.samples
                {
                    let [_, _, pitch] = planar.geometry(0);
                    let (stride, rows) = (pitch / 8, planar.height().div_ceil(16) * 2);
                    let dirty = (0..rows)
                        .flat_map(|row| (0..stride).map(move |block| (row, block)))
                        .filter(|&(row, block)| planar.dirty(0, row, block))
                        .count();
                    let _ = write!(
                        line,
                        " (dirty luma blocks {:.3})",
                        dirty as f64 / (rows * stride) as f64
                    );
                }
            }
            eprintln!("{line}");
        }
    }
}
