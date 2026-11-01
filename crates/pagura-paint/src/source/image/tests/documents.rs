//! Images read from documents: JPEG component planes, background blocks
//! and exchanged rows and columns sample as interleaved samples do.

use super::{SpanColors, converter};
use crate::source::image::{Description, ImageSampler, MaskDescription, Orientation, Prepared};
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

/// A sampler of an opaque device-colour image without masks, its samples
/// exchanged for a quarter turn as preparation decodes them.
fn oriented_sampler(decoded: Arc<Decoded>, unit_to_device: Transform, width: u32) -> ImageSampler {
    let orientation = Orientation::of(unit_to_device);
    let decoded = match orientation {
        Orientation::Rows => decoded,
        Orientation::Transposed => Arc::new(Decoded::from(
            decoded
                .image
                .transposed(&pagura_core::Limits::default())
                .unwrap(),
        )),
    };
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
    let convert = converter(&space, 3);
    let prepared = Prepared::new(
        &description,
        (decoded, None),
        (unit_to_device.inverse().unwrap(), (0, width), orientation),
        Some(convert),
        None,
        1,
        super::super::ImageBudgets {
            bytes: &mut super::meter(),
            work: &mut super::meter(),
            functions: &mut super::meter(),
        },
    )
    .unwrap();
    ImageSampler::new(Some(Box::new(prepared)), 3)
}

/// Every span of a `width` × `height` device window, colours then alpha.
fn sample_all(sampler: &ImageSampler, (width, height): (u32, u32)) -> (Vec<f64>, Vec<f64>) {
    let mut out = SpanColors::default();
    let (mut colors, mut alpha) = (Vec::new(), Vec::new());
    let len = usize::try_from(width).unwrap();
    let mut storage = super::scratch(sampler);
    for y in 0..height {
        sampler.span(
            crate::source::Pixels::new(0, y, len),
            &mut storage,
            &mut out,
        );
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
fn planes_background_blocks_and_turns_sample_as_interleaved_samples() {
    let directory = pagura_testkit::workspace_root().join("corpus/generated/image");
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "pdf"))
        .filter_map(|path| std::fs::read(path).ok())
        .collect();
    // The generated fixtures hold no baseline YCbCr JPEG that decodes to
    // planes; the real-world page does, where the corpus is available.
    let Some(page) = corpus("0000095.pdf") else {
        eprintln!("skipped: corpus page 0000095.pdf is not available");
        return;
    };
    files.push(page);
    let mut planar = 0;
    for bytes in files {
        let Ok(document) = Document::open(bytes, &OpenOptions::default()) else {
            continue;
        };
        for id in jpeg_images(&document).into_iter().take(1) {
            let Ok(source) = ImageSource::from_xobject(&document, document.object(id).unwrap())
            else {
                continue;
            };
            let limits = document.limits();
            let decode = |layout| {
                source
                    .decode(
                        &DecodeRequest::new(Reduce::Full, None, layout),
                        &limits,
                        source.recovery(),
                    )
                    .map(Arc::new)
            };
            let (Ok(interleaved), Ok(native)) =
                (decode(Layout::Interleaved), decode(Layout::Native))
            else {
                continue;
            };
            if !matches!(native.image.samples, pagura_image::Samples::Planar(_)) {
                continue;
            }
            planar += 1;
            let (w, h) = (f64::from(source.width()), f64::from(source.height()));
            let scale = (240.0 / w.max(h)).min(1.0);
            for (a, b, c, d) in [
                (w * scale / 3.3, 0.0, 0.0, -h * scale / 3.3),
                (w * scale, 0.0, 0.0, -h * scale),
                (w * 1.7, 0.0, 0.0, h * 1.7),
                (0.0, h * scale / 2.6, w * scale / 2.6, 0.0),
                (0.0, -h * 1.3, w * 1.3, 0.0),
            ] {
                let placement = Transform::new(a, b, c, d, 0.25, 0.4 - d.min(0.0) - b.min(0.0));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "test window"
                )]
                let window = (
                    ((a.abs() + c.abs()).ceil() as u32 + 1).min(200),
                    ((b.abs() + d.abs()).ceil() as u32 + 1).min(200),
                );
                let planes = oriented_sampler(Arc::clone(&native), placement, window.0);
                assert_eq!(
                    sample_all(
                        &oriented_sampler(Arc::clone(&interleaved), placement, window.0),
                        window
                    ),
                    sample_all(&planes, window),
                    "{id} {placement:?}"
                );
            }
        }
    }
    assert!(planar > 0, "no planar JPEG fixtures");
}
