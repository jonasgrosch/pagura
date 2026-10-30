use super::*;
use crate::budget::FontBudget;
use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};

fn word(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend(value.to_be_bytes());
}

fn long(bytes: &mut Vec<u8>, value: usize) {
    bytes.extend(u32::try_from(value).unwrap().to_be_bytes());
}

fn name_table(name: &str, platform: u16, encoding: u16, version: u16) -> Vec<u8> {
    let name = if (platform == 1 && encoding == 0) || (platform == 3 && (3..=5).contains(&encoding))
    {
        name.as_bytes().to_vec()
    } else {
        name.encode_utf16().flat_map(u16::to_be_bytes).collect()
    };
    let mut bytes = Vec::new();
    for value in [
        version,
        1,
        if version == 1 { 24 } else { 18 },
        platform,
        encoding,
        if version == 1 { 0x8000 } else { 0 },
        6,
        u16::try_from(name.len()).unwrap(),
        0,
    ] {
        word(&mut bytes, value);
    }
    if version == 1 {
        word(&mut bytes, 1);
        word(&mut bytes, 4);
        word(&mut bytes, u16::try_from(name.len()).unwrap());
    }
    bytes.extend(name);
    if version == 1 {
        bytes.extend([0, b'e', 0, b'n']);
    }
    bytes
}

/// Minimal directories suffice to test selection without any glyph parser.
fn collection(tables: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * tables.len();
    let mut bytes = b"ttcf\0\x01\0\0".to_vec();
    long(&mut bytes, tables.len());
    for index in 0..tables.len() {
        long(&mut bytes, header + 28 * index);
    }
    let mut offset = header + 28 * tables.len();
    for table in tables {
        long(&mut bytes, 0x10000);
        word(&mut bytes, 1);
        bytes.extend([0; 6]);
        bytes.extend(b"name");
        long(&mut bytes, 0);
        long(&mut bytes, offset);
        long(&mut bytes, table.len());
        offset += table.len();
    }
    for table in tables {
        bytes.extend(table);
    }
    bytes
}

fn choose(bytes: &[u8], names: &[&[u8]]) -> Result<usize> {
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
    select(bytes, FontFace::PostScriptNames(names), &mut work).map(|s| s.offset)
}

#[test]
fn naming_versions_encodings_and_priority_share_one_selection() {
    for version in [0, 1] {
        for (platform, encoding) in [
            (0, 4),
            (3, 0),
            (3, 1),
            (3, 10),
            (1, 0),
            (3, 3),
            (3, 4),
            (3, 5),
        ] {
            let bytes = collection(&[
                name_table("First", platform, encoding, version),
                name_table("Second", platform, encoding, version),
            ]);
            assert_eq!(choose(&bytes, &[b"Second", b"First"]).unwrap(), 48);
            assert_eq!(choose(&bytes, &[b"Missing", b"First"]).unwrap(), 20);
        }
    }
}

#[test]
fn ambiguity_does_not_choose_the_first_face_or_ignore_name_priority() {
    let bytes = collection(&[
        name_table("Same", 3, 1, 0),
        name_table("Same", 3, 1, 0),
        name_table("Unique", 3, 1, 0),
    ]);
    assert_eq!(
        choose(&bytes, &[b"Same", b"Unique"]).unwrap_err().message,
        "ambiguous font collection name"
    );
    assert_eq!(choose(&bytes, &[b"Unique", b"Same"]).unwrap(), 80);
    assert_eq!(
        choose(&bytes, &[b"same"]).unwrap_err().message,
        "font collection name not found"
    );
    assert_eq!(choose(&bytes, &[]).unwrap_err().kind, ErrorKind::Syntax);
}

#[test]
fn truncated_headers_directories_and_selected_name_strings_are_rejected() {
    let bytes = collection(&[name_table("Only", 3, 1, 0)]);
    for end in 4..bytes.len() {
        assert_eq!(
            choose(&bytes[..end], &[b"Only"]).unwrap_err().kind,
            ErrorKind::Syntax,
            "{end}"
        );
    }
    let mut empty = bytes.clone();
    empty[8..12].fill(0);
    assert_eq!(
        choose(&empty, &[b"Only"]).unwrap_err().message,
        "empty font collection"
    );
    let mut header_overlap = bytes.clone();
    header_overlap[12..16].copy_from_slice(&8_u32.to_be_bytes());
    assert_eq!(
        choose(&header_overlap, &[b"Only"]).unwrap_err().message,
        "font directory overlaps collection header"
    );
    let mut future = bytes;
    future[4..8].copy_from_slice(&0x30000_u32.to_be_bytes());
    assert_eq!(
        choose(&future, &[b"Only"]).unwrap_err().kind,
        ErrorKind::Unsupported(features::FORMAT)
    );
}

#[test]
fn malformed_and_unknown_name_records_cannot_select_a_face() {
    let original = name_table("Only", 3, 1, 1);
    for (field, value) in [(4, 6_u16), (14, 7), (16, u16::MAX)] {
        let mut table = original.clone();
        table[field..field + 2].copy_from_slice(&value.to_be_bytes());
        assert_eq!(
            choose(&collection(&[table]), &[b"Only"]).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
    for (field, value) in [(0, 2_u16), (6, 9), (10, 0x8001)] {
        let mut table = original.clone();
        table[field..field + 2].copy_from_slice(&value.to_be_bytes());
        assert_eq!(
            choose(&collection(&[table]), &[b"Only"])
                .unwrap_err()
                .message,
            "font collection name not found"
        );
    }
    for name in ["", "Bad Name", "Slash/Name", "Nämé"] {
        assert_eq!(
            choose(
                &collection(&[name_table(name, 3, 1, 0)]),
                &[name.as_bytes()]
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn name_scan_work_is_charged_before_untrusted_directory_counts() {
    let bytes = collection(&[name_table("First", 3, 1, 0), name_table("Second", 3, 1, 0)]);
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let mut work = Work::new(
        FontBudget::new(&Limits {
            font_work: 1,
            ..Limits::default()
        }),
        &mut diagnostics,
    );
    let error = select(&bytes, FontFace::PostScriptNames(&[b"Second"]), &mut work)
        .err()
        .unwrap();
    assert_eq!(
        (error.kind, error.position),
        (ErrorKind::Limit, Position::Buffer(8))
    );
}
