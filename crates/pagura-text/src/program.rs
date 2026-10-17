//! PDF embedding bindings, ISO 32000-2 9.9.1 Tables 124–125.
use crate::{FontKind, features, objects::optional};
use pagura_core::{Dictionary, Error, ObjectId, Position, Result, Value};
use pagura_document::Document;
use pagura_font::{FontFormat, FontProgram};

pub(crate) fn load(
    document: &Document,
    descriptor: &Dictionary,
    kind: FontKind,
    descendant: &Dictionary,
) -> Result<FontProgram> {
    for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
        if optional(document, descriptor, key)?.is_none() {
            continue;
        }
        let id = descriptor
            .get(key)
            .and_then(Value::as_reference)
            .ok_or_else(|| Error::syntax("embedded font must be a stream reference"))?;
        return load_stream(document, id, key, kind, descendant).map_err(|e| e.in_object(id));
    }
    Err(Error::unsupported(
        features::NON_EMBEDDED,
        "font program is not embedded",
    ))
}

fn load_stream(
    document: &Document,
    id: ObjectId,
    key: &[u8],
    kind: FontKind,
    descendant: &Dictionary,
) -> Result<FontProgram> {
    let object = document.object(id)?;
    let dictionary = object
        .dictionary()
        .ok_or_else(|| Error::syntax("embedded font is not a stream"))?;
    let format = format(document, dictionary, key, kind, descendant)?;
    let data = document.stream_data(object)?;
    if key == b"FontFile2" {
        if length(document, dictionary, b"Length1")? != data.len() {
            return Err(Error::syntax(
                "TrueType Length1 disagrees with decoded size",
            ));
        }
    } else if key == b"FontFile" {
        let mut total = 0_usize;
        for key in [b"Length1".as_slice(), b"Length2", b"Length3"] {
            total = total
                .checked_add(length(document, dictionary, key)?)
                .ok_or_else(|| Error::syntax("Type1 segment length overflow"))?;
        }
        if total != data.len() {
            return Err(Error::syntax(
                "Type1 segment lengths disagree with decoded size",
            ));
        }
    } else if format == FontFormat::Cff
        && [b"Length1".as_slice(), b"Length2", b"Length3"]
            .iter()
            .any(|key| dictionary.contains_key(key))
    {
        return Err(Error::syntax("CFF stream must omit Type1 segment lengths"));
    }
    FontProgram::parse(data.into_owned(), format, &document.limits()).map_err(|mut error| {
        if let Position::Buffer(offset) = error.position {
            error.position = Position::Stream { object: id, offset };
        }
        error
    })
}

fn length(document: &Document, dict: &Dictionary, key: &[u8]) -> Result<usize> {
    optional(document, dict, key)?
        .and_then(Value::as_integer)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| Error::syntax("invalid or missing font stream length"))
}

fn format(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    kind: FontKind,
    descendant: &Dictionary,
) -> Result<FontFormat> {
    let type1 = matches!(kind, FontKind::Type1 | FontKind::MmType1);
    let cid0 = descendant.is_name(b"Subtype", b"CIDFontType0");
    let cid2 = descendant.is_name(b"Subtype", b"CIDFontType2");
    let result = match key {
        b"FontFile" if type1 => Some(FontFormat::Type1),
        b"FontFile2" if kind == FontKind::TrueType || cid2 => Some(FontFormat::TrueType),
        b"FontFile3" => match optional(document, dictionary, b"Subtype")?
            .and_then(Value::as_name)
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"Type1C") if type1 => Some(FontFormat::Cff),
            Some(b"CIDFontType0C") if cid0 => Some(FontFormat::Cff),
            Some(b"OpenType") if type1 || kind == FontKind::TrueType || cid0 || cid2 => {
                Some(FontFormat::OpenType)
            }
            _ => None,
        },
        _ => None,
    };
    result.ok_or_else(|| Error::syntax("font program binding disagrees with PDF font subtype"))
}
