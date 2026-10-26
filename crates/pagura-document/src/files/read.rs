use super::{FileNames, FileSpecification, Relationship, stream};
use crate::view::{Context, Source};
use pagura_core::{Dictionary, Error, Name, Result, Value};

pub(super) fn entry<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<(&'a Value, Source)>> {
    let Some(value) = dictionary.get(key) else {
        return Ok(None);
    };
    let (value, source) = context.resolve(value, source)?;
    Ok((!value.is_null()).then_some((value, source)))
}

pub(super) fn bytes<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<&'a [u8]>> {
    entry(context, dictionary, key, source)?
        .map(|(value, source)| {
            value
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("file entry must be a string")))
        })
        .transpose()
}

pub(super) fn text<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<String>> {
    entry(context, dictionary, key, source)?
        .map(|(value, source)| {
            let bytes = value
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("file entry must be a text string")))?;
            context.text(bytes, source)
        })
        .transpose()
}

pub(super) fn name<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<&'a Name>> {
    entry(context, dictionary, key, source)?
        .map(|(value, source)| {
            value
                .as_name()
                .ok_or_else(|| source.error(Error::syntax("file entry must be a name")))
        })
        .transpose()
}

pub(super) fn specification<'a>(
    context: &mut Context<'a>,
    original: &'a Value,
    source: Source,
) -> Result<FileSpecification<'a>> {
    let indirect = matches!(original, Value::Reference(_));
    let (value, source) = context.resolve(original, source)?;
    context.allocate::<FileSpecification<'_>>(1, source)?;
    let mut result = FileSpecification {
        object: source.object.filter(|_| indirect),
        source: value,
        names: FileNames::default(),
        file_system: None,
        description: None,
        volatile: false,
        identifiers: None,
        relationship: Relationship::Unspecified,
        collection_item: Vec::new(),
        encrypted_payload: None,
        embedded: Vec::new(),
        related: Vec::new(),
    };
    if let Some(bytes) = value.as_string() {
        result.names.file = Some(bytes);
        return Ok(result);
    }
    if let Some(id) = result.object
        && context.document.object(id)?.is_stream()
    {
        return Err(source.error(Error::syntax("file specification must not be a stream")));
    }
    let dictionary = value.as_dictionary().ok_or_else(|| {
        source.error(Error::syntax(
            "file specification requires string or dictionary",
        ))
    })?;
    context.type_name(dictionary, b"Filespec", source)?;
    result.names = FileNames {
        file: bytes(context, dictionary, b"F", source)?,
        unicode: text(context, dictionary, b"UF", source)?,
        dos: bytes(context, dictionary, b"DOS", source)?,
        mac: bytes(context, dictionary, b"Mac", source)?,
        unix: bytes(context, dictionary, b"Unix", source)?,
    };
    if result.names.file.is_none()
        && result.names.dos.is_none()
        && result.names.mac.is_none()
        && result.names.unix.is_none()
    {
        return Err(source.error(Error::syntax(
            "file specification requires F or a legacy path",
        )));
    }
    result.file_system = name(context, dictionary, b"FS", source)?;
    result.description = text(context, dictionary, b"Desc", source)?;
    result.volatile = match entry(context, dictionary, b"V", source)? {
        None => false,
        Some((Value::Boolean(flag), _)) => *flag,
        Some((_, source)) => {
            return Err(source.error(Error::syntax("file volatility must be boolean")));
        }
    };
    result.identifiers = identifiers(context, dictionary, source)?;
    result.collection_item = super::collection::items(context, dictionary, source)?;
    result.encrypted_payload = super::collection::encrypted(context, dictionary, source)?;
    result.relationship = relationship(name(context, dictionary, b"AFRelationship", source)?);
    let embedded = context.present(dictionary, b"EF", source)?;
    let related = context.present(dictionary, b"RF", source)?;
    if (embedded.is_some()
        || related.is_some()
        || context.present(dictionary, b"EP", source)?.is_some())
        && name(context, dictionary, b"Type", source)?.is_none()
    {
        return Err(source.error(Error::syntax(
            "embedded file specification requires Type Filespec",
        )));
    }
    if (embedded.is_some() || related.is_some()) && !indirect {
        return Err(source.error(Error::syntax(
            "embedded file specification must be indirect",
        )));
    }
    if let Some(embedded) = embedded {
        let (embedded, location) = context.dictionary(embedded, source)?;
        result.embedded = stream::entries(context, dictionary, embedded, location)?;
        if let Some(related) = related {
            let (related, location) = context.dictionary(related, source)?;
            result.related = stream::related(context, embedded, related, location)?;
        }
    } else if related.is_some() {
        return Err(source.error(Error::syntax("related files require EF")));
    }
    Ok(result)
}

fn identifiers<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<Option<[&'a [u8]; 2]>> {
    let Some((value, source)) = entry(context, dictionary, b"ID", source)? else {
        return Ok(None);
    };
    let Some([first, second]) = value.as_array() else {
        return Err(source.error(Error::syntax("file ID requires two strings")));
    };
    let (first, first_source) = context.resolve(first, source)?;
    let (second, second_source) = context.resolve(second, source)?;
    Ok(Some([
        first
            .as_string()
            .ok_or_else(|| first_source.error(Error::syntax("file ID requires strings")))?,
        second
            .as_string()
            .ok_or_else(|| second_source.error(Error::syntax("file ID requires strings")))?,
    ]))
}

fn relationship(name: Option<&Name>) -> Relationship<'_> {
    match name.map(Name::as_bytes) {
        None | Some(b"Unspecified") => Relationship::Unspecified,
        Some(b"Source") => Relationship::Source,
        Some(b"Data") => Relationship::Data,
        Some(b"Alternative") => Relationship::Alternative,
        Some(b"Supplement") => Relationship::Supplement,
        Some(b"EncryptedPayload") => Relationship::EncryptedPayload,
        Some(b"FormData") => Relationship::FormData,
        Some(b"Schema") => Relationship::Schema,
        Some(_) => name.map_or(Relationship::Unspecified, Relationship::Other),
    }
}
