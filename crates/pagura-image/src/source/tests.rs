//! Unit tests of image sources: what interpretation reads without decoding
//! and independence from the document.

use super::{DecodeRequest, ImageSource, Layout, Orientation, Reduce, SourceMask};
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

/// A JBIG2 image decoded for native layout keeps the page bitmap's padded
/// rows as packed samples, of the same values as its unpacked samples, in
/// either orientation.
#[test]
fn jbig2_native_samples_are_the_packed_page_bitmap() {
    let fixture =
        |extension| pagura_testkit::fixture("jbig2", &format!("arithmetic-layout-0-0.{extension}"));
    let (globals, data) = (fixture("globals"), fixture("jb2"));
    let dictionary = "/Type /XObject /Subtype /Image /Width 13 /Height 12 /BitsPerComponent 1 \
                      /ColorSpace /DeviceGray /Filter /JBIG2Decode \
                      /DecodeParms << /JBIG2Globals 3 0 R >>";
    let document = open(pdf(&[("", &globals), (dictionary, &data)]));
    let image = ImageSource::from_xobject(&document, document.object(ObjectId::new(4, 0)).unwrap())
        .unwrap();
    let decode = |layout, orientation| {
        image
            .decode(
                &DecodeRequest::new(Reduce::Full, None, layout).with_orientation(orientation),
                &document.limits(),
                Recovery::Strict,
            )
            .unwrap()
            .image
    };
    let unpacked = decode(Layout::Interleaved, Orientation::Rows);
    assert!(matches!(unpacked.samples, Samples::Bytes(_)));
    for orientation in [Orientation::Rows, Orientation::Transposed] {
        let packed = decode(Layout::Native, orientation);
        let expected = match orientation {
            Orientation::Rows => unpacked.clone(),
            Orientation::Transposed => unpacked.transposed(&document.limits()).unwrap(),
        };
        assert_eq!(
            (packed.width, packed.height),
            (expected.width, expected.height)
        );
        let count = usize::try_from(packed.width * packed.height).unwrap();
        let values = |image: &crate::DecodedImage| {
            (0..count).map(|i| image.samples.get(i)).collect::<Vec<_>>()
        };
        assert_eq!(values(&packed), values(&expected), "{orientation:?}");
        if orientation == Orientation::Rows {
            assert!(matches!(packed.samples, Samples::Packed(_)));
        }
    }
}
