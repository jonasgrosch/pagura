//! Text strings against byte sequences written out from clause 7.9.2.2 and
//! Annex D, independent of the encoder.

use super::{decode, encode, encode_pdf_doc, pdf_doc_byte, pdf_doc_char};
use crate::repairs::{INVALID_TEXT, LANGUAGE_ESCAPE};
use crate::{Diagnostic, Diagnostics, ErrorKind, Limits, Recovery};

fn strict(bytes: &[u8]) -> crate::Result<String> {
    decode(
        bytes,
        &Limits::default(),
        &mut Diagnostics::new(Recovery::Strict),
    )
}

fn repaired(bytes: &[u8]) -> (String, Vec<crate::Repair>) {
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let text = decode(bytes, &Limits::default(), &mut diagnostics).unwrap();
    let tags = diagnostics
        .take()
        .iter()
        .filter_map(Diagnostic::repair)
        .collect();
    (text, tags)
}

#[test]
fn pdf_doc_encoding_matches_annex_d() {
    let undefined: Vec<u8> = (0..=255).filter(|&b| pdf_doc_char(b).is_none()).collect();
    let mut expected: Vec<u8> = (0x00..=0x08).collect();
    expected.extend([0x0b, 0x0c]);
    expected.extend(0x0e..=0x17);
    expected.extend([0x7f, 0x9f, 0xad]);
    assert_eq!(undefined, expected);
    for (byte, character) in [
        (0x09, '\t'),
        (0x18, '\u{02d8}'),
        (0x1f, '\u{02dc}'),
        (0x41, 'A'),
        (0x80, '\u{2022}'),
        (0x8a, '\u{2212}'),
        (0x93, '\u{fb01}'),
        (0x9e, '\u{017e}'),
        (0xa0, '\u{20ac}'),
        (0xa1, '\u{a1}'),
        (0xff, '\u{ff}'),
    ] {
        assert_eq!(pdf_doc_char(byte), Some(character), "{byte:#04x}");
    }
    for byte in 0..=255 {
        if let Some(character) = pdf_doc_char(byte) {
            assert_eq!(pdf_doc_byte(character), Some(byte), "{byte:#04x}");
        }
    }
}

#[test]
fn legacy_passwords_have_no_lossy_replacements() {
    assert_eq!(
        encode_pdf_doc("€éŁﬁ˘"),
        Some(vec![0xa0, 0xe9, 0x95, 0x93, 0x18])
    );
    assert_eq!(encode_pdf_doc("ASCII"), Some(b"ASCII".to_vec()));
    for text in ["\u{a0}", "\u{ad}", "🙂", "\u{7f}", "\u{9f}", "\u{1b}"] {
        assert_eq!(encode_pdf_doc(text), None, "{text:?}");
    }
}

#[test]
fn byte_order_marks_select_unicode_and_language_escapes_are_metadata() {
    assert_eq!(strict(b"Caf\xe9 \x80").unwrap(), "Café •");
    assert_eq!(
        strict(&[0xfe, 0xff, 0x00, 0x41, 0xd8, 0x3d, 0xde, 0x00]).unwrap(),
        "A😀"
    );
    assert_eq!(strict(b"\xef\xbb\xbfh\xc3\xa9").unwrap(), "hé");
    // UTF-16: escape, one or two code units of ASCII letters, escape.
    for bytes in [
        [0xfe, 0xff, 0x00, 0x1b, b'e', b'n', 0x00, 0x1b, 0x00, 0x41].as_slice(),
        &[
            0xfe, 0xff, 0, 0x1b, b'e', b'n', b'U', b'S', 0, 0x1b, 0, 0x41,
        ],
    ] {
        assert_eq!(strict(bytes).unwrap(), "A");
    }
    // UTF-8: escape, two or four ASCII letters, escape.
    assert_eq!(strict(b"\xef\xbb\xbf\x1bja\x1bhi").unwrap(), "hi");
    assert_eq!(strict(b"\xef\xbb\xbfh\x1benUS\x1bi").unwrap(), "hi");
    // In PDFDocEncoding 0x1B is a dot accent, not an escape.
    assert_eq!(strict(b"\x1ben\x1b").unwrap(), "\u{02d9}en\u{02d9}");
    assert_eq!(strict(b"").unwrap(), "");
    assert_eq!(strict(&[0xfe, 0xff]).unwrap(), "");
}

#[test]
fn invalid_sequences_are_strict_errors_or_one_reported_replacement() {
    for (bytes, expected) in [
        ([0xfe, 0xff, 0x00, 0x41, 0x00].as_slice(), "A\u{fffd}"),
        (
            &[0xfe, 0xff, 0xdc, 0x00, 0xd8, 0x00, 0x00, 0x42],
            "\u{fffd}\u{fffd}B",
        ),
        (b"\xef\xbb\xbfa\xffb\xc3", "a\u{fffd}b\u{fffd}"),
        (b"a\x7fb\x00", "a\u{fffd}b\u{fffd}"),
    ] {
        let error = strict(bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.repair),
            (ErrorKind::Syntax, Some(INVALID_TEXT))
        );
        assert_eq!(repaired(bytes), (expected.to_owned(), vec![INVALID_TEXT]));
    }
}

#[test]
fn malformed_language_escapes_are_strict_errors_or_dropped() {
    for (bytes, expected) in [
        ([0xfe, 0xff, 0x00, 0x1b, 0x00, 0x41].as_slice(), "A"),
        (&[0xfe, 0xff, 0x00, 0x1b, b'e', b'n'], "\u{656e}"),
        (b"\xef\xbb\xbf\x1be\x1bx", "e\u{1b}x"),
        (b"\xef\xbb\xbf\x1b1a\x1b", "1a"),
    ] {
        let error = strict(bytes).unwrap_err();
        assert_eq!(error.repair, Some(LANGUAGE_ESCAPE), "{bytes:?}");
        let (text, tags) = repaired(bytes);
        assert_eq!(text, expected.replace('\u{1b}', ""), "{bytes:?}");
        assert_eq!(tags, [LANGUAGE_ESCAPE]);
    }
}

#[test]
fn input_and_output_are_bounded_by_the_string_budget() {
    let limits = Limits {
        string_bytes: 4,
        ..Limits::default()
    };
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    assert_eq!(decode(b"abcd", &limits, &mut diagnostics).unwrap(), "abcd");
    for bytes in [b"abcde".as_slice(), &[0xa0, 0xa0]] {
        let error = decode(bytes, &limits, &mut diagnostics).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{bytes:?}");
    }
}

#[test]
fn encoding_prefers_pdf_doc_and_round_trips() {
    assert_eq!(encode("Hello €"), b"Hello \xa0");
    assert_eq!(encode("😀"), [0xfe, 0xff, 0xd8, 0x3d, 0xde, 0x00]);
    // PDFDocEncoding bytes that would read as a byte order mark.
    assert_eq!(encode("þÿ"), [0xfe, 0xff, 0x00, 0xfe, 0x00, 0xff]);
    assert_eq!(&encode("ï»¿x")[..2], [0xfe, 0xff]);
    for text in ["", "plain", "€éŁﬁ˘", "日本語", "þÿ", "a\u{ad}b", "😀\u{1b}"] {
        let bytes = encode(text);
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let decoded = decode(&bytes, &Limits::default(), &mut diagnostics).unwrap();
        if text.contains('\u{1b}') {
            assert_eq!(diagnostics.as_slice()[0].repair(), Some(LANGUAGE_ESCAPE));
        } else {
            assert_eq!((decoded.as_str(), diagnostics.is_empty()), (text, true));
        }
    }
}
