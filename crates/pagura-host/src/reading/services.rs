//! Inert document services (7.11, 14.3), encoded within the reading budget.
//! Views use the document's limits; payload decoding also uses its stream
//! limits. No source-controlled name becomes a path, URL or executable content.

use super::{MAX_ATTACHMENTS, Response};
use crate::protocol::{AttachmentKey, Field, ResponseKind};
use crate::wire::Writer;
use pagura::files::{FileSpecification, Relationship};
use pagura::metadata::Trapped;
use pagura::{Document, Error, Name, ObjectId, Result};

pub(crate) fn metadata(document: &Document, cap: usize) -> Result<Response> {
    let mut out = Writer::new(cap);
    let info = document.information()?;
    out.flag(info.is_some())?;
    if let Some(info) = info {
        object(&mut out, info.object)?;
        for value in [
            info.title,
            info.author,
            info.subject,
            info.keywords,
            info.creator,
            info.producer,
            info.creation_date,
            info.modification_date,
        ] {
            text(&mut out, value.as_deref())?;
        }
        out.text(match info.trapped {
            Trapped::True => "True",
            Trapped::False => "False",
            Trapped::Unknown => "Unknown",
        })?;
    }
    text(&mut out, document.language()?.as_deref())?;
    object(&mut out, document.metadata()?.map(|packet| packet.object()))?;
    Ok(Response::new(ResponseKind::Metadata, out))
}

pub(crate) fn metadata_bytes(document: &Document, cap: usize) -> Result<Response> {
    let mut out = Writer::new(cap);
    let packet = document.metadata()?;
    out.flag(packet.is_some())?;
    if let Some(packet) = packet {
        blob(&mut out, &packet.data()?)?;
    }
    Ok(Response::new(ResponseKind::MetadataBytes, out))
}

pub(crate) fn attachments(document: &Document, cap: usize) -> Result<Response> {
    let files = document.embedded_files()?;
    if files.len() > MAX_ATTACHMENTS {
        return Err(Error::limit("catalog attachment listing budget exceeded"));
    }
    let mut out = Writer::new(cap);
    out.count(files.len())?;
    for file in files {
        blob(&mut out, file.key)?;
        specification(&mut out, &file.specification)?;
    }
    Ok(Response::new(ResponseKind::Attachments, out))
}

fn specification(out: &mut Writer, file: &FileSpecification<'_>) -> Result<()> {
    object(out, file.object)?;
    text(out, file.names.unicode.as_deref())?;
    for name in [
        file.names.file,
        file.names.dos,
        file.names.mac,
        file.names.unix,
    ] {
        bytes(out, name)?;
    }
    text(out, file.description.as_deref())?;
    bytes(out, file.file_system.map(Name::as_bytes))?;
    bytes(out, relationship_name(file.relationship))?;
    out.flag(file.volatile)?;
    out.count(file.embedded.len())?;
    for entry in &file.embedded {
        let key = AttachmentKey::ALL
            .iter()
            .find(|key| key.pdf_name() == entry.key.as_bytes())
            .ok_or_else(|| Error::syntax("unknown embedded filename key"))?;
        out.u8(key.code())?;
        out.object(entry.file.object())?;
        bytes(out, entry.file.media_type().map(Name::as_bytes))?;
        let size = entry
            .file
            .parameters()
            .and_then(|parameters| parameters.size);
        out.flag(size.is_some())?;
        if let Some(size) = size {
            out.u64(size)?;
        }
    }
    Ok(())
}

pub(crate) fn attachment_bytes(
    document: &Document,
    index: Field<u32>,
    key: Field<AttachmentKey>,
    cap: usize,
) -> Result<Response> {
    let files = document.embedded_files()?;
    let file = files.get(index.index()?).ok_or_else(|| {
        Error::invalid_argument("attachment index outside catalog name tree").at(index.position())
    })?;
    let entry = file
        .specification
        .embedded
        .iter()
        .find(|entry| entry.key.as_bytes() == key.get().pdf_name())
        .ok_or_else(|| {
            Error::invalid_argument("attachment has no payload for this filename key")
                .at(key.position())
        })?;
    let mut out = Writer::new(cap);
    blob(&mut out, &entry.file.data()?)?;
    Ok(Response::new(ResponseKind::AttachmentBytes, out))
}

fn relationship_name(relationship: Relationship<'_>) -> Option<&[u8]> {
    Some(match relationship {
        Relationship::Source => b"Source",
        Relationship::Data => b"Data",
        Relationship::Alternative => b"Alternative",
        Relationship::Supplement => b"Supplement",
        Relationship::EncryptedPayload => b"EncryptedPayload",
        Relationship::FormData => b"FormData",
        Relationship::Schema => b"Schema",
        Relationship::Unspecified => b"Unspecified",
        Relationship::Other(name) => name.as_bytes(),
        _ => return None,
    })
}

fn blob(out: &mut Writer, value: &[u8]) -> Result<()> {
    out.count(value.len())?;
    out.push(value)
}

fn bytes(out: &mut Writer, value: Option<&[u8]>) -> Result<()> {
    out.flag(value.is_some())?;
    if let Some(value) = value {
        blob(out, value)?;
    }
    Ok(())
}

fn text(out: &mut Writer, value: Option<&str>) -> Result<()> {
    bytes(out, value.map(str::as_bytes))
}

fn object(out: &mut Writer, value: Option<ObjectId>) -> Result<()> {
    out.flag(value.is_some())?;
    if let Some(value) = value {
        out.object(value)?;
    }
    Ok(())
}
