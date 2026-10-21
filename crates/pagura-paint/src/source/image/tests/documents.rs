//! Images read from documents: requests for exact means decode and sample
//! as full decoding does, and the large JPEG benchmark page's timing.

use super::{Device, SpanColors};
use crate::source::image::texels::Convert;
use crate::source::image::{Description, ImageSampler, MaskDescription, Prepared};
use pagura_color::ColorSpace;
use pagura_core::{ObjectId, Value};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, Decoded, ImageSource, Layout, Reduce};
use pagura_raster::Transform;
use std::path::PathBuf;
use std::sync::Arc;

/// The DCTDecode image XObjects of `document`.
fn jpeg_images(document: &Document) -> Vec<ObjectId> {
    document
        .object_ids()
        .filter(|&id| {
            let Some(dictionary) = document.object(id).ok().and_then(|o| o.dictionary()) else {
                return false;
            };
            let filter = dictionary
                .get(b"Filter")
                .and_then(|v| document.resolve(v).ok());
            let last = match filter {
                Some(Value::Array(filters)) => filters.last(),
                filter => filter,
            };
            dictionary
                .get(b"Subtype")
                .and_then(Value::as_name)
                .is_some_and(|n| n == b"Image")
                && last
                    .and_then(Value::as_name)
                    .is_some_and(|n| n == b"DCTDecode")
        })
        .collect()
}

/// A sampler of an opaque device-colour image without masks.
fn device_sampler(decoded: Arc<Decoded>, unit_to_device: Transform) -> ImageSampler {
    let components = usize::from(decoded.image.components);
    let space = match components {
        1 => ColorSpace::DeviceGray,
        3 => ColorSpace::DeviceRgb,
        _ => ColorSpace::DeviceCmyk,
    };
    let decode = vec![[0.0, 1.0]; components];
    let description = Description {
        space: Some(&space),
        decode: &decode,
        key: None,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let convert: Box<dyn Convert> = Box::new(Device {
        source: components,
        target: 3,
    });
    let (prepared, _) = Prepared::new(
        &description,
        (decoded, None),
        unit_to_device.inverse().unwrap(),
        Some((convert, components == 3)),
    )
    .unwrap();
    ImageSampler {
        prepared: Some(Box::new(prepared)),
        channels: 3,
    }
}

/// Every span of a `width` × `height` device window, colours then alpha.
fn sample_all(sampler: &ImageSampler, (width, height): (u32, u32)) -> (Vec<f64>, Vec<f64>) {
    let mut out = SpanColors::default();
    let (mut colors, mut alpha) = (Vec::new(), Vec::new());
    let len = usize::try_from(width).unwrap();
    for y in 0..height {
        sampler.span(0, y, len, &mut out);
        colors.extend_from_slice(&out.colors);
        alpha.extend_from_slice(&out.alpha);
    }
    (colors, alpha)
}

fn corpus(file: &str) -> Option<Vec<u8>> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    std::fs::read(
        home.join(".cache/pagura-corpora/safedocs-cc-main-2021-31/pdf")
            .join(file),
    )
    .ok()
}

