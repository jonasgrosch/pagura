//! TrueType PDF selection routes (ISO 32000-2 9.6.5.4, Table 113).
use crate::tables;
use pagura_core::Name;
use pagura_font::{CmapSubtable, FontProgram, GlyphId};

pub(crate) fn truetype(
    program: &FontProgram,
    code: u32,
    name: Option<&Name>,
    unicode: Option<&str>,
    raw: bool,
) -> Option<GlyphId> {
    if raw {
        if program.has_cmap_subtable(CmapSubtable::WindowsSymbol) {
            // A conforming PDF symbol cmap uses one of these high bytes.
            // For unmapped characters choose .notdef, never another subtable.
            return [0, 0xf000, 0xf100, 0xf200]
                .into_iter()
                .find_map(|high| mapped(program, CmapSubtable::WindowsSymbol, code + high));
        }
        return mapped(program, CmapSubtable::MacRoman, code);
    }
    let name = name?;
    let selected = if program.has_cmap_subtable(CmapSubtable::WindowsUnicode) {
        unicode
            .and_then(|text| {
                let mut chars = text.chars();
                let scalar = chars.next()?;
                chars.next().is_none().then_some(scalar)
            })
            .and_then(|scalar| mapped(program, CmapSubtable::WindowsUnicode, u32::from(scalar)))
    } else if program.has_cmap_subtable(CmapSubtable::MacRoman) {
        mac_os_code(name.as_bytes()).and_then(|code| mapped(program, CmapSubtable::MacRoman, code))
    } else {
        None
    };
    // A missing entry does not mean its subtable is absent. The name route
    // uses post here, without trying a lower-priority cmap (9.6.5.4).
    selected.or_else(|| program.glyph_for_name(name.as_bytes()))
}

fn mapped(program: &FontProgram, subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
    program
        .glyph_for_code(subtable, code)
        .filter(|glyph| glyph.0 != 0)
}

fn mac_os_code(name: &[u8]) -> Option<u32> {
    // Table 113 adds these entries and replaces MacRoman's currency at 219.
    const ADDITIONS: &[(&[u8], u32)] = &[
        (b"notequal", 173),
        (b"infinity", 176),
        (b"lessequal", 178),
        (b"greaterequal", 179),
        (b"partialdiff", 182),
        (b"summation", 183),
        (b"product", 184),
        (b"pi", 185),
        (b"integral", 186),
        (b"Omega", 189),
        (b"radical", 195),
        (b"approxequal", 197),
        (b"Delta", 198),
        (b"lozenge", 215),
        (b"Euro", 219),
        (b"apple", 240),
    ];
    ADDITIONS
        .iter()
        .find_map(|(glyph, code)| (*glyph == name).then_some(*code))
        .or_else(|| {
            tables::MAC_ROMAN.iter().find_map(|(code, glyph)| {
                (*code != 219 && glyph.as_bytes() == name).then_some(u32::from(*code))
            })
        })
}
