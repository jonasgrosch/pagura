//! `font`: font programs parse, and every glyph's outline and advance load,
//! within budgets or with a structured error (ADR 0009). The first byte
//! selects the [`FontFormat`]; the rest is the program.

use pagura_font::{CmapSubtable, FontFormat, FontProgram, GlyphId};

const FORMATS: [FontFormat; 4] = [
    FontFormat::Type1,
    FontFormat::TrueType,
    FontFormat::Cff,
    FontFormat::OpenType,
];

const SUBTABLES: [CmapSubtable; 3] = [
    CmapSubtable::WindowsUnicode,
    CmapSubtable::WindowsSymbol,
    CmapSubtable::MacRoman,
];

/// Parses the program and asks for every glyph.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, program)) = data.split_first() else {
        return;
    };
    let format = FORMATS[usize::from(selector) % FORMATS.len()];
    let limits = super::limits();
    let Some(font) = super::checked(FontProgram::parse(program.to_vec(), format, &limits)) else {
        return;
    };
    assert_eq!(font.format(), format, "parsed program changed its format");
    let _ = (
        font.is_cid_keyed(),
        font.font_matrix(),
        font.builtin_encoding(),
    );
    // `Limits::font_entries` caps the glyph count, so this loop is bounded.
    for glyph in (0..font.glyph_count()).map(GlyphId) {
        super::checked(font.outline(glyph));
        let _ = font.advance(glyph);
    }
    for name in [b".notdef".as_slice(), b"space", b"A", b"a"] {
        if let Some(glyph) = font.glyph_for_name(name) {
            super::checked(font.outline(glyph));
        }
    }
    for subtable in SUBTABLES {
        if font.has_cmap_subtable(subtable) {
            for code in (0..256).chain([0xf020, 0xf041, 0xffff, 0x1_0000]) {
                let _ = font.glyph_for_code(subtable, code);
            }
        }
    }
    for cid in 0..256 {
        let _ = font.glyph_for_cid(cid);
    }
}
