//! Explicit scoped files, metadata, collection and source-image transfers.
use super::{hex, object, parameters, specification};
use crate::args::{Source, services::ServiceCommand};
use crate::{failure::Failure, input::write_new, json};
use pagura::files::{CollectionValue, FileSpecification};
use pagura::metadata::MetadataOwner;
use pagura::{Document, Name};
use std::io::Write;

pub(crate) fn run(
    source: &Source,
    request: &ServiceCommand,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let document = source.open()?;
    // Build complete reports in memory before stdout; late malformed declarations
    // cannot masquerade as a complete JSON report.
    let mut report = Vec::new();
    match request {
        ServiceCommand::Files(scope) => {
            let files = document.files(*scope)?;
            report.write_all(b"{\"schema\":2,\"files\":[")?;
            for (ordinal, file) in files.iter().enumerate() {
                if ordinal > 0 {
                    report.write_all(b",")?;
                }
                write!(report, "{{\"index\":{},\"key_hex\":", file.index)?;
                hex(&mut report, file.key)?;
                report.write_all(b",\"object\":")?;
                object(&mut report, file.specification.object)?;
                specification(&mut report, &file.specification)?;
                report.write_all(b"}")?;
            }
            report.write_all(b"]")?;
        }
        ServiceCommand::ExtractFile { selection, output } => {
            let file =
                document.file_payload(selection.scope, selection.index, selection.payload())?;
            write_new(output, &file.data()?)?;
            return Ok(());
        }
        ServiceCommand::CheckFile(selection) => {
            let file =
                document.file_payload(selection.scope, selection.index, selection.payload())?;
            let checked = file.check_parameters()?;
            write!(
                report,
                "{{\"schema\":2,\"decoded_size\":{},\"size\":\"{}\",\"checksum\":\"{}\"",
                checked.decoded_size,
                super::check_name(checked.size)?,
                super::check_name(checked.checksum)?
            )?;
            finish(&document, &mut report, out)?;
            if checked.size == pagura::files::ParameterCheck::Mismatch
                || checked.checksum == pagura::files::ParameterCheck::Mismatch
            {
                return Err(Failure::Request("attachment parameter mismatch".into()));
            }
            return Ok(());
        }
        ServiceCommand::Metadata(owner) => {
            let packet = document.metadata_for(*owner)?;
            report.write_all(b"{\"schema\":2,\"metadata_object\":")?;
            object(&mut report, packet.map(|packet| packet.object()))?;
        }
        ServiceCommand::ExtractMetadata { owner, output } => {
            let packet = document
                .metadata_for(*owner)?
                .ok_or_else(|| Failure::Request("selected owner has no metadata stream".into()))?;
            write_new(output, &packet.data()?)?;
            return Ok(());
        }
        ServiceCommand::Collection => collection(&document, &mut report)?,
        ServiceCommand::Images(owner) => images(&document, *owner, &mut report)?,
        ServiceCommand::ExtractImage {
            owner,
            thumbnail,
            mode,
            output,
        } => {
            return extract_image(&document, *owner, *thumbnail, *mode, output);
        }
    }
    finish(&document, &mut report, out)
}

fn finish(document: &Document, report: &mut Vec<u8>, out: &mut impl Write) -> Result<(), Failure> {
    report.write_all(b",")?;
    json::diagnostics(report, document)?;
    report.write_all(b"}\n")?;
    out.write_all(report)?;
    Ok(())
}

fn extract_image(
    document: &Document,
    owner: MetadataOwner,
    thumbnail: bool,
    mode: pagura::images::ImageTransferMode,
    output: &std::path::Path,
) -> Result<(), Failure> {
    let id = match owner {
        MetadataOwner::Page(page) => document.page(page)?.id(),
        MetadataOwner::Object(id) => id,
        _ => {
            return Err(Failure::Request(
                "image requires page or object selection".into(),
            ));
        }
    };
    let image = if thumbnail {
        document
            .thumbnail(id)?
            .ok_or_else(|| Failure::Request("selected owner has no thumbnail".into()))?
    } else {
        document.image_asset(id)?
    };
    write_new(output, &image.transfer(mode)?.data)?;
    Ok(())
}

