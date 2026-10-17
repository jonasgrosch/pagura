//! ISOAdobe glyph names and StandardEncoding (TN 5176 appendices).
use super::predefined;
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Name, Result};
use std::ops::Range;

const ISO_ADOBE: &str = ".notdef space exclam quotedbl numbersign dollar percent ampersand quoteright parenleft parenright asterisk plus comma hyphen period slash zero one two three four five six seven eight nine colon semicolon less equal greater question at A B C D E F G H I J K L M N O P Q R S T U V W X Y Z bracketleft backslash bracketright asciicircum underscore quoteleft a b c d e f g h i j k l m n o p q r s t u v w x y z braceleft bar braceright asciitilde exclamdown cent sterling fraction yen florin section currency quotesingle quotedblleft guillemotleft guilsinglleft guilsinglright fi fl endash dagger daggerdbl periodcentered paragraph bullet quotesinglbase quotedblbase quotedblright guillemotright ellipsis perthousand questiondown grave acute circumflex tilde macron breve dotaccent dieresis ring cedilla hungarumlaut ogonek caron emdash AE ordfeminine Lslash Oslash OE ordmasculine ae dotlessi lslash oslash oe germandbls onesuperior logicalnot mu trademark Eth onehalf plusminus Thorn onequarter divide brokenbar degree thorn threequarters twosuperior registered minus eth multiply threesuperior copyright Aacute Acircumflex Adieresis Agrave Aring Atilde Ccedilla Eacute Ecircumflex Edieresis Egrave Iacute Icircumflex Idieresis Igrave Ntilde Oacute Ocircumflex Odieresis Ograve Otilde Scaron Uacute Ucircumflex Udieresis Ugrave Yacute Ydieresis Zcaron aacute acircumflex adieresis agrave aring atilde ccedilla eacute ecircumflex edieresis egrave iacute icircumflex idieresis igrave ntilde oacute ocircumflex odieresis ograve otilde scaron uacute ucircumflex udieresis ugrave yacute ydieresis zcaron";

pub(crate) fn standard(code: u8) -> Option<Name> {
    let sid = match code {
        32..=126 => u16::from(code) - 31,
        161..=175 => u16::from(code) - 65,
        177..=180 => u16::from(code) - 66,
        182..=189 => u16::from(code) - 67,
        191 => 123,
        193..=200 => u16::from(code) - 69,
        202 => 132,
        203 => 133,
        205..=208 => u16::from(code) - 71,
        225 => 138,
        227 => 139,
        232 => 140,
        233 => 141,
        234 => 142,
        235 => 143,
        241 => 144,
        245 => 145,
        248 => 146,
        249 => 147,
        250 => 148,
        251 => 149,
        _ => return None,
    };
    ISO_ADOBE
        .split_ascii_whitespace()
        .nth(usize::from(sid))
        .map(|s| Name::new(s.as_bytes()))
}

pub(super) fn sid(
    sid: u16,
    strings: &[Range<usize>],
    data: &[u8],
    limits: &Limits,
) -> Result<Name> {
    if sid >= 65000 {
        return Err(bad(0, "reserved CFF SID"));
    }
    if sid < 391 {
        let value = if sid < 229 {
            ISO_ADOBE.split_ascii_whitespace().nth(usize::from(sid))
        } else {
            predefined::EXPERT_STRINGS
                .split_ascii_whitespace()
                .nth(usize::from(sid) - 229)
        }
        .ok_or_else(|| bad(0, "invalid standard CFF SID"))?;
        cap(value.len(), limits.string_bytes, 0)?;
        return Ok(Name::new(value.as_bytes()));
    }
    let range = strings
        .get(usize::from(sid) - 391)
        .ok_or_else(|| bad(0, "CFF SID outside String INDEX"))?;
    cap(range.len(), limits.string_bytes, range.start)?;
    let bytes = &data[range.clone()];
    if bytes.contains(&0) {
        return Err(bad(range.start, "NUL in CFF glyph name"));
    }
    Ok(Name::new(bytes))
}

pub(super) fn encoding(
    table: Read<'_>,
    offset: usize,
    charset: &[u16],
    strings: &[Range<usize>],
    data: &[u8],
    limits: &Limits,
) -> Result<Box<[Option<Name>; 256]>> {
    let mut result = Box::new(std::array::from_fn(|_| None));
    if offset == 0 {
        for code in 0..=255_u8 {
            result[usize::from(code)] = standard(code);
        }
        return Ok(result);
    }
    if offset == 1 {
        for code in 0..=255_u8 {
            if let Some(id) = predefined::expert_sid(code) {
                result[usize::from(code)] = Some(sid(id, strings, data, limits)?);
            }
        }
        return Ok(result);
    }
    let mut r = table.tail(offset)?;
    let format = r.u8()?;
    let count = usize::from(r.u8()?);
    if !matches!(format & 127, 0 | 1) {
        return Err(bad(r.at(), "invalid CFF encoding format"));
    }
    let mut gid = 1;
    for _ in 0..count {
        let first = r.u8()?;
        let left = match format & 127 {
            0 => 0,
            1 => r.u8()?,
            _ => return Err(bad(r.at(), "invalid CFF encoding format")),
        };
        let end = first
            .checked_add(left)
            .ok_or_else(|| bad(r.at(), "CFF encoding code overflow"))?;
        for code in first..=end {
            let sid = *charset
                .get(gid)
                .ok_or_else(|| bad(r.at(), "CFF encoding exceeds charset"))?;
            if result[usize::from(code)].is_some() {
                return Err(bad(r.at(), "duplicate CFF encoding code"));
            }
            result[usize::from(code)] = Some(self::sid(sid, strings, data, limits)?);
            gid += 1;
        }
    }
    if format & 128 != 0 {
        let supplements = r.u8()?;
        for _ in 0..supplements {
            let code = usize::from(r.u8()?);
            let sid = r.u16()?;
            if !charset[1..gid].contains(&sid) || result[code].is_some() {
                return Err(bad(r.at(), "invalid CFF encoding supplement"));
            }
            result[code] = Some(self::sid(sid, strings, data, limits)?);
        }
    }
    Ok(result)
}
