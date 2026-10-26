//! One exact selection model for library, CLI and host attachment operations.

use super::{EmbeddedFile, FileSpecification};
use crate::Document;
use pagura_core::{Error, ObjectId, Result};

/// The collection in which a file index has meaning. No fallback is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileScope {
    /// The catalog EmbeddedFiles name tree, in byte-key order.
    Catalog,
    /// The catalog AF array, in source order.
    Associated,
    /// A page AF array, in source order; page ordinals start at zero.
    PageAssociated(usize),
    /// An object's AF array, in source order and with exact generation identity.
    ObjectAssociated(ObjectId),
    /// FileAttachment annotations, indexed by their original Annots slots.
    PageAnnotations(usize),
}

/// One file in an explicitly selected collection (7.11, 14.13).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FileRecord<'a> {
    /// Index in the selected collection; Annots slots can be nonconsecutive.
    pub index: usize,
    /// Original EmbeddedFiles key, present only for catalog name-tree entries.
    pub key: Option<&'a [u8]>,
    /// Inert file specification and lazy EF/RF streams.
    pub specification: FileSpecification<'a>,
}

/// Exact payload within one file specification. Names and indexes remain data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilePayload<'a> {
    /// EF entry with this PDF filename key.
    Embedded(&'a [u8]),
    /// Zero-based RF pair under this exact EF key; duplicate names are distinct.
    Related {
        /// EF filename key to which the related files belong.
        key: &'a [u8],
        /// Original pair ordinal within this key's RF array.
        index: usize,
    },
}

impl Document {
    /// Lists files in exactly `scope`, without decoding payloads (7.11, 14.13).
    /// No partial list is returned on failure, and URLs/paths are never opened.
    ///
    /// # Errors
    /// Invalid page/object identity, malformed views, cycles and document budgets.
    pub fn files(&self, scope: FileScope) -> Result<Vec<FileRecord<'_>>> {
        let records: Vec<FileRecord<'_>> = match scope {
            FileScope::Catalog => self
                .embedded_files()?
                .into_iter()
                .enumerate()
                .map(|(index, file)| FileRecord {
                    index,
                    key: Some(file.key),
                    specification: file.specification,
                })
                .collect(),
            FileScope::PageAnnotations(page) => self
                .page(page)?
                .attachment_annotations(self)?
                .into_iter()
                .map(|annotation| FileRecord {
                    index: annotation.index,
                    key: None,
                    specification: annotation.specification,
                })
                .collect(),
            _ => {
                let files = match scope {
                    FileScope::Associated => self.associated_files()?,
                    FileScope::PageAssociated(page) => self.page(page)?.associated_files(self)?,
                    FileScope::ObjectAssociated(id) => {
                        let object = self.object(id)?;
                        let dictionary = object.dictionary().ok_or_else(|| {
                            Error::syntax("associated-file owner must be a dictionary")
                                .in_object(id)
                        })?;
                        self.object_associated_files(dictionary)?
                    }
                    _ => return Err(Error::invalid_argument("unsupported file scope")),
                };
                files
                    .into_iter()
                    .enumerate()
                    .map(|(index, specification)| FileRecord {
                        index,
                        key: None,
                        specification,
                    })
                    .collect()
            }
        };
        // The source views above separately budget their traversal. The adapted
        // records are also charged before they are returned to the caller.
        let mut context = crate::view::Context::new(self);
        context.allocate::<FileRecord<'_>>(records.len(), crate::view::Source::default())?;
        Ok(records)
    }

    /// Selects one EF or RF stream using the same indexes as [`Self::files`].
    /// Discovery and selection do not decode or authorize the payload.
    ///
    /// # Errors
    /// As [`Self::files`], or `InvalidArgument` for an absent index/key/RF pair.
    pub fn file_payload(
        &self,
        scope: FileScope,
        index: usize,
        payload: FilePayload<'_>,
    ) -> Result<EmbeddedFile<'_>> {
        let file = self
            .files(scope)?
            .into_iter()
            .find(|file| file.index == index)
            .ok_or_else(|| {
                Error::invalid_argument("file index is outside the selected collection")
            })?;
        match payload {
            FilePayload::Embedded(key) => file
                .specification
                .embedded
                .into_iter()
                .find(|entry| entry.key.as_bytes() == key)
                .map(|entry| entry.file),
            FilePayload::Related { key, index } => file
                .specification
                .related
                .into_iter()
                .filter(|entry| entry.key.as_bytes() == key)
                .nth(index)
                .map(|entry| entry.file),
        }
        .ok_or_else(|| Error::invalid_argument("file has no payload for this exact selection"))
    }
}
