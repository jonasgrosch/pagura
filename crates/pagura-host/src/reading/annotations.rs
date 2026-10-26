//! Page FileAttachment views retain Annots slots and never fall back to the catalog.

use super::{MAX_ATTACHMENTS, Response, services};
use crate::protocol::{AttachmentKey, Field, ResponseKind};
use crate::wire::Writer;
use pagura::files::EmbeddedFile;
use pagura::{Document, Error, Name, Result};

pub(crate) fn page_attachments(
    document: &Document,
    page: Field<u32>,
    cap: usize,
) -> Result<Response> {
    let annotations = document
        .page(page.index()?)?
        .attachment_annotations(document)?;
    if annotations.len() > MAX_ATTACHMENTS {
        return Err(Error::limit("page attachment listing budget exceeded"));
    }
    let mut out = Writer::new(cap);
    out.count(annotations.len())?;
    for annotation in annotations {
        out.count(annotation.index)?;
        out.object(annotation.object)?;
        let rect = annotation.rectangle;
        out.f64s(&[rect.x0(), rect.y0(), rect.x1(), rect.y1()])?;
        services::text(&mut out, annotation.contents.as_deref())?;
        services::bytes(&mut out, annotation.icon.map(Name::as_bytes))?;
        services::specification(&mut out, &annotation.specification)?;
    }
    Ok(Response::new(ResponseKind::PageAttachments, out))
}

pub(crate) fn page_attachment_bytes(
    document: &Document,
    page: Field<u32>,
    index: Field<u32>,
    key: Field<AttachmentKey>,
    cap: usize,
) -> Result<Response> {
    services::payload_bytes(
        &selected(document, page, index, key)?,
        cap,
        ResponseKind::PageAttachmentBytes,
    )
}

pub(crate) fn page_attachment_check(
    document: &Document,
    page: Field<u32>,
    index: Field<u32>,
    key: Field<AttachmentKey>,
    cap: usize,
) -> Result<Response> {
    services::payload_check(
        &selected(document, page, index, key)?,
        cap,
        ResponseKind::PageAttachmentCheck,
    )
}

fn selected(
    document: &Document,
    page: Field<u32>,
    index: Field<u32>,
    key: Field<AttachmentKey>,
) -> Result<EmbeddedFile<'_>> {
    let annotations = document
        .page(page.index()?)?
        .attachment_annotations(document)?;
    let slot = index.index()?;
    let annotation = annotations
        .into_iter()
        .find(|annotation| annotation.index == slot)
        .ok_or_else(|| {
            Error::invalid_argument("Annots slot is not a file attachment").at(index.position())
        })?;
    annotation
        .specification
        .embedded
        .into_iter()
        .find(|entry| entry.key.as_bytes() == key.get().pdf_name())
        .map(|entry| entry.file)
        .ok_or_else(|| {
            Error::invalid_argument("annotation attachment has no payload for this filename key")
                .at(key.position())
        })
}
