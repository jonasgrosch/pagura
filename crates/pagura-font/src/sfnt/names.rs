//! OpenType post glyph names. Standard names are defined by the post format.
use crate::GlyphId;
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Name, Result};
use std::collections::BTreeMap;

// The standard Macintosh glyph-name order (post versions 1 and 2).
const STANDARD: &str = ".notdef .null nonmarkingreturn space exclam quotedbl numbersign dollar percent ampersand quotesingle parenleft parenright asterisk plus comma hyphen period slash zero one two three four five six seven eight nine colon semicolon less equal greater question at A B C D E F G H I J K L M N O P Q R S T U V W X Y Z bracketleft backslash bracketright asciicircum underscore grave a b c d e f g h i j k l m n o p q r s t u v w x y z braceleft bar braceright asciitilde Adieresis Aring Ccedilla Eacute Ntilde Odieresis Udieresis aacute agrave acircumflex adieresis atilde aring ccedilla eacute egrave ecircumflex edieresis iacute igrave icircumflex idieresis ntilde oacute ograve ocircumflex odieresis otilde uacute ugrave ucircumflex udieresis dagger degree cent sterling section bullet paragraph germandbls registered copyright trademark acute dieresis notequal AE Oslash infinity plusminus lessequal greaterequal yen mu partialdiff summation product pi integral ordfeminine ordmasculine Omega ae oslash questiondown exclamdown logicalnot radical florin approxequal Delta guillemotleft guillemotright ellipsis nonbreakingspace Agrave Atilde Otilde OE oe endash emdash quotedblleft quotedblright quoteleft quoteright divide lozenge ydieresis Ydieresis fraction currency guilsinglleft guilsinglright fi fl daggerdbl periodcentered quotesinglbase quotedblbase perthousand Acircumflex Ecircumflex Aacute Edieresis Egrave Iacute Icircumflex Idieresis Igrave Oacute Ocircumflex apple Ograve Uacute Ucircumflex Ugrave dotlessi circumflex tilde macron breve dotaccent ring cedilla hungarumlaut ogonek caron Lslash lslash Scaron scaron Zcaron zcaron brokenbar Eth eth Yacute yacute Thorn thorn minus multiply onesuperior twosuperior threesuperior onehalf onequarter threequarters franc Gbreve gbreve Idotaccent Scedilla scedilla Cacute cacute Ccaron ccaron dcroat";

pub(super) fn parse(
    mut r: Read<'_>,
    count: usize,
    limits: &Limits,
) -> Result<BTreeMap<Name, GlyphId>> {
    let version = r.u32()?;
    r.skip(28)?;
    let standard: Vec<_> = STANDARD
        .split_ascii_whitespace()
        .map(|s| Name::new(s.as_bytes()))
        .collect();
    let mut names = BTreeMap::new();
    match version {
        0x0001_0000 => {
            if count > standard.len() {
                return Err(bad(r.at(), "post standard-name count exceeded"));
            }
            for (i, name) in standard.into_iter().take(count).enumerate() {
                names.insert(
                    name,
                    GlyphId(
                        u32::try_from(i)
                            .map_err(|_| crate::read::limit(r.at(), "glyph index overflow"))?,
                    ),
                );
            }
        }
        0x0002_0000 => {
            if usize::from(r.u16()?) != count {
                return Err(bad(r.at(), "post glyph count mismatch"));
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
            cap(custom_count, limits.value_nodes, r.at())?;
            let mut custom = Vec::with_capacity(custom_count);
            let mut bytes = 0;
            for _ in 0..custom_count {
                let len = usize::from(r.u8()?);
                bytes += len;
                cap(bytes, limits.string_bytes, r.at())?;
                let name = r.take(len)?;
                if name.contains(&0) {
                    return Err(bad(r.at(), "NUL in post glyph name"));
                }
                custom.push(Name::new(name));
            }
            for (i, index) in indices.into_iter().enumerate() {
                let name = if index < 258 {
                    standard.get(index)
                } else {
                    custom.get(index - 258)
                }
                .ok_or_else(|| bad(r.at(), "invalid post glyph name"))?;
                names.entry(name.clone()).or_insert(GlyphId(
                    u32::try_from(i)
                        .map_err(|_| crate::read::limit(r.at(), "glyph index overflow"))?,
                ));
            }
        }
        0x0003_0000 => {}
        _ => {
            return Err(pagura_core::Error::unsupported(
                crate::features::FORMAT,
                "post name format is not implemented",
            )
            .at(pagura_core::Position::Buffer(r.base)));
        }
    }
    Ok(names)
}
