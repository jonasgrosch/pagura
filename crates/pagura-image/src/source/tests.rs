//! Unit tests of image sources: what interpretation reads without decoding,
//! drawn-size reductions and independence from the document.

use super::{DecodeRequest, ImageSource, Layout, Reduce, SourceMask};
use crate::{ImageKind, MaskKind, Samples};
use pagura_core::{ObjectId, Recovery, Value};
use pagura_document::{Document, OpenOptions};

/// A PDF whose objects 3 and onwards are the given streams, each as
/// (dictionary entries, data).
fn pdf(streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |pdf: &mut Vec<u8>, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    object(&mut pdf, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut pdf, b"<< /Type /Pages /Kids [] /Count 0 >>");
    for (dictionary, data) in streams {
        let mut body = format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        object(&mut pdf, &body);
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn source(document: &Document, number: u32) -> ImageSource {
    ImageSource::from_xobject(document, document.object(ObjectId::new(number, 0)).unwrap()).unwrap()
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, &OpenOptions::default()).unwrap()
}

const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 17 /Height 17 /BitsPerComponent 8";

#[test]
fn drawn_cells_keep_two_and_a_half_cells_per_device_pixel() {
    let jpeg = pagura_testkit::fixture("image", "jpeg-interleaved-420.in");
    let rgb = format!("{IMAGE} /ColorSpace /DeviceRGB /Filter /DCTDecode");
    let document = open(pdf(&[(&rgb, &jpeg)]));
    let image = source(&document, 3);
    for (ratio, cells) in [
        (f64::NAN, 8),
        (0.5, 8),
        (3.3, 8),
        (3.4, 6),
        (4.9, 6),
        (5.0, 4),
        (6.6, 4),
        (6.7, 3),
        (9.9, 3),
        (10.0, 2),
        (19.9, 2),
        (20.0, 1),
        (f64::INFINITY, 1),
    ] {
        let expected = if cells == 8 {
            Reduce::Full
        } else {
            Reduce::Drawn { cells }
        };
        assert_eq!(image.drawn(ratio), expected, "{ratio}");
    }
    let reduced = image
        .decode(
            &DecodeRequest::new(Reduce::Drawn { cells: 2 }, None, Layout::Interleaved),
            &document.limits(),
            Recovery::Strict,
        )
        .unwrap()
        .image;
    assert_eq!((reduced.width, reduced.height), (5, 5));
    assert_eq!(reduced.reduction.map(|r| r.cells), Some(2));
}

#[test]
fn only_opaque_device_jpeg_images_are_reduced() {
    let jpeg = pagura_testkit::fixture("image", "jpeg-interleaved-420.in");
    let gray = [7u8; 17 * 17];
    let streams = [
        format!("{IMAGE} /ColorSpace /DeviceRGB /Filter /DCTDecode /Decode [0 1 0 1 0 1]"),
        format!("{IMAGE} /ColorSpace [/DeviceRGB] /Filter /DCTDecode"),
        format!("{IMAGE} /ColorSpace /DeviceRGB /Filter /DCTDecode /SMask 9 0 R"),
        format!("{IMAGE} /ColorSpace /DeviceRGB /Filter /DCT"),
        format!("{IMAGE} /ColorSpace /DeviceGray"),
        format!("{IMAGE} /ColorSpace /DeviceRGB /Filter [/DCTDecode] /Mask null"),
    ];
    let mut objects: Vec<(&str, &[u8])> = streams
        .iter()
        .take(5)
        .map(|dictionary| {
            let data: &[u8] = if dictionary.contains("DCT") {
                &jpeg
            } else {
                &gray
            };
            (dictionary.as_str(), data)
        })
        .collect();
    objects.push((&streams[5], &jpeg));
    objects.push(("/Type /XObject /Subtype /Image /Width 17 /Height 17 /BitsPerComponent 8 /ColorSpace /DeviceGray", &gray));
    let document = Document::open(
        pdf(&objects),
        &OpenOptions {
            recovery: Recovery::Repair,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    for number in 3..=7 {
        let image = ImageSource::from_xobject(
            &document,
            document.object(ObjectId::new(number, 0)).unwrap(),
        );
        if let Ok(image) = image {
            assert_eq!(image.drawn(100.0), Reduce::Full, "object {number}");
        }
    }
    assert_eq!(
        source(&document, 8).drawn(100.0),
        Reduce::Drawn { cells: 1 }
    );
    // A reduction asked of an image that may not be reduced decodes fully.
    let gray = source(&document, 7);
    let decoded = gray
        .decode(
            &DecodeRequest::new(Reduce::Drawn { cells: 1 }, None, Layout::Interleaved),
            &document.limits(),
            Recovery::Strict,
        )
        .unwrap();
    assert_eq!((decoded.image.width, decoded.image.reduction), (17, None));
}

#[test]
fn sources_outlive_their_document_and_expose_masks_without_decoding() {
    let samples: Vec<u8> = (0..12).collect();
    let mask = [255u8, 128, 0, 64];
    let image = "/Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 \
         /ColorSpace /DeviceRGB /SMask 4 0 R /Intent /Perceptual /Interpolate true";
    let document = open(pdf(&[
        (image, &samples),
        (
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 \
             /ColorSpace /DeviceGray /Matte [0 0.5 1]",
            &mask,
        ),
    ]));
    let source = source(&document, 3);
    let limits = document.limits();
    drop(document);
    assert_eq!(
        source.mask(),
        &SourceMask::Soft {
            object: ObjectId::new(4, 0),
            matte: Some(vec![0.0, 0.5, 1.0]),
        }
    );
    assert_eq!(source.intent(), Some(&Value::name(b"Perceptual")));
    assert!(source.interpolate());
    assert_eq!((source.components(), source.bits_per_component()), (3, 8));
    assert_eq!(source.color_space(), Some(&Value::name(b"DeviceRGB")));
    assert!(source.retained_bytes() >= samples.len());
    let decoded = source
        .decode(&DecodeRequest::FULL, &limits, Recovery::Strict)
        .unwrap();
    assert_eq!(decoded.image.samples, Samples::Bytes(samples));
    assert_eq!(decoded.image.mask, MaskKind::Soft(ObjectId::new(4, 0)));
    assert_eq!(decoded.diagnostics.len(), 0);
}

#[test]
fn stencils_report_their_polarity_and_pack_their_samples_on_request() {
    let rows = [0b1010_0000u8, 0b0101_0000];
    let stencil = "/Type /XObject /Subtype /Image /Width 4 /Height 2 /ImageMask true /Decode [1 0]";
    let document = open(pdf(&[(stencil, &rows)]));
    let source = source(&document, 3);
    assert_eq!(source.kind(), &ImageKind::Stencil { inverted: true });
    let packed = source
        .decode(
            &DecodeRequest::new(Reduce::Full, None, Layout::Native),
            &document.limits(),
            Recovery::Strict,
        )
        .unwrap()
        .image;
    let Samples::Packed(packed) = &packed.samples else {
        panic!("one-bit samples stay packed");
    };
    assert_eq!(packed.row(1), Some(&rows[1..]));
}

/// Width, height and components from a JPEG's frame header.
fn frame(jpeg: &[u8]) -> Option<(u16, u16, u8)> {
    let mut at = 2;
    while at + 9 < jpeg.len() {
        if jpeg[at] != 0xff {
            return None;
        }
        let marker = jpeg[at + 1];
        let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        if matches!(marker, 0xc0..=0xc2) {
            let height = u16::from_be_bytes([jpeg[at + 5], jpeg[at + 6]]);
            let width = u16::from_be_bytes([jpeg[at + 7], jpeg[at + 8]]);
            return Some((width, height, jpeg[at + 9]));
        }
        at += 2 + length;
    }
    None
}

#[test]
fn native_cell_planes_hold_the_interleaved_cells() {
    let directory = pagura_testkit::workspace_root().join("corpus/generated/image");
    let mut planar = 0;
    for entry in std::fs::read_dir(directory).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("jpeg") || path.extension().is_none_or(|e| e != "in") {
            continue;
        }
        let jpeg = std::fs::read(&path).unwrap();
        let Some((width, height, components)) = frame(&jpeg) else {
            continue;
        };
        let space = match components {
            1 => "/DeviceGray",
            3 => "/DeviceRGB",
            _ => continue,
        };
        let dictionary = format!(
            "/Type /XObject /Subtype /Image /Width {width} /Height {height} \
             /BitsPerComponent 8 /ColorSpace {space} /Filter /DCTDecode"
        );
        let document = open(pdf(&[(&dictionary, &jpeg)]));
        let Ok(image) =
            ImageSource::from_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap())
        else {
            continue;
        };
        for cells in [1, 2, 3, 4, 6] {
            let decode = |layout| {
                image.decode(
                    &DecodeRequest::new(Reduce::Drawn { cells }, None, layout),
                    &document.limits(),
                    Recovery::Repair,
                )
            };
            let (Ok(native), Ok(interleaved)) =
                (decode(Layout::Native), decode(Layout::Interleaved))
            else {
                continue;
            };
            let (native, interleaved) = (native.image, interleaved.image);
            assert_eq!(
                (native.width, native.height, native.reduction),
                (interleaved.width, interleaved.height, interleaved.reduction),
                "{name} {cells}"
            );
            match (&native.samples, &interleaved.samples) {
                (Samples::Planar(planes), Samples::Bytes(bytes)) => {
                    planar += 1;
                    let row = usize::try_from(native.width).unwrap() * 3;
                    let mut converted = vec![0; row];
                    for (y, expected) in bytes.chunks_exact(row).enumerate() {
                        assert!(planes.rgb(y, &mut converted));
                        assert_eq!(converted, expected, "{name} {cells} row {y}");
                    }
                }
                (native, interleaved) => assert_eq!(native, interleaved, "{name} {cells}"),
            }
        }
    }
    assert!(planar > 0, "no JPEG decoded into native cell planes");
}
