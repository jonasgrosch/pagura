use super::parse;
use crate::{
    FontFormat, FontProgram, Program,
    budget::{FontBudget, Work},
    read,
};
use pagura_core::{Diagnostics, Error, ErrorKind, Limits, Recovery, Result};

fn bytes(name: &str) -> Result<Vec<u8>> {
    let program = FontProgram::parse(
        pagura_testkit::fixture("vertical_fonts", &format!("{name}.font")),
        FontFormat::TrueType,
        &Limits::default(),
    )?;
    let Program::Sfnt(font) = program.parsed.as_ref() else {
        return Err(Error::syntax("expected sfnt fixture"));
    };
    let range = font
        .gsub
        .clone()
        .ok_or_else(|| Error::syntax("missing GSUB fixture"))?;
    Ok(program.data()[range].to_vec())
}

#[test]
fn truncated_selected_tables_fail_without_publishing_a_map() -> Result<()> {
    let bytes = bytes("single-delta")?;
    for end in 0..bytes.len() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        assert!(
            parse(
                read::cursor(&bytes[..end]),
                *b"hani",
                Some(*b"JAN "),
                5,
                &mut work
            )
            .is_err(),
            "prefix {end}"
        );
    }
    Ok(())
}

#[test]
fn layout_work_and_entries_have_exact_independent_boundaries() -> Result<()> {
    let bytes = bytes("single-delta")?;
    // Nine metadata/glyph visits, four admitted index/mapping entries in
    // this literal one-feature, one-lookup, one-glyph table.
    for (font_work, font_entries, valid) in [(9, 4, true), (8, 4, false), (9, 3, false)] {
        let limits = Limits {
            font_work,
            font_entries,
            operations: 0,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let result = parse(read::cursor(&bytes), *b"hani", Some(*b"JAN "), 5, &mut work);
        if valid {
            assert!(result?.is_some());
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
    }
    Ok(())
}

#[test]
fn unused_vertical_fallback_does_not_consume_selected_entry_budget() -> Result<()> {
    let bytes = bytes("unused-vert-entries")?;
    let limits = Limits {
        font_work: 12,
        font_entries: 5,
        operations: 0,
        ..Limits::default()
    };
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
    assert!(parse(read::cursor(&bytes), *b"hani", Some(*b"JAN "), 5, &mut work)?.is_some());
    Ok(())
}
