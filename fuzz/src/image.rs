//! `image`: image codecs and image dictionaries decode within budgets or
//! fail with a structured error, and decoded samples match the dimensions
//! they report (clauses 7.4.6–7.4.10 and 8.9; ADR 0010).
//!
//! The low three bits of the first byte select the decoder:
//!
//! - 0: `dct::decode`; bits 3–4 choose `/ColorTransform` (absent, 0, 1, 2).
//! - 1: `ccitt::decode`, then reported row recovery; seven parameter bytes
//!   follow: K (signed), flags (EndOfLine, EncodedByteAlign, EndOfBlock,
//!   BlackIs1), Columns and Rows (little-endian `u16`, Rows 0 for unknown)
//!   and DamagedRowsBeforeError.
//! - 2: `jbig2::decode`; with bit 3 set, a little-endian `u16` length and
//!   that many bytes of `/JBIG2Globals` precede the embedded stream.
//! - 3: `jpx::decode`.
//! - 4: `decode_xobject`: the rest is parts that become objects 3, 4, … of a
//!   fixed document; bits 3–4 choose the image object (3 + bits) and bit 7
//!   selects strict recovery.
//! - 5: `decode_inline`: an inline image dictionary, then its data, then
//!   optional objects 3, 4, … the dictionary may refer to.
//! - 6, 7: as 4 and 5 with strict recovery.

use super::wrapper::{self, FIRST_OBJECT, Pdf};
use pagura::{Document, ObjectId, OpenOptions, Recovery};
use pagura_core::{Diagnostics, Limits};
use pagura_image::{DecodedImage, Precision, Samples};
use pagura_imagecodec::{ccitt, dct, jbig2, jpx};

/// Named colour spaces the inline image dictionary can select (8.9.7).
const RESOURCES: &[u8] = b"<< /ColorSpace << /G2 /DeviceGray /RGB2 /DeviceRGB \
    /Pal [/Indexed /DeviceRGB 1 <000000ffffff>] /Gray [/CalGray << /WhitePoint [.9505 1 1.089] >>] >> >>";

/// Decodes the input with the selected decoder.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let limits = super::limits();
    match selector & 7 {
        0 => decode_dct(rest, selector >> 3, &limits),
        1 => decode_ccitt(rest, &limits),
        2 => decode_jbig2(rest, selector & 8 != 0, &limits),
        3 => decode_jpx(rest, &limits),
        mode => {
            let strict = selector & 0x80 != 0 || mode >= 6;
            if mode % 2 == 0 {
                xobject(
                    rest,
                    u32::from((selector >> 3) & 3),
                    super::recovery(strict),
                );
            } else {
                inline(rest, super::recovery(strict));
            }
        }
    }
}

fn decode_dct(data: &[u8], transform: u8, limits: &Limits) {
    let transform = [None, Some(0), Some(1), Some(2)][usize::from(transform & 3)];
    if let Some(image) = super::checked(dct::decode(data, transform, limits)) {
        assert!(
            matches!(image.components, 1 | 3 | 4),
            "JPEG with {} components",
            image.components
        );
        assert_eq!(
            Some(image.samples.len()),
            count(image.width, image.height, image.components),
            "JPEG samples disagree with its size"
        );
    }
}

fn decode_ccitt(data: &[u8], limits: &Limits) {
    let Some((head, data)) = data.split_first_chunk::<7>() else {
        return;
    };
    let [k, flags, c0, c1, r0, r1, damaged] = *head;
    let rows = u16::from_le_bytes([r0, r1]);
    let params = ccitt::CcittParams {
        k: i64::from(i8::from_le_bytes([k])),
        end_of_line: flags & 1 != 0,
        encoded_byte_align: flags & 2 != 0,
        end_of_block: flags & 4 != 0,
        black_is_1: flags & 8 != 0,
        columns: u32::from(u16::from_le_bytes([c0, c1])),
        rows: (rows != 0).then_some(u32::from(rows)),
        damaged_rows_before_error: u32::from(damaged),
    };
    let row = usize::try_from(params.columns.div_ceil(8)).unwrap_or(usize::MAX);
    let strict = super::checked(ccitt::decode(data, &params, limits));
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut diagnostics = Diagnostics::new(recovery);
        let rows = super::checked(ccitt::decode_with_diagnostics(
            data,
            &params,
            limits,
            &mut diagnostics,
        ));
        super::assert_diagnostics(diagnostics.as_slice(), recovery);
        if recovery == Recovery::Strict {
            assert_eq!(
                rows, strict,
                "strict fax decoding differs between entry points"
            );
        }
        if let Some(rows) = rows {
            assert!(
                row > 0 && rows.len() % row == 0,
                "fax output is not whole rows"
            );
        }
    }
}

