//! OpenType post glyph names. Standard names are defined by the post format.
use crate::budget::Work;
use crate::read::{glyph_id, unsupported};
use crate::{GlyphId, features};
use pagura_core::bytes::Cursor;
use pagura_core::{Name, Position, Result};
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// post table versions: standard Macintosh order, an explicit name index,
/// or no names.
const STANDARD_ORDER: u32 = 0x0001_0000;
const NAME_INDEX: u32 = 0x0002_0000;
const NO_NAMES: u32 = 0x0003_0000;

/// The standard Macintosh glyph-name order (post versions 1 and 2).
const STANDARD: [&str; 258] = [
    ".notdef",
    ".null",
    "nonmarkingreturn",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quotesingle",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "grave",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "Adieresis",
    "Aring",
    "Ccedilla",
    "Eacute",
    "Ntilde",
    "Odieresis",
    "Udieresis",
    "aacute",
    "agrave",
    "acircumflex",
    "adieresis",
    "atilde",
    "aring",
    "ccedilla",
    "eacute",
    "egrave",
    "ecircumflex",
    "edieresis",
    "iacute",
    "igrave",
    "icircumflex",
    "idieresis",
    "ntilde",
    "oacute",
    "ograve",
    "ocircumflex",
    "odieresis",
    "otilde",
    "uacute",
    "ugrave",
    "ucircumflex",
    "udieresis",
    "dagger",
    "degree",
    "cent",
    "sterling",
    "section",
    "bullet",
    "paragraph",
    "germandbls",
    "registered",
    "copyright",
    "trademark",
    "acute",
    "dieresis",
    "notequal",
    "AE",
    "Oslash",
    "infinity",
    "plusminus",
    "lessequal",
    "greaterequal",
    "yen",
    "mu",
    "partialdiff",
    "summation",
    "product",
    "pi",
    "integral",
    "ordfeminine",
    "ordmasculine",
    "Omega",
    "ae",
    "oslash",
    "questiondown",
    "exclamdown",
    "logicalnot",
    "radical",
    "florin",
    "approxequal",
    "Delta",
    "guillemotleft",
    "guillemotright",
    "ellipsis",
    "nonbreakingspace",
    "Agrave",
    "Atilde",
    "Otilde",
    "OE",
    "oe",
    "endash",
    "emdash",
    "quotedblleft",
    "quotedblright",
    "quoteleft",
    "quoteright",
    "divide",
    "lozenge",
    "ydieresis",
    "Ydieresis",
    "fraction",
    "currency",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "daggerdbl",
    "periodcentered",
    "quotesinglbase",
    "quotedblbase",
    "perthousand",
    "Acircumflex",
    "Ecircumflex",
    "Aacute",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Oacute",
    "Ocircumflex",
    "apple",
    "Ograve",
    "Uacute",
    "Ucircumflex",
    "Ugrave",
    "dotlessi",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "Lslash",
    "lslash",
    "Scaron",
    "scaron",
    "Zcaron",
    "zcaron",
    "brokenbar",
    "Eth",
    "eth",
    "Yacute",
    "yacute",
    "Thorn",
    "thorn",
    "minus",
    "multiply",
    "onesuperior",
    "twosuperior",
    "threesuperior",
    "onehalf",
    "onequarter",
    "threequarters",
    "franc",
    "Gbreve",
    "gbreve",
    "Idotaccent",
    "Scedilla",
    "scedilla",
    "Cacute",
    "cacute",
    "Ccaron",
    "ccaron",
    "dcroat",
];

/// The standard names as `Name`s, built once rather than per font.
static STANDARD_NAMES: LazyLock<Vec<Name>> = LazyLock::new(|| {
    STANDARD
        .iter()
        .map(|name| Name::new(name.as_bytes()))
        .collect()
});

/// Glyph names by name (the first glyph for a repeated name) and by glyph.
#[derive(Debug, Default)]
pub(super) struct PostNames {
    pub(super) by_name: BTreeMap<Name, GlyphId>,
    pub(super) by_glyph: Vec<Option<Name>>,
}

pub(super) fn parse(mut r: Cursor<'_>, count: usize, work: &mut Work<'_>) -> Result<PostNames> {
    let start = r.position();
    let version = r.u32()?;
    r.skip(28)?; // italicAngle .. maxMemType1
    let by_glyph = match version {
        STANDARD_ORDER => {
            if count > STANDARD.len() {
                return Err(r.error("post standard-name count exceeded"));
            }
            let at = r.position();
            crate::names::indexed(
                count,
                STANDARD_NAMES
                    .iter()
                    .take(count)
                    .enumerate()
                    .map(|(i, name)| Ok((glyph_id(i, at)?, name))),
                work,
                start,
            )?
        }
        NAME_INDEX => {
            if usize::from(r.u16()?) != count {
                return Err(r.error("post glyph count mismatch"));
            }
            let mut indices = Vec::with_capacity(count);
            let mut custom_count = 0;
            for _ in 0..count {
                let index = usize::from(r.u16()?);
                if let Some(custom) = index.checked_sub(STANDARD.len()) {
                    custom_count = custom_count.max(custom + 1);
                }
                indices.push(index);
            }
            work.entries(custom_count, r.position())?;
            let mut custom = Vec::with_capacity(custom_count);
            let mut bytes = 0;
            for _ in 0..custom_count {
                let len = usize::from(r.u8()?);
                bytes += len;
                work.string(bytes, r.position())?;
                let at = r.position();
                custom.push(Name::try_new(r.take(len)?).map_err(|e| e.at(at))?);
            }
            let at = r.position();
            crate::names::indexed(
                count,
                indices.into_iter().enumerate().map(|(i, index)| {
                    let name = if let Some(custom_index) = index.checked_sub(STANDARD.len()) {
                        custom.get(custom_index)
                    } else {
                        STANDARD_NAMES.get(index)
                    };
                    let name = name.ok_or_else(|| r.error("invalid post glyph name"))?;
                    Ok((glyph_id(i, at)?, name))
                }),
                work,
                start,
            )?
        }
        NO_NAMES => Vec::new(),
        _ => {
            return Err(unsupported(
                features::FORMAT,
                start,
                "post name format is not implemented",
            ));
        }
    };
    let mut by_name = BTreeMap::new();
    for (i, name) in by_glyph.iter().enumerate() {
        if let Some(name) = name {
            by_name
                .entry(name.clone())
                .or_insert(glyph_id(i, Position::Unknown)?);
        }
    }
    Ok(PostNames { by_name, by_glyph })
}
