//! CLI access to inert document services through the public facade.

mod annotations;
pub(crate) mod extended;

use crate::{args::Source, failure::Failure, input::write_new, json};
use pagura::files::{EmbeddedFile, FileSpecification, ParameterCheck, Relationship};
use pagura::metadata::{DocumentInfo, Trapped};
use pagura::{Document, Name, ObjectId};
use std::io::{self, Write};
use std::path::Path;

/// Metadata report schema. Fields are additive; Info and XMP are never merged.
const METADATA_SCHEMA: u32 = 1;
/// Catalog attachment listing schema; byte fields are lowercase hexadecimal.
const ATTACHMENTS_SCHEMA: u32 = 1;

pub(crate) fn metadata(source: &Source, out: &mut impl Write) -> Result<(), Failure> {
    let document = source.open()?;
    let info = document.information()?;
    let language = document.language()?;
    let packet = document.metadata()?;
    write!(out, "{{\"schema\":{METADATA_SCHEMA},\"info\":")?;
    match info {
        Some(info) => information(out, &info)?,
        None => out.write_all(b"null")?,
    }
    out.write_all(b",\"language\":")?;
    json::optional(out, language.as_deref())?;
    out.write_all(b",\"metadata_object\":")?;
    object(out, packet.map(|packet| packet.object()))?;
    out.write_all(b",")?;
    json::diagnostics(out, &document)?;
    writeln!(out, "}}")?;
    Ok(())
}

fn information(out: &mut impl Write, info: &DocumentInfo<'_>) -> io::Result<()> {
    out.write_all(b"{\"object\":")?;
    object(out, info.object)?;
    for (key, text) in [
        ("Title", &info.title),
        ("Author", &info.author),
        ("Subject", &info.subject),
        ("Keywords", &info.keywords),
        ("Creator", &info.creator),
        ("Producer", &info.producer),
        ("CreationDate", &info.creation_date),
        ("ModDate", &info.modification_date),
    ] {
        write!(out, ",\"{key}\":")?;
        json::optional(out, text.as_deref())?;
    }
    let trapped = match info.trapped {
        Trapped::True => "True",
        Trapped::False => "False",
        Trapped::Unknown => "Unknown",
    };
    write!(out, ",\"Trapped\":\"{trapped}\"}}")
}

pub(crate) fn attachments(
    source: &Source,
    page: Option<usize>,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let document = source.open()?;
    if let Some(page) = page {
        return annotations::list(&document, page, out);
    }
    let files = document.embedded_files()?;
    write!(out, "{{\"schema\":{ATTACHMENTS_SCHEMA},\"files\":[")?;
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        write!(out, "{{\"index\":{index},\"key_hex\":")?;
        hex(out, Some(file.key))?;
        out.write_all(b",\"object\":")?;
        object(out, file.specification.object)?;
        specification(out, &file.specification)?;
        out.write_all(b"}")?;
    }
    out.write_all(b"],")?;
    json::diagnostics(out, &document)?;
    writeln!(out, "}}")?;
    Ok(())
}

fn specification(out: &mut impl Write, file: &FileSpecification<'_>) -> io::Result<()> {
    out.write_all(b",\"names\":{\"UF\":")?;
    json::optional(out, file.names.unicode.as_deref())?;
    for (key, value) in [
        ("F_hex", file.names.file),
        ("DOS_hex", file.names.dos),
        ("Mac_hex", file.names.mac),
        ("Unix_hex", file.names.unix),
    ] {
        write!(out, ",\"{key}\":")?;
        hex(out, value)?;
    }
    out.write_all(b"},\"description\":")?;
    json::optional(out, file.description.as_deref())?;
    out.write_all(b",\"file_system_hex\":")?;
    hex(out, file.file_system.map(Name::as_bytes))?;
    let relationship = relationship_name(file.relationship);
    out.write_all(b",\"relationship\":")?;
    json::optional(
        out,
        relationship.and_then(|bytes| std::str::from_utf8(bytes).ok()),
    )?;
    out.write_all(b",\"relationship_hex\":")?;
    hex(out, relationship)?;
    write!(out, ",\"volatile\":{},\"payloads\":[", file.volatile)?;
    for (index, entry) in file.embedded.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key\":")?;
        // The file view admits only the five standard filename keys.
        json::optional(out, std::str::from_utf8(entry.key.as_bytes()).ok())?;
        out.write_all(b",\"object\":")?;
        object(out, Some(entry.file.object()))?;
        out.write_all(b",\"media_type_hex\":")?;
        hex(out, entry.file.media_type().map(Name::as_bytes))?;
        out.write_all(b",\"declared_size\":")?;
        match entry
            .file
            .parameters()
            .and_then(|parameters| parameters.size)
        {
            Some(size) => write!(out, "{size}")?,
            None => out.write_all(b"null")?,
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"]")
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

