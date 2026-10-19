//! OpenType post glyph names. Standard names are defined by the post format.
use crate::GlyphId;
use crate::budget::Work;
use crate::read::{bad, here};
use pagura_core::bytes::Cursor;
use pagura_core::{Name, Position, Result};
use std::collections::BTreeMap;

// The standard Macintosh glyph-name order (post versions 1 and 2).
const STANDARD: &str = ".notdef .null nonmarkingreturn space exclam quotedbl numbersign dollar percent ampersand quotesingle parenleft parenright asterisk plus comma hyphen period slash zero one two three four five six seven eight nine colon semicolon less equal greater question at A B C D E F G H I J K L M N O P Q R S T U V W X Y Z bracketleft backslash bracketright asciicircum underscore grave a b c d e f g h i j k l m n o p q r s t u v w x y z braceleft bar braceright asciitilde Adieresis Aring Ccedilla Eacute Ntilde Odieresis Udieresis aacute agrave acircumflex adieresis atilde aring ccedilla eacute egrave ecircumflex edieresis iacute igrave icircumflex idieresis ntilde oacute ograve ocircumflex odieresis otilde uacute ugrave ucircumflex udieresis dagger degree cent sterling section bullet paragraph germandbls registered copyright trademark acute dieresis notequal AE Oslash infinity plusminus lessequal greaterequal yen mu partialdiff summation product pi integral ordfeminine ordmasculine Omega ae oslash questiondown exclamdown logicalnot radical florin approxequal Delta guillemotleft guillemotright ellipsis nonbreakingspace Agrave Atilde Otilde OE oe endash emdash quotedblleft quotedblright quoteleft quoteright divide lozenge ydieresis Ydieresis fraction currency guilsinglleft guilsinglright fi fl daggerdbl periodcentered quotesinglbase quotedblbase perthousand Acircumflex Ecircumflex Aacute Edieresis Egrave Iacute Icircumflex Idieresis Igrave Oacute Ocircumflex apple Ograve Uacute Ucircumflex Ugrave dotlessi circumflex tilde macron breve dotaccent ring cedilla hungarumlaut ogonek caron Lslash lslash Scaron scaron Zcaron zcaron brokenbar Eth eth Yacute yacute Thorn thorn minus multiply onesuperior twosuperior threesuperior onehalf onequarter threequarters franc Gbreve gbreve Idotaccent Scedilla scedilla Cacute cacute Ccaron ccaron dcroat";

#[derive(Debug, Default)]
pub(super) struct PostNames {
    pub by_name: BTreeMap<Name, GlyphId>,
    pub by_glyph: Vec<Option<Name>>,
}

pub(super) fn parse(mut r: Cursor<'_>, count: usize, work: &mut Work<'_>) -> Result<PostNames> {
    let start = here(&r);
    let version = r.u32()?;
    r.skip(28)?;
    let standard: Vec<_> = STANDARD
        .split_ascii_whitespace()
        .map(|s| Name::new(s.as_bytes()))
        .collect();
    let by_glyph = match version {
        0x0001_0000 => {
            if count > standard.len() {
                return Err(bad(here(&r), "post standard-name count exceeded"));
            }
            crate::names::indexed(
                count,
                standard
                    .iter()
                    .take(count)
                    .enumerate()
                    .map(|(i, name)| Ok((glyph_id(i, here(&r))?, name))),
                work,
                start,
            )?
        }
        0x0002_0000 => {
            if usize::from(r.u16()?) != count {
                return Err(bad(here(&r), "post glyph count mismatch"));
            }
            let mut indices = Vec::with_capacity(count);
            let mut custom_count = 0;
            for _ in 0..count {
                let index = usize::from(r.u16()?);
                if index >= 258 {
                    custom_count = custom_count.max(index - 257);
                }
                indices.push(index);
            }
            work.entries(custom_count, Position::Buffer(here(&r)))?;
            let mut custom = Vec::with_capacity(custom_count);
            let mut bytes = 0;
            for _ in 0..custom_count {
                let len = usize::from(r.u8()?);
                bytes += len;
                work.string(bytes, Position::Buffer(here(&r)))?;
                let name = r.take(len)?;
                if name.contains(&0) {
                    return Err(bad(here(&r), "NUL in post glyph name"));
                }
                custom.push(Name::new(name));
            }
            crate::names::indexed(
                count,
                indices.into_iter().enumerate().map(|(i, index)| {
                    let name = if index < 258 {
                        standard.get(index)
                    } else {
                        custom.get(index - 258)
                    }
                    .ok_or_else(|| bad(here(&r), "invalid post glyph name"))?;
                    Ok((glyph_id(i, here(&r))?, name))
                }),
                work,
                start,
            )?
        }
        0x0003_0000 => Vec::new(),
        _ => {
            return Err(pagura_core::Error::unsupported(
                crate::features::FORMAT,
                "post name format is not implemented",
            )
            .at(Position::Buffer(start)));
        }
    };
    let mut by_name = BTreeMap::new();
    for (i, name) in by_glyph.iter().enumerate() {
        if let Some(name) = name {
            by_name
                .entry(name.clone())
                .or_insert(glyph_id(i, here(&r))?);
        }
    }
    Ok(PostNames { by_name, by_glyph })
}

fn glyph_id(i: usize, at: usize) -> Result<GlyphId> {
    u32::try_from(i)
        .map(GlyphId)
        .map_err(|_| crate::read::limit(at, "glyph index overflow"))
}
