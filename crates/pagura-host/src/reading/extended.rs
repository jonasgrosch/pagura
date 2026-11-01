//! Protocol 1.12 exact document-service selections; source parsing stays in Pagura.

use super::{MAX_ATTACHMENTS, Response, services};
use crate::protocol::RequestKind;
use crate::wire::Writer;
use pagura::files::{CollectionValue, FilePayload, FileScope};
use pagura::images::{ImageAsset, ImageTransferMode};
use pagura::metadata::MetadataOwner;
use pagura::{Document, Error, Name, ObjectId, Result};

/// Exact file and payload selector for protocol 1.12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileSelection {
    /// Explicit collection; there is no fallback to catalog files.
    pub scope: FileScope,
    /// Collection index, or original Annots slot.
    pub index: usize,
    /// EF key or exact RF pair under an EF key.
    pub payload: FilePayload<'static>,
}

/// Additive synchronous document-service requests (1.12).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum ServiceRequest {
    /// Exact file collection with complete EF/RF metadata.
    Files(FileScope),
    /// Decode a selected EF/RF payload.
    FileBytes(FileSelection),
    /// Compare size/checksum declarations of a selected EF/RF payload.
    FileCheck(FileSelection),
    /// Metadata packet identity for one owner.
    Metadata(MetadataOwner),
    /// Metadata packet bytes for one owner.
    MetadataBytes(MetadataOwner),
    /// Portable collection declarations.
    Collection,
    /// Direct page image resources or one explicit image object.
    Images(MetadataOwner),
    /// Exact image bytes. A thumbnail selection names its owner, not its stream.
    ImageBytes {
        /// Page/object owner for thumbnails, or an explicit image object.
        owner: MetadataOwner,
        /// Whether `object` owns a Thumb entry.
        thumbnail: bool,
        /// Stored ciphertext/filter bytes or decrypted encoded asset bytes.
        mode: ImageTransferMode,
    },
    /// Stored thumbnail declaration for a page or object owner.
    Thumbnail(MetadataOwner),
}

impl ServiceRequest {
    /// Protocol request kind.
    pub const fn kind(self) -> RequestKind {
        match self {
            Self::Files(_) => RequestKind::Files,
            Self::FileBytes(_) => RequestKind::FileBytes,
            Self::FileCheck(_) => RequestKind::FileCheck,
            Self::Metadata(_) => RequestKind::ObjectMetadata,
            Self::MetadataBytes(_) => RequestKind::ObjectMetadataBytes,
            Self::Collection => RequestKind::Collection,
            Self::Images(_) => RequestKind::Images,
            Self::ImageBytes { .. } => RequestKind::ImageBytes,
            Self::Thumbnail(_) => RequestKind::Thumbnail,
        }
    }
}

pub(crate) fn service(
    document: &Document,
    request: ServiceRequest,
    cap: usize,
) -> Result<Response> {
    let kind = request.kind().response();
    let mut out = Writer::new(cap);
    match request {
        ServiceRequest::Files(scope) => {
            let files = document.files(scope)?;
            if files.len() > MAX_ATTACHMENTS {
                return Err(Error::limit("file listing budget exceeded"));
            }
            out.count(files.len())?;
            for file in files {
                out.count(file.index)?;
                services::bytes(&mut out, file.key)?;
                services::specification(&mut out, &file.specification)?;
                file_details(&mut out, &file.specification)?;
            }
        }
        ServiceRequest::FileBytes(selection) => {
            let file =
                document.file_payload(selection.scope, selection.index, selection.payload)?;
            return services::payload_bytes(&file, cap, kind);
        }
        ServiceRequest::FileCheck(selection) => {
            let file =
                document.file_payload(selection.scope, selection.index, selection.payload)?;
            return services::payload_check(&file, cap, kind);
        }
        ServiceRequest::Metadata(owner) => {
            services::object(
                &mut out,
                document.metadata_for(owner)?.map(|packet| packet.object()),
            )?;
        }
        ServiceRequest::MetadataBytes(owner) => {
            let packet = document.metadata_for(owner)?;
            out.flag(packet.is_some())?;
            if let Some(packet) = packet {
                services::blob(&mut out, &packet.data()?)?;
            }
        }
        ServiceRequest::Collection => collection(&mut out, document)?,
        ServiceRequest::Images(owner) => images(&mut out, document, owner)?,
        ServiceRequest::ImageBytes {
            owner,
            thumbnail,
            mode,
        } => {
            let object = owner_object(document, owner)?;
            let asset = if thumbnail {
                document
                    .thumbnail(object)?
                    .ok_or_else(|| Error::invalid_argument("selected owner has no thumbnail"))?
            } else {
                document.image_asset(object)?
            };
            let transferred = asset.transfer(mode)?;
            // Metadata names the mode/codec separately from the opaque bytes.
            out.text(match mode {
                ImageTransferMode::Stored => "stored",
                ImageTransferMode::Encoded => "encoded",
                _ => return Err(Error::invalid_argument("unsupported image transfer mode")),
            })?;
            services::text(
                &mut out,
                transferred
                    .codec
                    .as_ref()
                    .map(|(codec, _)| codec_name(*codec)),
            )?;
            services::blob(&mut out, &transferred.data)?;
        }
        ServiceRequest::Thumbnail(owner) => {
            let id = owner_object(document, owner)?;
            let thumbnail = document.thumbnail(id)?;
            out.flag(thumbnail.is_some())?;
            if let Some(thumbnail) = thumbnail {
                image(&mut out, thumbnail)?;
            }
        }
    }
    Ok(Response::new(kind, out))
}

