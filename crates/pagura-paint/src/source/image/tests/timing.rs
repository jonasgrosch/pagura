//! Timing of a page's images through decode requests and the sampler
//! family. Run in release:
//!
//! ```text
//! cargo test --release -p pagura-paint --lib image_page_timing -- --ignored --nocapture
//! ```
//!
//! `PAGURA_IMAGE_PAGES` names pages as `file:page`, comma separated, with
//! files relative to the benchmark corpus (default: the ten image-heavy
//! benchmark pages); `PAGURA_IMAGE_DPI` the resolutions (default
//! `72,150,300`); `PAGURA_IMAGE_RUNS` the samples of which the best is kept
//! (default 7); `PAGURA_IMAGE_STAGE` `decode` or `sample` times that stage
//! alone.
//!
//! Each run decodes every image of the page with a fresh store, then
//! prepares its sampler and fills every span of its device bounds within the
//! page.

use super::converter;
use crate::source::PaintContext;
use crate::source::SpanColors;
use crate::source::image::{
    Description, ImageSampler, MaskDescription, Orientation, Prepared, decode as decode_image,
    planes_suit,
};
use pagura_color::{ColorSpace, Process, Target};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, Limits, ObjectId, Store, Value};
use pagura_document::{Document, OpenOptions, Page};
use pagura_image::{Decoded, ImageKind, ImageSource};
use pagura_interpret::{Composite, PageSpace};
use pagura_raster::{DeviceRect, Point, Transform};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// The image-heavy pages of the benchmark manifest.
pub(super) const IMAGE_PAGES: &str = "0000008.pdf:0,0000008.pdf:1,0000009.pdf:0,0000009.pdf:1,\
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
    let space = PageSpace {
        crop: page.crop_box(),
        rotation: page.rotation(),
        user_unit: page.user_unit(),
    };
    let Ok(request) = crate::RenderRequest::page(&space, scale) else {
        return Vec::new();
    };
    let device = request.transform;
    let size = (request.page.width(), request.page.height());
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
pub(super) fn decode_once(placed: &Placed, limits: &Limits) -> Option<Arc<Decoded>> {
    let target = Target::new(Process::Rgb);
    let composite = Composite::default();
    let store = Store::new(usize::MAX);
    let context = PaintContext {
        transform: Transform::IDENTITY,
        target: &target,
        composite: &composite,
        store: &store,
        limits,
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
    let orientation = Orientation::of(placed.unit_to_device);
    decode_image(&placed.source, (planes, orientation), &context).ok()
}

/// The decode array of `placed`, one interval per component.
fn decode_array(placed: &Placed, components: usize) -> Vec<[f64; 2]> {
    placed.source.decode_array().map_or_else(
        || vec![[0.0, 1.0]; components],
        |values| values.as_chunks::<2>().0.to_vec(),
    )
}

/// The sampler of `placed` over `decoded`, into RGB, or `None` where its
/// placement cannot be sampled.
pub(super) fn sampler(placed: &Placed, decoded: Arc<Decoded>) -> Option<ImageSampler> {
    let components = usize::from(decoded.image.components);
    let decode = decode_array(placed, components);
    let key = match placed.source.mask() {
        pagura_image::SourceMask::ColorKey(ranges) => Some(ranges.as_slice()),
        _ => None,
    };
    let description = Description {
        space: placed.space.as_ref(),
        decode: &decode,
        key,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let convert = placed.space.as_ref().map(|space| converter(space, 3));
    let (prepared, _) = Prepared::new(
        &description,
        (decoded, None),
        (
            placed.unit_to_device.inverse()?,
            (placed.rect.x0, placed.rect.x1),
            Orientation::of(placed.unit_to_device),
        ),
        convert,
    )
    .ok()?;
    Some(ImageSampler::new(
        Some(Box::new(prepared)),
        if placed.space.is_some() { 3 } else { 0 },
    ))
}

/// Prepares the sampler of `placed` over `decoded` and fills every span of
/// its bounds, returning a checksum.
fn sample_once(placed: &Placed, decoded: Arc<Decoded>) -> f64 {
    let Some(sampler) = sampler(placed, decoded) else {
        return 0.0;
    };
    let rect = placed.rect;
    let len = usize::try_from(rect.x1 - rect.x0).unwrap();
    let mut checksum = 0.0;
    let mut out = SpanColors::default();
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
fn new_route(images: &[Placed], limits: &Limits, runs: usize) -> (f64, f64) {
    let only = std::env::var("PAGURA_IMAGE_STAGE").unwrap_or_default();
    let decoding = if only == "sample" {
        f64::NAN
    } else {
        best(runs, || {
            for placed in images {
                std::hint::black_box(decode_once(placed, limits));
            }
        })
    };
    if only == "decode" {
        return (decoding, f64::NAN);
    }
    let decoded: Vec<_> = images
        .iter()
        .map(|placed| decode_once(placed, limits))
        .collect();
    let sampling = best(runs, || {
        for (placed, decoded) in images.iter().zip(&decoded) {
            if let Some(decoded) = decoded {
                std::hint::black_box(sample_once(placed, Arc::clone(decoded)));
            }
        }
    });
    (decoding, sampling)
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
    eprintln!("page dpi | decode + sample = total, milliseconds");
    for (name, document, index) in pages(IMAGE_PAGES) {
        let page = document.page(index).unwrap();
        let limits = document.limits();
        for &dpi in &dpis {
            let images = placed(&document, page, dpi / 72.0);
            let (decoding, sampling) = new_route(&images, &limits, runs);
            let total = decoding + sampling;
            eprintln!(
                "{name} {dpi} ({} images) | {decoding:.2} + {sampling:.2} = {total:.2}",
                images.len()
            );
        }
    }
}

/// Decode times of each image of the pages in the interleaved and native
/// layouts.
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
            for (label, layout) in [
                ("interleaved", Layout::Interleaved),
                ("native", Layout::Native),
            ] {
                let request = DecodeRequest::new(Reduce::Full, None, layout);
                let time = best(runs, || {
                    std::hint::black_box(
                        source.decode(&request, &limits, source.recovery()).unwrap(),
                    );
                });
                let _ = write!(line, " | {label} {time:.2}");
                let decoded = source.decode(&request, &limits, source.recovery());
                if let Ok(decoded) = &decoded
                    && layout == Layout::Native
                    && !matches!(decoded.image.samples, pagura_image::Samples::Planar(_))
                {
                    line += " (not planar)";
                }
                if let Ok(decoded) = decoded
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

/// What each image of the pages is and how it is sampled at each
/// resolution.
#[test]
#[ignore = "needs the benchmark corpus"]
fn image_page_details() {
    let dpis: Vec<f64> = setting("PAGURA_IMAGE_DPI", "72,150,300")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    for (name, document, index) in pages(IMAGE_PAGES) {
        let page = document.page(index).unwrap();
        let limits = document.limits();
        for &dpi in &dpis {
            for placed in placed(&document, page, dpi / 72.0) {
                let source = &placed.source;
                let decoded = decode_once(&placed, &limits).unwrap();
                let sampler = sampler(&placed, Arc::clone(&decoded)).unwrap();
                let prepared = sampler.prepared.as_deref().unwrap();
                let image = &decoded.image;
                let layout = match &image.samples {
                    pagura_image::Samples::Bytes(_) => "bytes",
                    pagura_image::Samples::Words(_) => "words",
                    pagura_image::Samples::Mixed { .. } => "mixed",
                    pagura_image::Samples::Packed(_) => "packed",
                    pagura_image::Samples::Planar(_) => "planar",
                };
                eprintln!(
                    "{name} {dpi}: source {}x{} {}x{}bit {:?} | decoded {}x{} {layout} | \
                     drawn {}x{} at {:?} | mode {:?} codes {}",
                    source.width(),
                    source.height(),
                    source.components(),
                    source.bits_per_component(),
                    source.kind(),
                    image.width,
                    image.height,
                    placed.rect.x1 - placed.rect.x0,
                    placed.rect.y1 - placed.rect.y0,
                    placed.unit_to_device,
                    prepared.geometry.mode(),
                    prepared.codes.is_some(),
                );
            }
        }
    }
}
