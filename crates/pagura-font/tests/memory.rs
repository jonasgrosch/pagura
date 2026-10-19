#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "fixture setup fails loudly on malformed expected data"
)]
//! Estimates describe retained representation, not parse or outline scratch.
use pagura_core::Limits;
use pagura_font::{FontFormat, FontProgram, GlyphId};

fn program(name: &str, format: FontFormat) -> FontProgram {
    FontProgram::parse(
        pagura_testkit::fixture("font", &format!("memory-{name}.font")),
        format,
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn estimates_are_cached_and_shared_across_clones_and_outlining() {
    for (name, format) in [
        ("truetype", FontFormat::TrueType),
        ("cff", FontFormat::Cff),
        ("cff-cid", FontFormat::Cff),
        ("cff2", FontFormat::OpenType),
        ("type1-small", FontFormat::Type1),
        ("type1-large", FontFormat::Type1),
    ] {
        let p = program(name, format);
        let bytes = p.retained_bytes();
        assert!(bytes > p.data().len());
        let clone = p.clone();
        for glyph in 0..p.glyph_count() {
            p.outline(GlyphId(glyph)).unwrap();
        }
        assert_eq!(p.retained_bytes(), bytes);
        assert_eq!(clone.retained_bytes(), bytes);
    }
}

#[test]
fn original_input_is_charged_once_without_a_size_multiplier() {
    let p = program("truetype", FontFormat::TrueType);
    let mut padded = p.data().to_vec();
    padded.resize(padded.len() + 4096, 0);
    let padded = FontProgram::parse(padded, FontFormat::TrueType, &Limits::default()).unwrap();
    assert_eq!(padded.retained_bytes(), p.retained_bytes() + 4096);
}

#[test]
fn decrypted_subroutines_are_counted_even_with_the_same_glyph_count() {
    let small = program("type1-small", FontFormat::Type1);
    let large = program("type1-large", FontFormat::Type1);
    assert_eq!(small.glyph_count(), large.glyph_count());
    // The independent writer adds 64 return-only subroutines. Each retains
    // one decrypted byte, its Vec descriptor and the offset that positions
    // its errors, with all other state equal.
    assert_eq!(
        (large.retained_bytes() - large.data().len())
            - (small.retained_bytes() - small.data().len()),
        64 * (size_of::<Vec<u8>>() + size_of::<usize>() + 1)
    );
}
