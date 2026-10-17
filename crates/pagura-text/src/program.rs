//! PDF embedding bindings, ISO 32000-2 9.9.1 Tables 124–125.
use crate::{FontKind, features, objects::optional};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, ObjectId, Position, Repair, Result, Value,
};
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
    if let Some(problem) = length_problem(document, dictionary, key, format, data.len())? {
        // The decoded program describes its own extent; the Length entries
        // are bookkeeping that writers often get wrong (Table 125).
        let mut diagnostics = Diagnostics::new(document.recovery());
        diagnostics.repair(Diagnostic::new(STREAM_LENGTHS, problem).in_object(id))?;
        document.report(diagnostics.take());
    }
    FontProgram::parse(data.into_owned(), format, &document.limits()).map_err(|mut error| {
        if let Position::Buffer(offset) = error.position {
            error.position = Position::Stream { object: id, offset };
        }
        error
    })
}

/// A font stream whose `Length1` to `Length3` are absent, invalid or disagree
/// with its decoded program (Table 125). The program is parsed as decoded.
pub const STREAM_LENGTHS: Repair = Repair::new("font.stream-lengths");

/// Why the stream's Length entries disagree with its decoded program, if they
/// do: TrueType needs `Length1` equal to the program, Type 1 needs the three
/// segment lengths to add up to it, and CFF has none.
fn length_problem(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    format: FontFormat,
    decoded: usize,
) -> Result<Option<&'static str>> {
    if key == b"FontFile2" {
        return Ok(match length(document, dictionary, b"Length1")? {
            None => Some("TrueType font stream has no valid Length1"),
            Some(length) if length != decoded => {
                Some("TrueType Length1 disagrees with decoded size")
            }
            Some(_) => None,
        });
    }
    if key == b"FontFile" {
        let mut total = Some(0_usize);
        for key in [b"Length1".as_slice(), b"Length2", b"Length3"] {
            total = match (total, length(document, dictionary, key)?) {
                (Some(sum), Some(length)) => sum.checked_add(length),
                _ => None,
            };
        }
        return Ok(match total {
            None => Some("Type1 font stream lacks valid Length1, Length2 or Length3"),
            Some(total) if total != decoded => {
                Some("Type1 segment lengths disagree with decoded size")
            }
            Some(_) => None,
        });
    }
    let has_lengths = [b"Length1".as_slice(), b"Length2", b"Length3"]
        .iter()
        .any(|key| dictionary.contains_key(key));
    Ok((format == FontFormat::Cff && has_lengths)
        .then_some("CFF font stream has Type1 segment lengths"))
}

fn length(document: &Document, dict: &Dictionary, key: &[u8]) -> Result<Option<usize>> {
    Ok(optional(document, dict, key)?
        .and_then(Value::as_integer)
        .and_then(|v| usize::try_from(v).ok()))
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