fn object(out: &mut impl Write, id: Option<ObjectId>) -> io::Result<()> {
    match id {
        Some(id) => write!(
            out,
            "{{\"number\":{},\"generation\":{}}}",
            id.number, id.generation
        ),
        None => out.write_all(b"null"),
    }
}

fn hex(out: &mut impl Write, bytes: Option<&[u8]>) -> io::Result<()> {
    match bytes {
        Some(bytes) => {
            out.write_all(b"\"")?;
            for byte in bytes {
                write!(out, "{byte:02x}")?;
            }
            out.write_all(b"\"")
        }
        None => out.write_all(b"null"),
    }
}

pub(crate) fn extract_metadata(source: &Source, output: &Path) -> Result<(), Failure> {
    let document = source.open()?;
    let metadata = document
        .metadata()?
        .ok_or_else(|| Failure::Request("document has no metadata stream".into()))?;
    let bytes = metadata.data()?;
    write_new(output, &bytes)?;
    Ok(())
}

pub(crate) fn extract_attachment(
    source: &Source,
    page: Option<usize>,
    index: usize,
    key: &str,
    output: &Path,
) -> Result<(), Failure> {
    let document = source.open()?;
    let payload = attachment(&document, page, index, key)?;
    let bytes = payload.data()?;
    write_new(output, &bytes)?;
    Ok(())
}

fn attachment<'a>(
    document: &'a Document,
    page: Option<usize>,
    index: usize,
    key: &str,
) -> Result<EmbeddedFile<'a>, Failure> {
    let specification = if let Some(page) = page {
        document
            .page(page)?
            .attachment_annotations(document)?
            .into_iter()
            .find(|annotation| annotation.index == index)
            .map(|annotation| annotation.specification)
            .ok_or_else(|| {
                Failure::Request(
                    "INDEX does not identify a file attachment in this page's Annots".into(),
                )
            })?
    } else {
        document
            .embedded_files()?
            .into_iter()
            .nth(index)
            .map(|file| file.specification)
            .ok_or_else(|| {
                Failure::Request("attachment INDEX is outside the catalog listing".into())
            })?
    };
    specification
        .embedded
        .into_iter()
        .find(|entry| entry.key.as_bytes() == key.as_bytes())
        .map(|entry| entry.file)
        .ok_or_else(|| Failure::Request("attachment has no EF payload with this KEY".into()))
}

pub(crate) fn check_attachment(
    source: &Source,
    page: Option<usize>,
    index: usize,
    key: &str,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let document = source.open()?;
    let payload = attachment(&document, page, index, key)?;
    let checked = payload.check_parameters()?;
    let size = check_name(checked.size)?;
    let checksum = check_name(checked.checksum)?;
    out.write_all(b"{\"schema\":1,")?;
    if let Some(page) = page {
        write!(out, "\"page\":{page},")?;
    }
    write!(out, "\"index\":{index},\"key\":")?;
    json::string(out, key)?;
    out.write_all(b",\"object\":")?;
    object(out, Some(payload.object()))?;
    write!(
        out,
        ",\"decoded_size\":{},\"size\":\"{size}\",\"checksum\":\"{checksum}\",",
        checked.decoded_size
    )?;
    json::diagnostics(out, &document)?;
    writeln!(out, "}}")?;
    if checked.size == ParameterCheck::Mismatch || checked.checksum == ParameterCheck::Mismatch {
        return Err(Failure::Request(
            "attachment declarations do not match decoded payload".into(),
        ));
    }
    Ok(())
}

fn check_name(value: ParameterCheck) -> Result<&'static str, Failure> {
    match value {
        ParameterCheck::NotDeclared => Ok("not-declared"),
        ParameterCheck::Matches => Ok("match"),
        ParameterCheck::Mismatch => Ok("mismatch"),
        _ => Err(Failure::Request(
            "unsupported attachment check result".into(),
        )),
    }
}
