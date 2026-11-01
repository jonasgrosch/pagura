use super::{Cmap, Format, Segments, u16_at, validate, validate_four};
use crate::budget::{FontBudget, Work};
use crate::read::cursor;
use crate::{CmapSubtable, GlyphId};
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

/// A format 4 subtable body: ends, pad, starts, deltas, range offsets
/// and one glyph array word, after the 14-byte header.
fn segments(
    ends: &[u16],
    starts: &[u16],
    deltas: &[u16],
    offsets: &[u16],
    array: &[u16],
) -> Vec<u8> {
    let mut data = vec![0; 14];
    for words in [ends, &[0], starts, deltas, offsets, array] {
        data.extend(words.iter().flat_map(|w| w.to_be_bytes()));
    }
    data
}

#[test]
fn format_four_deltas_range_offsets_and_the_sentinel() {
    // Segment 0 maps 10..=12 through the glyph array; segment 1 is
    // the sentinel, mapped by delta 1 to glyph 0.
    let data = segments(&[12, 0xffff], &[10, 0xffff], &[5, 1], &[4, 0], &[7, 0, 9]);
    let s = Segments {
        data: &data,
        count: 2,
    };
    // idRangeOffset 4 from its own word reaches the array's first word.
    assert_eq!(s.lookup(10), Some(12));
    assert_eq!(s.lookup(11), Some(0));
    assert_eq!(s.lookup(12), Some(14));
    assert_eq!(s.lookup(9), Some(0));
    assert_eq!(s.lookup(0xffff), Some(0));
    let truncated = &data[..data.len() - 2];
    assert_eq!(
        Segments {
            data: truncated,
            count: 2
        }
        .lookup(12),
        None
    );
    assert_eq!(u16_at(&data, data.len() - 1), None);
}

/// A whole format 4 subtable: 20..22 use delta -31; 30..32 use a glyph
/// array and delta -1; ffff is the required missing-code sentinel.
fn format_four() -> Vec<u8> {
    [
        4_u16, 46, 0, 6, 4, 1, 2, 0x22, 0x32, 0xffff, 0, 0x20, 0x30, 0xffff, 0xffe1, 0xffff, 1, 0,
        4, 0, 5, 0, 9,
    ]
    .into_iter()
    .flat_map(u16::to_be_bytes)
    .collect()
}

