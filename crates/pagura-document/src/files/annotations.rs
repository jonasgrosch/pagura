//! Inert file-attachment annotation discovery (12.5.6.15, Table 187).

use super::{FileSpecification, read};
use crate::source::ReadResult;
use crate::view::{Context, Source, number};
use crate::{Document, Page};
use pagura_core::{Dictionary, Error, Name, ObjectId, Rect, Value};

/// A page's `FileAttachment` annotation and its unactivated file specification.
/// This view does not render an icon, follow a path or launch an attached file.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AttachmentAnnotation<'a> {
    /// Index in the page's original Annots array, including other annotation types.
    pub index: usize,
    /// Resolved annotation dictionary identity, separate from the FS identity.
    pub object: ObjectId,
    /// Original annotation dictionary, including flags, markup and appearance data.
    pub dictionary: &'a Dictionary,
    /// Normalized annotation rectangle in default user space, before page rotation.
    pub rectangle: Rect,
    /// Annotation Contents text. Interactive descriptions use this instead of FS/Desc.
    /// Absent and empty strings remain distinct; no FS/Desc fallback is inferred.
    pub contents: Option<String>,
    /// Declared icon name, including extension names; absent means `PushPin`.
    pub icon: Option<&'a Name>,
    /// File specification from FS, preserving EF/RF handles and original names.
    pub specification: FileSpecification<'a>,
}

impl AttachmentAnnotation<'_> {
    /// The declared icon name or the Table 187 default, `PushPin`.
    pub fn icon_name(&self) -> &[u8] {
        self.icon.map_or(b"PushPin".as_slice(), Name::as_bytes)
    }
}

impl Page {
    /// Lists this page's `FileAttachment` annotations in Annots order (12.5.6.15).
    /// Catalog EmbeddedFiles and AF associations remain separate views: the same
    /// payload can occur in several places, and this method does not deduplicate.
    /// Valid other annotation subtypes are skipped, including their geometry.
    /// Payload decoding and attachment authorization remain lazy.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a page from another document, positioned
    /// syntax/reference/cycle errors for malformed Annots or attachment entries,
    /// reported text-repair errors and bounded traversal/result allocation errors.
    /// Annots members must be indirect dictionaries (Table 31); FS and Rect are
    /// required for a `FileAttachment`. No partial result is returned on failure.
    pub fn attachment_annotations<'a>(
        &'a self,
        document: &'a Document,
    ) -> ReadResult<Vec<AttachmentAnnotation<'a>>> {
        document.check_page(self)?;
        let mut context = Context::new(document);
        let source = context.source(self.id())?;
        let Some((value, source)) =
            read::entry(&mut context, self.dictionary(), b"Annots", source)?
        else {
            return Ok(Vec::new());
        };
        let values = value
            .as_array()
            .ok_or_else(|| source.error(Error::syntax("page Annots must be an array")))?;
        let mut result = Vec::new();
        for (index, value) in values.iter().enumerate() {
            context.step(0, source)?;
            if !matches!(value, Value::Reference(_)) {
                return Err(source
                    .error(Error::syntax("annotation must be indirect"))
                    .into());
            }
            let (dictionary, source) = context.dictionary(value, source)?;
            let object = source
                .object
                .ok_or_else(|| source.error(Error::syntax("annotation identity is absent")))?;
            if document.object(object)?.is_stream() {
                return Err(source
                    .error(Error::syntax("annotation must not be a stream"))
                    .into());
            }
            let subtype = read::name(&mut context, dictionary, b"Subtype", source)?
                .ok_or_else(|| source.error(Error::syntax("annotation Subtype is absent")))?;
            if subtype.as_bytes() != b"FileAttachment" {
                continue;
            }
            context.type_name(dictionary, b"Annot", source)?;
            context.allocate::<AttachmentAnnotation<'_>>(1, source)?;
            let file = context
                .present(dictionary, b"FS", source)?
                .ok_or_else(|| source.error(Error::syntax("file attachment requires FS")))?;
            result.push(AttachmentAnnotation {
                index,
                object,
                dictionary,
                rectangle: rectangle(&mut context, dictionary, source)?,
                contents: read::text(&mut context, dictionary, b"Contents", source)?,
                icon: read::name(&mut context, dictionary, b"Name", source)?,
                specification: read::specification(&mut context, file, source)?,
            });
        }
        Ok(result)
    }
}

fn rectangle<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<Rect> {
    let (value, source) = read::entry(context, dictionary, b"Rect", source)?
        .ok_or_else(|| source.error(Error::syntax("file attachment requires Rect")))?;
    let values = value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("annotation Rect must be an array")))?;
    let [x0, y0, x1, y1] = values else {
        return Err(source
            .error(Error::syntax("annotation Rect requires four numbers"))
            .into());
    };
    let mut coordinate = |value| {
        context
            .resolve(value, source)
            .and_then(|(value, source)| Ok(number(value, source)?))
    };
    Ok(Rect::from_corners(
        coordinate(x0)?,
        coordinate(y0)?,
        coordinate(x1)?,
        coordinate(y1)?,
    )
    .ok_or_else(|| source.error(Error::syntax("invalid annotation rectangle")))?)
}
