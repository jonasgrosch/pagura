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
        return symbol(program, code)
            .or_else(|| mapped(program, CmapSubtable::MacRoman, code))
            .or_else(|| raw_unicode(program, code));
    }
    let name = name?;
    let selected = if program.has_cmap_subtable(CmapSubtable::WindowsUnicode) {
        scalar(unicode).and_then(|code| mapped(program, CmapSubtable::WindowsUnicode, code))
    } else if program.has_cmap_subtable(CmapSubtable::MacRoman) {
        mac_os_code(name.as_bytes()).and_then(|code| mapped(program, CmapSubtable::MacRoman, code))
    } else {
        None
    };
    // Preserve the mandated post route before reader-chosen alternatives.
    selected
        .or_else(|| program.glyph_for_name(name.as_bytes()))
        .or_else(|| named_fallback(program, code, name, unicode))
}

// 9.6.5.4's final paragraph permits reader-selected mappings. Policy: after
// the normative routes (including post) fail, try Mac Roman by glyph name,
// then full Windows Unicode, platform 0 Unicode, then the symbol byte route.
// ToUnicode is deliberately never consulted for painting.
fn named_fallback(
    program: &FontProgram,
    code: u32,
    name: &Name,
    unicode: Option<&str>,
) -> Option<GlyphId> {
    mac_os_code(name.as_bytes())
        .and_then(|code| mapped(program, CmapSubtable::MacRoman, code))
        .or_else(|| {
            scalar(unicode).and_then(|code| {
                [CmapSubtable::WindowsUnicodeFull, CmapSubtable::Unicode]
                    .into_iter()
                    .find_map(|table| mapped(program, table, code))
            })
        })
        .or_else(|| symbol(program, code))
}

fn scalar(text: Option<&str>) -> Option<u32> {
    let mut chars = text?.chars();
    let value = chars.next()?;
    chars.next().is_none().then_some(u32::from(value))
}

fn symbol(program: &FontProgram, code: u32) -> Option<GlyphId> {
    [0, 0xf000, 0xf100, 0xf200]
        .into_iter()
        .find_map(|high| mapped(program, CmapSubtable::WindowsSymbol, code + high))
}

// Without an Encoding, byte values are the only available character identity.
// Keep symbol/Mac priority; only on failure try Unicode subtables in this
// order, using literal bytes before the three symbol high-byte alternatives.
fn raw_unicode(program: &FontProgram, code: u32) -> Option<GlyphId> {
    [
        CmapSubtable::WindowsUnicode,
        CmapSubtable::WindowsUnicodeFull,
        CmapSubtable::Unicode,
    ]
    .into_iter()
    .find_map(|table| {
        [0, 0xf000, 0xf100, 0xf200]
            .into_iter()
            .find_map(|high| mapped(program, table, code + high))
    })
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