#[test]
fn format_four_validates_both_mapping_routes_and_keeps_work_charges() {
    let data = format_four();
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    // Seven codes are validated: one work unit each.
    for (work, valid) in [(7, true), (6, false)] {
        let limits = Limits {
            font_work: work,
            ..Limits::default()
        };
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let result = validate_four(cursor(&data), 9, &mut work);
        if valid {
            assert_eq!(result.unwrap(), 3);
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
    }
    assert_eq!(diagnostics.as_slice(), []);
    let segments = Segments {
        data: &data,
        count: 3,
    };
    for (code, glyph) in [
        (0x1f, 0),
        (0x20, 1),
        (0x21, 2),
        (0x22, 3),
        (0x23, 0),
        (0x30, 4),
        (0x31, 0),
        (0x32, 8),
        (0xffff, 0),
    ] {
        assert_eq!(segments.lookup(code), Some(glyph));
    }
}

#[test]
fn disordered_end_codes_preserve_the_structural_error_position() {
    let mut data = format_four();
    data[14..16].copy_from_slice(&0x32_u16.to_be_bytes());
    data[16..18].copy_from_slice(&0x22_u16.to_be_bytes());
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
    let error = validate_four(cursor(&data), 9, &mut work).unwrap_err();
    assert_eq!(error.message, "overlapping cmap segments");
    assert_eq!(error.position, Position::Buffer(40));
}

/// Independently encoded sequential groups. Format 8's bitmap marks
/// high words 1 and D83D; neither is also a standalone mapped code.
fn group_table(format: u16, groups: &[[u32; 3]]) -> Vec<u8> {
    let mut data = format.to_be_bytes().to_vec();
    data.extend([0; 10]);
    if format == 8 {
        let mut bitmap = [0; 8192];
        bitmap[0] = 0x40;
        bitmap[0x1b07] = 0x04;
        data.extend(bitmap);
    }
    data.extend(u32::try_from(groups.len()).unwrap().to_be_bytes());
    data.extend(
        groups
            .iter()
            .flatten()
            .flat_map(|value| value.to_be_bytes()),
    );
    let length = u32::try_from(data.len()).unwrap();
    data[4..8].copy_from_slice(&length.to_be_bytes());
    data
}

fn group_cmap(data: &[u8], format: Format, work: &mut Work<'_>) -> pagura_core::Result<Cmap> {
    let (length, layout) = validate(cursor(data), format, 8, work)?.unwrap();
    Ok(Cmap {
        selection: CmapSubtable::WindowsUnicodeFull,
        range: 0..length,
        layout,
    })
}

#[test]
fn mixed_length_groups_use_complete_numeric_codes_and_bound_group_work() {
    let data = group_table(
        8,
        &[
            [0x41, 0x43, 1],
            [0x1f600, 0x1f601, 4],
            [0xd83d_de00, 0xd83d_de01, 6],
        ],
    );
    for (units, entries, valid) in [(3, 3, true), (2, 3, false), (3, 2, false)] {
        let limits = Limits {
            font_work: units,
            font_entries: entries,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let result = group_cmap(&data, Format::MixedLength, &mut work);
        if valid {
            let cmap = result.unwrap();
            for (code, expected) in [
                (0x41, 1),
                (0x42, 2),
                (0x43, 3),
                (0x1f600, 4),
                (0x1f601, 5),
                (0xd83d_de00, 6),
                (0xd83d_de01, 7),
            ] {
                assert_eq!(cmap.lookup(&data, code), Some(GlyphId(expected)));
            }
            for code in [0, 1, 0x40, 0x44, 0xd83d, 0x1f602, u32::MAX] {
                assert_eq!(cmap.lookup(&data, code), None);
            }
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
        assert_eq!(diagnostics.as_slice(), []);
    }
}

#[test]
fn mixed_length_header_and_group_truncations_preserve_existing_recovery() {
    let data = group_table(8, &[[0x41, 0x43, 1], [0x1f600, 0x1f601, 4]]);
    for end in [
        0, 1, 3, 7, 11, 12, 4108, 8203, 8204, 8207, 8208, 8219, 8220, 8231,
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        assert_eq!(
            validate(cursor(&data[..end]), Format::MixedLength, 8, &mut work)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax,
            "prefix {end}"
        );
    }
    // The first complete group survives Repair; the incomplete second
    // group is absent and repeated truncation reports are deduplicated.
    let data = &data[..8225];
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
    let cmap = group_cmap(data, Format::MixedLength, &mut work).unwrap();
    assert_eq!(cmap.lookup(data, 0x42), Some(GlyphId(2)));
    assert_eq!(cmap.lookup(data, 0x1f600), None);
    assert_eq!(diagnostics.as_slice().len(), 1);
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(crate::repairs::CMAP_TRUNCATED)
    );
}

#[test]
fn group_validation_rejects_disorder_and_glyph_overflow_without_expansion() {
    for groups in [
        vec![[0x43, 0x41, 1]],
        vec![[0x41, 0x43, 1], [0x43, 0x44, 4]],
        vec![[0x41, 0x43, 6]],
        vec![[0x41, 0x43, u32::MAX]],
    ] {
        let data = group_table(8, &groups);
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let error = group_cmap(&data, Format::MixedLength, &mut work).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, "invalid cmap group");
    }
    // The shared group reader still distinguishes sequential and
    // many-to-one maps, and retains their Unicode range ceiling.
    for (number, format, last) in [(12, Format::Coverage, 3), (13, Format::ManyToOne, 1)] {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let data = group_table(number, &[[0x41, 0x43, 1]]);
        let cmap = group_cmap(&data, format, &mut work).unwrap();
        assert_eq!(cmap.lookup(&data, 0x43), Some(GlyphId(last)));
        let data = group_table(number, &[[0x11_0000, 0x11_0000, 1]]);
        assert_eq!(
            group_cmap(&data, format, &mut work).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
}
