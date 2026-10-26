//! Inert file specifications and embedded/associated files (7.11, 14.13).
//!
//! Paths, URLs, MIME names and relationships are source data, never instructions
//! to open a file, connect to a server or execute content. Payload decoding is
//! lazy and applies the document's filters, crypt filters and budgets. Declared
//! sizes, dates and checksums are preserved. Explicit [`EmbeddedFile::check_parameters`]
//! compares Size and CheckSum with decoded bytes; dates remain unvalidated.
//! Collection schemas, item values and encrypted-payload declarations are inert
//! typed views. Executable navigators and MIME/date/profile validation remain outside
//! these views; their source entries remain available to callers.

mod annotations;
mod checks;
mod collection;
mod model;
mod read;
mod selection;
mod stream;

pub use annotations::AttachmentAnnotation;
pub use checks::{FileParameterChecks, ParameterCheck};
pub use collection::{
    Collection, CollectionField, CollectionFieldKind, CollectionItem, CollectionSort,
    CollectionValue, CollectionView, EncryptedPayload,
};
pub use model::{
    EmbeddedFile, EmbeddedFileEntry, FileName, FileNames, FileParameters, FileSpecification,
    NamedFile, RelatedFile, Relationship,
};
pub use selection::{FilePayload, FileRecord, FileScope};

use crate::navigation::trees::NameTree;
use crate::source::ReadResult;
use crate::view::{Context, Source};
use crate::{Document, Page};
use pagura_core::{Dictionary, Error, Value};

impl Document {
    /// Reads an inert file specification string or dictionary (7.11).
    /// References are interpreted in this document; source bytes remain intact.
    /// Pass the original reference to retain identity: EF/RF specifications
    /// require an indirect dictionary, not a detached copy of its value.
    ///
    /// # Errors
    /// Positioned syntax, reference/cycle, text-repair and view-budget errors.
    /// Payload filters and authorization are checked lazily by [`EmbeddedFile::data`].
    pub fn file_specification<'a>(&'a self, value: &'a Value) -> ReadResult<FileSpecification<'a>> {
        read::specification(&mut Context::new(self), value, Source::default())
    }

    /// Catalog EmbeddedFiles name-tree entries in byte-key order (7.7.4, 7.11.4).
    /// Keys are preserved separately from the file's F/UF names. This collection
    /// does not imply an exhaustive search of annotations or arbitrary objects.
    ///
    /// # Errors
    /// As [`Self::file_specification`], plus bounded name-tree validation and
    /// reported tree repairs. No partial result is returned after failure.
    pub fn embedded_files(&self) -> ReadResult<Vec<NamedFile<'_>>> {
        let mut context = Context::new(self);
        let (catalog, source) = context.catalog()?;
        let Some(names) = context.present(catalog, b"Names", source)? else {
            return Ok(Vec::new());
        };
        let (names, source) = context.dictionary(names, source)?;
        let Some(root) = context.present(names, b"EmbeddedFiles", source)? else {
            return Ok(Vec::new());
        };
        let entries = NameTree::new(self, root)
            .with_source(source)
            .entries_with(&mut context)?;
        context.allocate::<NamedFile<'_>>(entries.len(), source)?;
        let mut files = Vec::new();
        for entry in entries {
            context.step(0, entry.source)?;
            files.push(NamedFile {
                key: entry.key,
                specification: read::specification(&mut context, entry.value, entry.source)?,
            });
        }
        Ok(files)
    }

    /// Files associated with the catalog, preserving AF array order (14.13.3).
    /// External specifications remain inert and may have no embedded payload.
    ///
    /// # Errors
    /// As [`Self::file_specification`], including a non-array AF or a non-dictionary
    /// member. This reads the associations; it does not validate a PDF profile.
    pub fn associated_files(&self) -> ReadResult<Vec<FileSpecification<'_>>> {
        let mut context = Context::new(self);
        let (catalog, source) = context.catalog()?;
        associated(&mut context, catalog, source)
    }

    /// Files associated with an object dictionary (14.13).
    /// References in `dictionary` are interpreted in this immutable document.
    ///
    /// # Errors
    /// As [`Self::associated_files`]. Payload bytes remain lazy.
    pub fn object_associated_files<'a>(
        &'a self,
        dictionary: &'a Dictionary,
    ) -> ReadResult<Vec<FileSpecification<'a>>> {
        associated(&mut Context::new(self), dictionary, Source::default())
    }
}

impl Page {
    /// Files associated with this page's dictionary (14.13.4).
    ///
    /// # Errors
    /// As [`Document::associated_files`], or `InvalidArgument` when the page
    /// belongs to another document.
    pub fn associated_files<'a>(
        &'a self,
        document: &'a Document,
    ) -> ReadResult<Vec<FileSpecification<'a>>> {
        document.check_page(self)?;
        let mut context = Context::new(document);
        let source = context.source(self.id())?;
        associated(&mut context, self.dictionary(), source)
    }
}

fn associated<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<FileSpecification<'a>>> {
    let Some((value, source)) = read::entry(context, dictionary, b"AF", source)? else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("associated files must be an array")))?;
    context.allocate::<FileSpecification<'_>>(values.len(), source)?;
    let mut files = Vec::new();
    for value in values {
        context.step(0, source)?;
        let specification = read::specification(context, value, source)?;
        if specification.source.as_dictionary().is_none() {
            return Err(source
                .error(Error::syntax("associated file requires a dictionary"))
                .into());
        }
        files.push(specification);
    }
    Ok(files)
}