pub(super) fn details(out: &mut impl Write, file: &FileSpecification<'_>) -> std::io::Result<()> {
    out.write_all(b",\"identifiers_hex\":")?;
    if let Some(ids) = file.identifiers {
        out.write_all(b"[")?;
        hex(out, Some(ids[0]))?;
        out.write_all(b",")?;
        hex(out, Some(ids[1]))?;
        out.write_all(b"]")?;
    } else {
        out.write_all(b"null")?;
    }
    out.write_all(b",\"related\":[")?;
    let mut indexes = std::collections::BTreeMap::new();
    for (ordinal, related) in file.related.iter().enumerate() {
        if ordinal > 0 {
            out.write_all(b",")?;
        }
        let index = indexes.entry(related.key.as_bytes()).or_insert(0_usize);
        write!(out, "{{\"index\":{index},\"key_hex\":")?;
        *index += 1;
        hex(out, Some(related.key.as_bytes()))?;
        out.write_all(b",\"name_hex\":")?;
        hex(out, Some(related.name))?;
        out.write_all(b",\"object\":")?;
        object(out, Some(related.file.object()))?;
        out.write_all(b",\"media_type_hex\":")?;
        hex(out, related.file.media_type().map(Name::as_bytes))?;
        out.write_all(b",\"parameters\":")?;
        parameters(out, &related.file)?;
        out.write_all(b"}")?;
    }
    out.write_all(b"],\"collection_item\":[")?;
    for (ordinal, item) in file.collection_item.iter().enumerate() {
        if ordinal > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"key_hex\":")?;
        hex(out, Some(item.key.as_bytes()))?;
        out.write_all(b",\"prefix\":")?;
        json::optional(out, item.prefix.as_deref())?;
        out.write_all(b",\"value\":")?;
        match &item.value {
            Some(CollectionValue::Text(value)) => json::string(out, value)?,
            Some(CollectionValue::Integer(value)) => write!(out, "{value}")?,
            Some(CollectionValue::Real(value)) => write!(out, "{value}")?,
            _ => out.write_all(b"null")?,
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"],\"encrypted_payload\":")?;
    if let Some(payload) = file.encrypted_payload {
        out.write_all(b"{\"subtype_hex\":")?;
        hex(out, Some(payload.subtype.as_bytes()))?;
        out.write_all(b",\"version_hex\":")?;
        hex(out, payload.version.map(Name::as_bytes))?;
        out.write_all(b"}")?;
    } else {
        out.write_all(b"null")?;
    }
    Ok(())
}

fn collection(document: &Document, out: &mut impl Write) -> Result<(), Failure> {
    let collection = document.collection()?;
    out.write_all(b"{\"schema\":1,\"collection\":")?;
    if let Some(collection) = collection {
        out.write_all(b"{\"object\":")?;
        object(out, collection.object)?;
        out.write_all(b",\"initial_file_hex\":")?;
        hex(out, collection.initial_file)?;
        write!(out, ",\"view\":\"{:?}\",\"fields\":[", collection.view)?;
        for (ordinal, field) in collection.fields.iter().enumerate() {
            if ordinal > 0 {
                out.write_all(b",")?;
            }
            out.write_all(b"{\"key_hex\":")?;
            hex(out, Some(field.key.as_bytes()))?;
            out.write_all(b",\"name\":")?;
            json::string(out, &field.name)?;
            write!(
                out,
                ",\"kind\":\"{:?}\",\"visible\":{},\"editable\":{},\"order\":",
                field.kind, field.visible, field.editable
            )?;
            if let Some(order) = field.order {
                write!(out, "{order}")?;
            } else {
                out.write_all(b"null")?;
            }
            out.write_all(b"}")?;
        }
        out.write_all(b"],\"sort\":[")?;
        for (ordinal, sort) in collection.sort.iter().enumerate() {
            if ordinal > 0 {
                out.write_all(b",")?;
            }
            out.write_all(b"{\"key_hex\":")?;
            hex(out, Some(sort.key.as_bytes()))?;
            write!(out, ",\"ascending\":{}}}", sort.ascending)?;
        }
        out.write_all(b"]}")?;
    } else {
        out.write_all(b"null")?;
    }
    Ok(())
}

fn images(document: &Document, owner: MetadataOwner, out: &mut impl Write) -> Result<(), Failure> {
    out.write_all(b"{\"schema\":1,\"scope\":\"direct-resources-or-object\",\"images\":[")?;
    match owner {
        MetadataOwner::Page(page) => {
            for (ordinal, resource) in document.page_image_resources(page)?.iter().enumerate() {
                if ordinal > 0 {
                    out.write_all(b",")?;
                }
                image(out, resource.asset, Some(resource.name.as_bytes()))?;
            }
            out.write_all(b"],\"thumbnail\":")?;
            if let Some(thumbnail) = document.thumbnail(document.page(page)?.id())? {
                image(out, thumbnail, None)?;
            } else {
                out.write_all(b"null")?;
            }
        }
        MetadataOwner::Object(id) => {
            image(out, document.image_asset(id)?, None)?;
            out.write_all(b"]")?;
        }
        _ => {
            return Err(Failure::Request(
                "image inventory requires page or object selection".into(),
            ));
        }
    }
    Ok(())
}

fn image(
    out: &mut impl Write,
    image: pagura::images::ImageAsset<'_>,
    name: Option<&[u8]>,
) -> std::io::Result<()> {
    out.write_all(b"{\"object\":")?;
    object(out, Some(image.object()))?;
    out.write_all(b",\"resource_hex\":")?;
    hex(out, name)?;
    write!(
        out,
        ",\"width\":{},\"height\":{},\"thumbnail\":{}}}",
        image.width(),
        image.height(),
        image.is_thumbnail()
    )
}