#[test]
fn exact_means_decode_and_sample_as_full_decoding() {
    let directory = pagura_testkit::workspace_root().join("corpus/generated/image");
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "pdf"))
        .filter_map(|path| std::fs::read(path).ok())
        .collect();
    files.extend(corpus("0000603.pdf"));
    let mut checked = 0;
    for bytes in files {
        let Ok(document) = Document::open(bytes, &OpenOptions::default()) else {
            continue;
        };
        // One image per document keeps the large page's four quick.
        for id in jpeg_images(&document).into_iter().take(1) {
            let Ok(source) = ImageSource::from_xobject(&document, document.object(id).unwrap())
            else {
                continue;
            };
            let limits = document.limits();
            let decode = |reduce| {
                source.decode(
                    &DecodeRequest::new(reduce, None, Layout::Interleaved),
                    &limits,
                    source.recovery(),
                )
            };
            let Ok(full) = decode(Reduce::Full) else {
                continue;
            };
            let full = Arc::new(full);
            let means = Arc::new(decode(Reduce::ExactMeans { x: 8, y: 8 }).unwrap());
            assert_eq!(means.image, full.image, "{id}");
            for minified in [8.5, 16.25] {
                let (w, h) = (f64::from(source.width()), f64::from(source.height()));
                let placement = Transform::new(
                    w / minified,
                    0.0,
                    0.0,
                    -h / minified,
                    0.25,
                    h / minified + 0.5,
                );
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "test window"
                )]
                let window = (
                    (w / minified).ceil() as u32 + 1,
                    (h / minified).ceil() as u32 + 1,
                );
                assert_eq!(
                    sample_all(&device_sampler(Arc::clone(&full), placement), window),
                    sample_all(&device_sampler(Arc::clone(&means), placement), window),
                    "{id}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no JPEG fixtures");
}

/// The page-0 image of `0000603.pdf` at 72 dpi, decoded and sampled
/// through the new request and sampler, exactly and with drawn-size cells.
/// Run in release with `--ignored --nocapture`.
#[test]
#[ignore = "timing; needs the benchmark corpus"]
fn large_jpeg_page_timing() {
    let Some(bytes) = corpus("0000603.pdf") else {
        return;
    };
    let document = Document::open(bytes, &OpenOptions::default()).unwrap();
    let source =
        ImageSource::from_xobject(&document, document.object(ObjectId::new(21, 0)).unwrap())
            .unwrap();
    // `0.24 0 0 0.24 0 0 cm 2552 0 0 4200 0 0 cm` on a 612 × 1008 page
    // turned by 90 degrees, at 72 dpi.
    let unit_to_user = Transform::new(2552.0, 0.0, 0.0, 4200.0, 0.0, 0.0)
        .then(Transform::new(0.24, 0.0, 0.0, 0.24, 0.0, 0.0));
    let device = Transform::new(0.0, 1.0, 1.0, 0.0, 0.0, 0.0);
    let unit_to_device = unit_to_user.then(device);
    let window = (1008, 612);
    let limits = document.limits();
    let t = unit_to_device;
    let ratio = (2552.0 / t.a.hypot(t.b)).min(4200.0 / t.c.hypot(t.d));
    let decode = |reduce| {
        Arc::new(
            source
                .decode(
                    &DecodeRequest::new(reduce, None, Layout::Interleaved),
                    &limits,
                    source.recovery(),
                )
                .unwrap(),
        )
    };
    let best = |run: &mut dyn FnMut()| {
        (0..15)
            .map(|_| {
                let start = std::time::Instant::now();
                run();
                start.elapsed().as_secs_f64() * 1e3
            })
            .fold(f64::INFINITY, f64::min)
    };
    // `PAGURA_IMAGE_SAMPLES=n` samples the exact decode n times and stops,
    // for counting instructions.
    if let Some(count) = std::env::var("PAGURA_IMAGE_SAMPLES")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
    {
        let decoded = decode(Reduce::Full);
        for _ in 0..count {
            sample_all(
                &device_sampler(Arc::clone(&decoded), unit_to_device),
                window,
            );
        }
        return;
    }
    for (name, reduce) in [("exact", Reduce::Full), ("reduced", source.drawn(ratio))] {
        let decoding = best(&mut || {
            decode(reduce);
        });
        let decoded = decode(reduce);
        let sampling = best(&mut || {
            sample_all(
                &device_sampler(Arc::clone(&decoded), unit_to_device),
                window,
            );
        });
        let checksum: f64 = sample_all(&device_sampler(decoded, unit_to_device), window)
            .0
            .iter()
            .sum();
        eprintln!(
            "{name} {reduce:?}: decode {decoding:.1} ms, sample {sampling:.1} ms, \
             total {:.1} ms (checksum {checksum:.3})",
            decoding + sampling
        );
    }
}
