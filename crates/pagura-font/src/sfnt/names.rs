//! OpenType post glyph names. Standard names are defined by the post format.
use crate::budget::Work;
use crate::read::{glyph_id, unsupported};
use crate::{GlyphId, features};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Result};
use std::sync::{LazyLock, OnceLock};

/// post table versions: standard Macintosh order, explicit name indices,
/// signed offsets into the standard order, or no names.
const STANDARD_ORDER: u32 = 0x0001_0000;
const NAME_INDEX: u32 = 0x0002_0000;
const STANDARD_OFFSETS: u32 = 0x0002_5000;
const NO_NAMES: u32 = 0x0003_0000;

/// The standard Macintosh glyph-name order (post versions 1, 2 and 2.5).
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

/// Glyph names by glyph, and an index by name built at the first lookup:
/// selection reads names only when no cmap subtable maps a code (9.6.5.4).
#[derive(Debug, Default)]
pub(super) struct PostNames {
    pub(super) by_glyph: Vec<Option<Name>>,
    /// The named glyphs, ordered by name and then glyph.
    by_name: OnceLock<Vec<u32>>,
}

impl PostNames {
    /// The first glyph named `name`.
    pub(super) fn glyph(&self, name: &[u8]) -> Option<GlyphId> {
        let index = self.by_name.get_or_init(|| self.index());
        let at = index.partition_point(|&glyph| self.name(glyph) < Some(name));
        let glyph = *index.get(at)?;
        (self.name(glyph) == Some(name)).then_some(GlyphId(glyph))
    }

    fn name(&self, glyph: u32) -> Option<&[u8]> {
        self.by_glyph
            .get(u32_to_usize(glyph))?
            .as_ref()
            .map(Name::as_bytes)
    }

    fn index(&self) -> Vec<u32> {
        // Glyph counts fit u32: the parser checked every glyph id.
        let mut index: Vec<u32> = (0..u32::try_from(self.by_glyph.len()).unwrap_or(u32::MAX))
            .filter(|&glyph| self.name(glyph).is_some())
            .collect();
        index.sort_unstable_by(|&a, &b| self.name(a).cmp(&self.name(b)).then(a.cmp(&b)));
        index
    }

    /// Heap bytes, the index by name included whether built or not.
    pub(super) fn heap_bytes(&self) -> usize {
        let named = self.by_glyph.iter().flatten().count();
        crate::names::heap_bytes(&self.by_glyph)
            .saturating_add(named.saturating_mul(size_of::<u32>()))
    }
}