fn decode_jbig2(data: &[u8], has_globals: bool, limits: &Limits) {
    let (globals, data) = if has_globals {
        let Some((length, rest)) = data.split_first_chunk::<2>() else {
            return;
        };
        let length = usize::from(u16::from_le_bytes(*length)).min(rest.len());
        let (globals, data) = rest.split_at(length);
        (Some(globals), data)
    } else {
        (None, data)
    };
    if let Some(bitmap) = super::checked(jbig2::decode(data, globals, limits)) {
        let row = usize::try_from(bitmap.width().div_ceil(8)).ok();
        let height = usize::try_from(bitmap.height()).ok();
        assert_eq!(
            row,
            Some(bitmap.stride()),
            "JBIG2 stride disagrees with its width"
        );
        assert_eq!(
            Some(bitmap.data().len()),
            row.zip(height).and_then(|(r, h)| r.checked_mul(h)),
            "JBIG2 bitmap disagrees with its size"
        );
    }
}

fn decode_jpx(data: &[u8], limits: &Limits) {
    if let Some(image) = super::checked(jpx::decode(data, limits)) {
        assert!(
            image
                .alpha
                .is_none_or(|alpha| alpha < image.components.len()),
            "JPEG 2000 alpha component out of range"
        );
        for component in &image.components {
            assert_eq!(
                Some(component.samples.len()),
                count(component.width, component.height, 1),
                "JPEG 2000 component disagrees with its size"
            );
            assert!(
                (1..=16).contains(&component.bits)
                    && component
                        .samples
                        .iter()
                        .all(|&s| u32::from(s) < 1 << component.bits),
                "JPEG 2000 sample exceeds its precision"
            );
        }
    }
}

fn xobject(data: &[u8], root: u32, recovery: Recovery) {
    let mut pdf = Pdf::without_pages();
    pdf.parts(&wrapper::parts(data));
    let Some(document) = open(pdf, recovery) else {
        return;
    };
    if let Some(object) = super::checked(document.object(ObjectId::new(FIRST_OBJECT + root, 0)))
        && let Some(image) = super::checked(pagura_image::decode_xobject(&document, object))
    {
        assert_decoded(&image);
    }
    super::assert_diagnostics(&document.diagnostics(), recovery);
}

fn inline(data: &[u8], recovery: Recovery) {
    let parts = wrapper::parts(data);
    let [dictionary, samples, objects @ ..] = parts.as_slice() else {
        return;
    };
    let mut pdf = Pdf::without_pages();
    pdf.parts(objects);
    let Some(document) = open(pdf, recovery) else {
        return;
    };
    let limits = document.limits();
    if let (Some(resources), Some(dictionary)) = (
        wrapper::dictionary(RESOURCES, limits),
        wrapper::dictionary(dictionary, limits),
    ) {
        if let Some(length) = super::checked(pagura_image::inline_data_length(
            &dictionary,
            samples,
            &limits,
        )) {
            assert!(
                length <= samples.len(),
                "inline image data runs past its content"
            );
        }
        if let Some(image) = super::checked(pagura_image::decode_inline(
            &document,
            &resources,
            &dictionary,
            samples,
        )) {
            assert_decoded(&image);
        }
    }
    super::assert_diagnostics(&document.diagnostics(), recovery);
}

fn open(pdf: Pdf, recovery: Recovery) -> Option<Document> {
    let options = OpenOptions {
        limits: super::limits(),
        recovery,
        ..OpenOptions::default()
    };
    super::checked(Document::open(pdf.finish(), &options))
}

/// One value per component and sample, each within its bit depth.
fn assert_decoded(image: &DecodedImage) {
    assert_eq!(
        Some(image.samples().len()),
        count(image.width(), image.height(), image.components()),
        "decoded samples disagree with the image size"
    );
    let representable = match (image.precision(), image.samples()) {
        // Bytes hold 1- to 8-bit values (JPX permits any depth in that range).
        (&Precision::Uniform(bits), Samples::Bytes(values)) => {
            assert!(
                (1..=8).contains(&bits) && values.iter().all(|&v| u16::from(v) < 1 << bits),
                "{bits}-bit samples out of range"
            );
            true
        }
        (&Precision::Uniform(bits), Samples::Words(_)) => {
            assert!(
                (9..=16).contains(&bits),
                "16-bit storage for {bits}-bit samples"
            );
            true
        }
        // Mixed JPX precisions: each colour component's values fit its own depth.
        (Precision::Mixed(bits), Samples::Words(values)) => {
            assert!(
                !bits.is_empty() && bits.iter().all(|b| (1..=16).contains(b)),
                "mixed precisions out of range: {bits:?}"
            );
            let components = usize::from(image.components()).max(1);
            for (index, &value) in values.iter().enumerate() {
                if let Some(&depth) = bits.get(index % components) {
                    assert!(
                        u32::from(value) < 1 << depth,
                        "{depth}-bit mixed sample out of range"
                    );
                }
            }
            true
        }
        _ => false,
    };
    assert!(
        representable,
        "{:?} samples in the wrong representation",
        image.precision()
    );
}

fn count(width: u32, height: u32, components: u8) -> Option<usize> {
    usize::try_from(u64::from(width) * u64::from(height) * u64::from(components)).ok()
}
