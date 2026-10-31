//! Unit tests of image sources: what interpretation reads without decoding
//! and independence from the document.

use super::{DecodeRequest, ImageSource, Layout, Orientation, Reduce, SourceMask};
use crate::{ImageKind, MaskKind, Samples};
use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Value};
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
    assert_eq!(decoded.image.samples, Samples::Bytes(samples.into()));
    assert_eq!(decoded.image.mask, MaskKind::Soft(ObjectId::new(4, 0)));
    assert_eq!(decoded.diagnostics.len(), 0);
}

#[test]
fn samples_of_exactly_the_image_extent_are_the_sources_data() {
    let samples: Vec<u8> = (0..12).collect();
    let flate = pagura_codec::flate::compress(
        &samples,
        pagura_codec::flate::Level::Default,
        &Limits::default(),
    )
    .unwrap();
    let rgb = "/Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 \
               /ColorSpace /DeviceRGB";
    let document = open(pdf(&[
        (&format!("{rgb} /Filter /FlateDecode"), &flate),
        (rgb, &samples[..10]),
        (
            "/Type /XObject /Subtype /Image /Width 4 /Height 1 /BitsPerComponent 4 \
             /ColorSpace /DeviceGray",
            &[0x12, 0x34],
        ),
    ]));
    let limits = document.limits();
    let bytes = |source: &ImageSource| {
        let decoded = source
            .decode(&DecodeRequest::FULL, &limits, Recovery::Repair)
            .unwrap();
        let Samples::Bytes(bytes) = decoded.image.samples else {
            panic!("byte samples");
        };
        bytes
    };
    // The inflated stream is kept as inflated, and its bytes are the samples.
    let inflated = source(&document, 3);
    assert_eq!(inflated.data.capacity(), samples.len());
    let shared = bytes(&inflated);
    assert!(shared.shares(&inflated.data));
    assert_eq!(shared.as_slice(), samples.as_slice());
    assert_eq!(crate::deferred::storage_bytes(&Samples::Bytes(shared)), 12);
    // Short data are padded, and packed samples unpacked, into bytes of
    // their own.
    let short = source(&document, 4);
    let padded = bytes(&short);
    assert!(!padded.shares(&short.data));
    assert_eq!(padded.as_slice(), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0]);
    let packed = source(&document, 5);
    let unpacked = bytes(&packed);
    assert!(!unpacked.shares(&packed.data));
    assert_eq!(unpacked.as_slice(), [1, 2, 3, 4]);
}

#[test]
fn shared_samples_meet_the_budget_unpacked_samples_meet() {
    // Twelve 8-bit samples, the data byte for byte, and twelve 4-bit ones.
    let document = open(pdf(&[
        (
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 \
             /ColorSpace /DeviceRGB",
            &[7; 12],
        ),
        (
            "/Type /XObject /Subtype /Image /Width 4 /Height 3 /BitsPerComponent 4 \
             /ColorSpace /DeviceGray",
            &[0x77; 6],
        ),
    ]));
    for number in [3, 4] {
        let source = source(&document, number);
        let decode = |decoded_bytes| {
            let limits = Limits {
                decoded_bytes,
                ..document.limits()
            };
            source.decode(&DecodeRequest::FULL, &limits, Recovery::Strict)
        };
        assert!(decode(12).is_ok(), "object {number}");
        assert_eq!(
            decode(11).unwrap_err().kind,
            ErrorKind::Limit,
            "object {number}"
        );
    }
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
fn native_planes_hold_the_interleaved_samples_in_either_orientation() {
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
        let decode = |layout, orientation| {
            image.decode(
                &DecodeRequest::new(Reduce::Full, None, layout).with_orientation(orientation),
                &document.limits(),
                Recovery::Repair,
            )
        };
        let Ok(interleaved) = decode(Layout::Interleaved, Orientation::Rows) else {
            continue;
        };
        let interleaved = interleaved.image;
        for orientation in [Orientation::Rows, Orientation::Transposed] {
            let native = decode(Layout::Native, orientation).unwrap().image;
            let expected = match orientation {
                Orientation::Rows => interleaved.clone(),
                _ => interleaved.transposed(&document.limits()).unwrap(),
            };
            let context = format!("{name} {orientation:?}");
            assert_eq!(
                (native.width, native.height),
                (expected.width, expected.height),
                "{context}"
            );
            match (&native.samples, &expected.samples) {
                (Samples::Planar(planes), Samples::Bytes(bytes)) => {
                    planar += 1;
                    let row = usize::try_from(native.width).unwrap() * 3;
                    let mut converted = vec![0; row];
                    for (y, expected) in bytes.chunks_exact(row).enumerate() {
                        assert!(planes.rgb(y, &mut converted));
                        assert_eq!(converted, expected, "{context} row {y}");
                    }
                }
                (native, expected) => assert_eq!(native, expected, "{context}"),
            }
        }
    }
    assert!(planar > 0, "no JPEG decoded into native planes");
}