pub(super) fn parse(mut r: Cursor<'_>, count: usize, work: &mut Work<'_>) -> Result<PostNames> {
    let start = r.position();
    let version = r.u32()?;
    r.skip(28)?; // italicAngle .. maxMemType1
    let by_glyph = match version {
        STANDARD_ORDER => {
            if count != STANDARD.len() {
                return Err(r.error("post standard-name count mismatch"));
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
        STANDARD_OFFSETS => {
            if usize::from(r.u16()?) != count {
                return Err(r.error("post glyph count mismatch"));
            }
            crate::names::indexed(
                count,
                (0..count).map(|glyph| {
                    let at = r.position();
                    let name = glyph
                        .checked_add_signed(isize::from(r.i8()?))
                        .and_then(|index| STANDARD_NAMES.get(index))
                        .ok_or_else(|| r.error("invalid post standard-name offset"))?;
                    Ok((glyph_id(glyph, at)?, name))
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
    Ok(PostNames {
        by_glyph,
        by_name: OnceLock::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::PostNames;
    use crate::GlyphId;
    use crate::budget::{FontBudget, Work};
    use pagura_core::bytes::Cursor;
    use pagura_core::{Diagnostics, ErrorKind, Limits, Name, Recovery, Result};

    fn offset_table(offsets: &[i8]) -> Vec<u8> {
        let mut table = super::STANDARD_OFFSETS.to_be_bytes().to_vec();
        table.extend([0; 28]);
        table.extend(u16::try_from(offsets.len()).unwrap().to_be_bytes());
        table.extend(offsets.iter().map(|n| n.to_be_bytes()[0]));
        table
    }

    fn parse_table(table: &[u8], count: usize, limits: &Limits) -> Result<PostNames> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(limits), &mut diagnostics);
        super::parse(Cursor::new(table), count, &mut work)
    }

    #[test]
    fn standard_order_requires_all_258_glyphs() {
        let mut table = super::STANDARD_ORDER.to_be_bytes().to_vec();
        table.extend([0; 28]);
        let names = parse_table(&table, 258, &Limits::default()).unwrap();
        assert_eq!(names.name(0), Some(b".notdef".as_slice()));
        assert_eq!(names.name(257), Some(b"dcroat".as_slice()));
        for count in [0, 1, 5, 257, 259] {
            assert_eq!(
                parse_table(&table, count, &Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
    }

    #[test]
    fn legacy_offsets_are_signed_and_keep_the_standard_name_identity() {
        let table = offset_table(&[0, 35, -1, 34, -1]);
        let names = parse_table(&table, 5, &Limits::default()).unwrap();
        for (glyph, expected) in [".notdef", "A", ".null", "B", "space"].iter().enumerate() {
            assert_eq!(
                names.by_glyph[glyph].as_ref().unwrap().as_bytes(),
                expected.as_bytes()
            );
        }
        assert_eq!(names.glyph(b"A"), Some(GlyphId(1)));
        let mut offsets = vec![0; 131];
        offsets[128] = -128;
        offsets[130] = 127;
        let names = parse_table(&offset_table(&offsets), 131, &Limits::default()).unwrap();
        assert_eq!(names.name(128), Some(b".notdef".as_slice()));
        assert_eq!(names.name(130), Some(b"dcroat".as_slice()));
        assert_eq!(names.glyph(b".notdef"), Some(GlyphId(0)));
    }

    #[test]
    fn legacy_offsets_reject_truncation_counts_and_both_index_bounds() {
        let table = offset_table(&[0, 35, -1, 34, -1]);
        for end in 0..table.len() {
            assert_eq!(
                parse_table(&table[..end], 5, &Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
        assert_eq!(
            parse_table(&table, 4, &Limits::default()).unwrap_err().kind,
            ErrorKind::Syntax
        );
        let mut high = vec![0; 132];
        high[131] = 127;
        for offsets in [vec![-1], high] {
            assert_eq!(
                parse_table(&offset_table(&offsets), offsets.len(), &Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
    }

    #[test]
    fn legacy_offsets_reuse_the_name_copy_work_and_entry_limits() {
        let table = offset_table(&[0, 35, -1, 34, -1]);
        // Five held names, with 7 + 1 + 5 + 1 + 5 copied name bytes.
        for (font_work, font_entries, valid) in [(19, 5, true), (18, 5, false), (19, 4, false)] {
            let limits = Limits {
                font_work,
                font_entries,
                operations: 0,
                ..Limits::default()
            };
            let result = parse_table(&table, 5, &limits);
            if valid {
                assert_eq!(result.unwrap().glyph(b"A"), Some(GlyphId(1)));
            } else {
                assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
            }
        }
    }

    #[test]
    fn a_repeated_name_selects_its_first_glyph() {
        let names = PostNames {
            by_glyph: ["b", "a", "c", "a"]
                .into_iter()
                .map(|name| Some(Name::new(name.as_bytes())))
                .chain([None])
                .collect(),
            ..PostNames::default()
        };
        assert_eq!(names.glyph(b"a"), Some(GlyphId(1)));
        assert_eq!(names.glyph(b"b"), Some(GlyphId(0)));
        assert_eq!(names.glyph(b"c"), Some(GlyphId(2)));
        assert_eq!(names.glyph(b"d"), None);
        assert_eq!(names.glyph(b""), None);
    }
}
