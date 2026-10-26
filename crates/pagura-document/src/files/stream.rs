use super::{EmbeddedFile, EmbeddedFileEntry, FileParameters, RelatedFile, read};
use crate::source::ReadResult;
use crate::view::{Context, Source};
use pagura_core::{Dictionary, Error, Value};

pub(super) fn entries<'a>(
    context: &mut Context<'a>,
    specification: &'a Dictionary,
    embedded: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<EmbeddedFileEntry<'a>>> {
    let mut files = Vec::new();
    for (key, value) in embedded {
        context.step(0, source)?;
        if context.resolve(value, source)?.0.is_null() {
            continue;
        }
        if !matches!(key.as_bytes(), b"F" | b"UF" | b"DOS" | b"Mac" | b"Unix") {
            return Err((source.error(Error::syntax("invalid EF filename key"))).into());
        }
        if read::bytes(context, specification, key.as_bytes(), source)?.is_none() {
            return Err((source.error(Error::syntax("EF entry has no matching filename"))).into());
        }
        context.allocate::<EmbeddedFileEntry<'_>>(1, source)?;
        files.push(EmbeddedFileEntry {
            key,
            file: stream(context, value, source)?,
        });
    }
    Ok(files)
}

pub(super) fn related<'a>(
    context: &mut Context<'a>,
    embedded: &'a Dictionary,
    related: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<RelatedFile<'a>>> {
    let mut files = Vec::new();
    for (key, value) in related {
        context.step(0, source)?;
        let (value, source) = context.resolve(value, source)?;
        if value.is_null() {
            continue;
        }
        if context.present(embedded, key.as_bytes(), source)?.is_none() {
            return Err((source.error(Error::syntax("RF key has no matching EF entry"))).into());
        }
        let pairs = value
            .as_array()
            .ok_or_else(|| source.error(Error::syntax("RF entry requires an array")))?;
        let (pairs, remainder) = pairs.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(
                (source.error(Error::syntax("RF array requires name/stream pairs"))).into(),
            );
        }
        for [name, value] in pairs {
            context.step(0, source)?;
            let (name, location) = context.resolve(name, source)?;
            let name = name.as_string().ok_or_else(|| {
                location.error(Error::syntax("related filename must be a string"))
            })?;
            context.allocate::<RelatedFile<'_>>(1, source)?;
            files.push(RelatedFile {
                key,
                name,
                file: stream(context, value, source)?,
            });
        }
    }
    Ok(files)
}

fn stream<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<EmbeddedFile<'a>> {
    if !matches!(value, Value::Reference(_)) {
        return Err((source.error(Error::syntax("embedded file must reference a stream"))).into());
    }
    let (value, source) = context.resolve(value, source)?;
    let id = source
        .object
        .ok_or_else(|| source.error(Error::syntax("embedded stream has no identity")))?;
    let object = context.document.object(id)?;
    let dictionary = value
        .as_dictionary()
        .filter(|_| object.is_stream())
        .ok_or_else(|| source.error(Error::syntax("embedded file must reference a stream")))?;
    context.type_name(dictionary, b"EmbeddedFile", source)?;
    let media_type = read::name(context, dictionary, b"Subtype", source)?;
    let parameters = context
        .present(dictionary, b"Params", source)?
        .map(|value| parameters(context, value, source))
        .transpose()?;
    context.allocate::<EmbeddedFile<'_>>(1, source)?;
    Ok(EmbeddedFile {
        document: context.document,
        object,
        media_type,
        parameters,
    })
}

fn parameters<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<FileParameters<'a>> {
    let (dictionary, source) = context.dictionary(value, source)?;
    let size = read::entry(context, dictionary, b"Size", source)?
        .map(|(value, source)| {
            value
                .as_integer()
                .and_then(|size| u64::try_from(size).ok())
                .ok_or_else(|| {
                    source.error(Error::syntax(
                        "embedded file size must be a nonnegative integer",
                    ))
                })
        })
        .transpose()?;
    let checksum = read::bytes(context, dictionary, b"CheckSum", source)?
        .map(|bytes| {
            bytes.try_into().map_err(|_| {
                source.error(Error::syntax("embedded file checksum requires 16 bytes"))
            })
        })
        .transpose()?;
    context.allocate::<FileParameters<'_>>(1, source)?;
    Ok(FileParameters {
        dictionary,
        size,
        creation_date: read::text(context, dictionary, b"CreationDate", source)?,
        modification_date: read::text(context, dictionary, b"ModDate", source)?,
        checksum,
    })
}
