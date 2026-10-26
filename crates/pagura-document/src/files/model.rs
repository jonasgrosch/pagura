use crate::{Document, IndirectObject};
use pagura_core::{Dictionary, Name, ObjectId, Result, Value};
use std::borrow::Cow;

/// A filename as declared, without operating-system path interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileName<'a> {
    /// UF decoded using the PDF text-string rules.
    Unicode(&'a str),
    /// A simple string or F path, whose bytes have no inferred text encoding.
    Bytes(&'a [u8]),
}

/// Standard and legacy file names (Table 43), with original path bytes retained.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct FileNames<'a> {
    /// F, or the entire value of a simple file specification string.
    pub file: Option<&'a [u8]>,
    /// UF text; when present it takes precedence over F, even when empty.
    pub unicode: Option<String>,
    /// Legacy DOS path bytes; deprecated in PDF 2.0.
    pub dos: Option<&'a [u8]>,
    /// Legacy Mac path bytes; deprecated in PDF 2.0.
    pub mac: Option<&'a [u8]>,
    /// Legacy Unix path bytes; deprecated in PDF 2.0.
    pub unix: Option<&'a [u8]>,
}

impl FileNames<'_> {
    /// The standard UF-before-F choice (Table 43), without choosing a platform
    /// for a specification containing only legacy DOS/Mac/Unix entries.
    pub fn preferred(&self) -> Option<FileName<'_>> {
        self.unicode
            .as_deref()
            .map(FileName::Unicode)
            .or_else(|| self.file.map(FileName::Bytes))
    }
}

/// A producer-declared associated-file relationship, never an execution request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Relationship<'a> {
    /// Original source material.
    Source,
    /// Data used to derive a visual representation.
    Data,
    /// An alternative representation of the content.
    Alternative,
    /// Supplemental content.
    Supplement,
    /// An encrypted payload declaration; decryption/display is not implied.
    EncryptedPayload,
    /// Data associated with the AcroForm.
    FormData,
    /// A schema for the associated object.
    Schema,
    /// Unspecified or absent relationship (the PDF default).
    #[default]
    Unspecified,
    /// An extension relationship, preserved without inferred semantics.
    Other(&'a Name),
}

/// A file specification and its lazy embedded payload handles (7.11).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FileSpecification<'a> {
    /// The specification's indirect identity, if it was referenced indirectly.
    pub object: Option<ObjectId>,
    /// Resolved source value, retaining unknown/collection/payload entries.
    pub source: &'a Value,
    /// Unmodified path bytes and decoded UF text.
    pub names: FileNames<'a>,
    /// File system name, including URL and registered extensions.
    pub file_system: Option<&'a Name>,
    /// Optional descriptive text.
    pub description: Option<String>,
    /// Producer's volatile-file flag, defaulting to false.
    pub volatile: bool,
    /// Original/current file identifier pair, without verifying a target file.
    pub identifiers: Option<[&'a [u8]; 2]>,
    /// Producer-declared association, defaulting to Unspecified.
    pub relationship: Relationship<'a>,
    /// Custom collection data, with presentation prefixes kept separate.
    pub collection_item: Vec<super::CollectionItem<'a>>,
    /// Inert encrypted-payload declaration; no nested document is opened.
    pub encrypted_payload: Option<super::EncryptedPayload<'a>>,
    /// EF entries, preserving each key even when several refer to one stream.
    pub embedded: Vec<EmbeddedFileEntry<'a>>,
    /// RF pairs, preserving their parent EF key and declared related-file name.
    pub related: Vec<RelatedFile<'a>>,
}

/// One catalog EmbeddedFiles name-tree entry.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NamedFile<'a> {
    /// Raw name-tree key; distinct from the file's F/UF filename.
    pub key: &'a [u8],
    /// The referenced specification.
    pub specification: FileSpecification<'a>,
}

/// One EF dictionary entry (Table 43).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct EmbeddedFileEntry<'a> {
    /// Filename key, such as F or UF.
    pub key: &'a Name,
    /// Lazy embedded stream.
    pub file: EmbeddedFile<'a>,
}

/// One related-file pair in an RF array (7.11.4.2).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RelatedFile<'a> {
    /// The EF key to which this related file belongs.
    pub key: &'a Name,
    /// Uninterpreted related-file path bytes.
    pub name: &'a [u8],
    /// Lazy related-file stream.
    pub file: EmbeddedFile<'a>,
}

/// Source declarations from an embedded file's Params dictionary (Table 45).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FileParameters<'a> {
    /// Original dictionary, including deprecated Mac and extension entries.
    pub dictionary: &'a Dictionary,
    /// Declared uncompressed length; never used as an unchecked allocation hint.
    pub size: Option<u64>,
    /// Decoded source creation date, without calendar validation.
    pub creation_date: Option<String>,
    /// Decoded source modification date, without calendar validation.
    pub modification_date: Option<String>,
    /// Declared MD5 checksum, not a verification or security result.
    pub checksum: Option<&'a [u8; 16]>,
}

/// A validated embedded-stream handle whose payload remains unprocessed.
#[derive(Debug, Clone)]
pub struct EmbeddedFile<'a> {
    pub(super) document: &'a Document,
    pub(super) object: &'a IndirectObject,
    pub(super) media_type: Option<&'a Name>,
    pub(super) parameters: Option<FileParameters<'a>>,
}

impl<'a> EmbeddedFile<'a> {
    /// The stream's indirect identity in this immutable document revision.
    pub fn object(&self) -> ObjectId {
        self.object.id()
    }

    /// Original stream object, including its encoded byte range and dictionary.
    pub fn source(&self) -> &'a IndirectObject {
        self.object
    }

    /// Declared MIME type, with PDF name escapes already decoded.
    /// This does not validate MIME grammar or inspect the file's contents.
    pub fn media_type(&self) -> Option<&'a Name> {
        self.media_type
    }

    /// Optional source parameter declarations.
    pub fn parameters(&self) -> Option<&FileParameters<'a>> {
        self.parameters.as_ref()
    }

    /// Decodes the embedded bytes, never fetching an external file or executing
    /// content. Attachment crypt-filter selection applies even if the stream
    /// omits its optional Type. Permission flags remain host policy, distinct
    /// from password authentication.
    ///
    /// # Errors
    /// Stream/filter/decoded-budget errors, missing password authorization,
    /// or explicit refusal of external stream data. No partial payload is returned.
    pub fn data(&self) -> Result<Cow<'a, [u8]>> {
        crate::stream::embedded_decoded(self.document, self.object)
    }
}
