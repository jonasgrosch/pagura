//! TrueType PDF selection routes (ISO 32000-2 9.6.5.4, Table 113).
use crate::tables;
use pagura_core::Name;
use pagura_font::{CmapSubtable, FontProgram, GlyphId};
use std::sync::LazyLock;

/// How a TrueType font's codes reach its cmap (9.6.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// Symbolic fonts and fonts without `/Encoding`: the byte itself is the
    /// character code, looked up in the (3,0) or (1,0) subtable.
    Raw,
    /// Nonsymbolic fonts with `/Encoding`: the code's glyph name, through
    /// Unicode in (3,1) or Mac Roman in (1,0), then the post table.
    Named,
}

/// The high bytes under which a (3,0) symbol subtable may place a single-byte
/// code: none, or U+F000, U+F100 or U+F200 (9.6.5.4).
const SYMBOL_PAGES: [u32; 4] = [0, 0xf000, 0xf100, 0xf200];

pub(crate) fn truetype(
    program: &FontProgram,
    code: u32,
    name: Option<&Name>,
    unicode: Option<&str>,
    route: Route,
) -> Option<GlyphId> {
    if route == Route::Raw {
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

/// Recovered descriptors require a source-named selection. Reader-selected
/// literal-byte/symbol alternatives cannot establish the recovered identity.
pub(crate) fn truetype_required_named(
    program: &FontProgram,
    name: Option<&Name>,
) -> Option<GlyphId> {
    let name = name?;
    let glyph = if program.has_cmap_subtable(CmapSubtable::WindowsUnicode) {
        required_unicode(name).and_then(|code| mapped(program, CmapSubtable::WindowsUnicode, code))
    } else if program.has_cmap_subtable(CmapSubtable::MacRoman) {
        mac_os_code(name.as_bytes()).and_then(|code| mapped(program, CmapSubtable::MacRoman, code))
    } else {
        None
    };
    glyph
        .or_else(|| program.glyph_for_name(name.as_bytes()))
        .filter(|glyph| glyph.0 != 0)
}

/// Only complete source names prove a scalar. Do not discard a suffix or an
/// unknown underscore component as the ordinary extraction policy may do.
fn required_unicode(name: &Name) -> Option<u32> {
    let name = std::str::from_utf8(name.as_bytes()).ok()?;
    if let Ok(index) = tables::UNICODE.binary_search_by_key(&name, |entry| entry.0) {
        return Some(tables::UNICODE[index].1);
    }
    let hex = if let Some(hex) = name.strip_prefix("uni").filter(|hex| hex.len() == 4) {
        hex
    } else {
        name.strip_prefix('u')
            .filter(|hex| (4..=6).contains(&hex.len()))?
    };
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
    {
        return None;
    }
    let scalar = u32::from_str_radix(hex, 16).ok()?;
    char::from_u32(scalar).map(u32::from)
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
    SYMBOL_PAGES
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
        SYMBOL_PAGES
            .into_iter()
            .find_map(|high| mapped(program, table, code + high))
    })
}

fn mapped(program: &FontProgram, subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
    program
        .glyph_for_code(subtable, code)
        .filter(|glyph| glyph.0 != 0)
}

/// The Mac OS character code of a glyph name (Table 113), by binary search.
fn mac_os_code(name: &[u8]) -> Option<u32> {
    // Table 113 adds these entries and replaces MacRoman's currency at 219.
    const ADDITIONS: [(&str, u8); 16] = [
        ("notequal", 173),
        ("infinity", 176),
        ("lessequal", 178),
        ("greaterequal", 179),
        ("partialdiff", 182),
        ("summation", 183),
        ("product", 184),
        ("pi", 185),
        ("integral", 186),
        ("Omega", 189),
        ("radical", 195),
        ("approxequal", 197),
        ("Delta", 198),
        ("lozenge", 215),
        ("Euro", 219),
        ("apple", 240),
    ];
    static CODES: LazyLock<Vec<(&'static [u8], u8)>> = LazyLock::new(|| {
        let mut codes: Vec<(&[u8], u8)> = ADDITIONS
            .iter()
            .map(|(name, code)| (name.as_bytes(), *code))
            .collect();
        // The first assignment of a name wins, as in a scan of the table.
        for &(code, name) in tables::MAC_ROMAN {
            if code != 219 && !codes.iter().any(|(known, _)| *known == name.as_bytes()) {
                codes.push((name.as_bytes(), code));
            }
        }
        codes.sort_unstable();
        codes
    });
    let index = CODES
        .binary_search_by(|(glyph, _)| (*glyph).cmp(name))
        .ok()?;
    Some(u32::from(CODES[index].1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_os_codes_include_table_113_additions() {
        assert_eq!(mac_os_code(b"A"), Some(65));
        assert_eq!(mac_os_code(b"Euro"), Some(219));
        assert_eq!(mac_os_code(b"currency"), None);
        assert_eq!(mac_os_code(b"apple"), Some(240));
        assert_eq!(mac_os_code(b"unknown"), None);
    }
}