fn codec_name(codec: pagura::ImageCodec) -> &'static str {
    match codec {
        pagura::ImageCodec::Dct => "DCTDecode",
        pagura::ImageCodec::Jpx => "JPXDecode",
        pagura::ImageCodec::Jbig2 => "JBIG2Decode",
        pagura::ImageCodec::CcittFax => "CCITTFaxDecode",
    }
}

fn owner_object(document: &Document, owner: MetadataOwner) -> Result<ObjectId> {
    match owner {
        MetadataOwner::Page(page) => Ok(document.page(page)?.id()),
        MetadataOwner::Object(id) => Ok(id),
        _ => Err(Error::invalid_argument(
            "operation requires a page or object owner",
        )),
    }
}

fn image(out: &mut Writer, image: ImageAsset<'_>) -> Result<()> {
    out.object(image.object())?;
    out.u32(image.width())?;
    out.u32(image.height())?;
    out.flag(image.is_thumbnail())
}

fn images(out: &mut Writer, document: &Document, owner: MetadataOwner) -> Result<()> {
    match owner {
        MetadataOwner::Page(page) => {
            let images = document.page_image_resources(page)?;
            if images.len() > MAX_ATTACHMENTS {
                return Err(Error::limit("image listing budget exceeded"));
            }
            out.count(images.len())?;
            for resource in images {
                services::bytes(out, Some(resource.name.as_bytes()))?;
                image(out, resource.asset)?;
            }
        }
        MetadataOwner::Object(id) => {
            let asset = document.image_asset(id)?;
            out.count(1)?;
            services::bytes(out, None)?;
            image(out, asset)?;
        }
        _ => {
            return Err(Error::invalid_argument(
                "image listing requires a page or object",
            ));
        }
    }
    Ok(())
}

fn file_details(out: &mut Writer, file: &pagura::files::FileSpecification<'_>) -> Result<()> {
    out.flag(file.identifiers.is_some())?;
    if let Some(ids) = file.identifiers {
        for id in ids {
            services::blob(out, id)?;
        }
    }
    out.count(file.embedded.len())?;
    for entry in &file.embedded {
        parameters(out, &entry.file)?;
    }
    bounded_count(out, file.related.len())?;
    for related in &file.related {
        services::blob(out, related.key.as_bytes())?;
        services::blob(out, related.name)?;
        out.object(related.file.object())?;
        services::bytes(out, related.file.media_type().map(Name::as_bytes))?;
        parameters(out, &related.file)?;
    }
    bounded_count(out, file.collection_item.len())?;
    for item in &file.collection_item {
        services::blob(out, item.key.as_bytes())?;
        services::text(out, item.prefix.as_deref())?;
        match &item.value {
            None => out.text("absent")?,
            Some(CollectionValue::Text(value)) => {
                out.text("text")?;
                out.text(value)?;
            }
            Some(CollectionValue::Integer(value)) => {
                out.text("integer")?;
                out.text(&value.to_string())?;
            }
            Some(CollectionValue::Real(value)) => {
                out.text("real")?;
                out.f64(*value)?;
            }
            _ => return Err(Error::invalid_argument("unsupported collection value")),
        }
    }
    out.flag(file.encrypted_payload.is_some())?;
    if let Some(payload) = file.encrypted_payload {
        services::blob(out, payload.subtype.as_bytes())?;
        services::bytes(out, payload.version.map(Name::as_bytes))?;
    }
    Ok(())
}

fn parameters(out: &mut Writer, file: &pagura::files::EmbeddedFile<'_>) -> Result<()> {
    let parameters = file.parameters();
    out.flag(parameters.is_some())?;
    if let Some(parameters) = parameters {
        out.flag(parameters.size.is_some())?;
        if let Some(size) = parameters.size {
            out.u64(size)?;
        }
        services::text(out, parameters.creation_date.as_deref())?;
        services::text(out, parameters.modification_date.as_deref())?;
        services::bytes(out, parameters.checksum.map(<[u8; 16]>::as_slice))?;
    }
    Ok(())
}

fn collection(out: &mut Writer, document: &Document) -> Result<()> {
    let collection = document.collection()?;
    out.flag(collection.is_some())?;
    if let Some(collection) = collection {
        services::object(out, collection.object)?;
        services::bytes(out, collection.initial_file)?;
        out.text(&format!("{:?}", collection.view))?;
        bounded_count(out, collection.fields.len())?;
        for field in collection.fields {
            services::blob(out, field.key.as_bytes())?;
            out.text(&field.name)?;
            out.text(&format!("{:?}", field.kind))?;
            services::text(out, field.order.map(|order| order.to_string()).as_deref())?;
            out.flag(field.visible)?;
            out.flag(field.editable)?;
        }
        bounded_count(out, collection.sort.len())?;
        for sort in collection.sort {
            services::blob(out, sort.key.as_bytes())?;
            out.flag(sort.ascending)?;
        }
    }
    Ok(())
}

fn bounded_count(out: &mut Writer, count: usize) -> Result<()> {
    if count > MAX_ATTACHMENTS {
        return Err(Error::limit("document-service record budget exceeded"));
    }
    out.count(count)
}
