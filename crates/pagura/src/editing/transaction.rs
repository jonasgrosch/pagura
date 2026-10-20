//! Generation-aware, caller-owned object overlays.

use super::{SaveOptions, SignaturePolicy, features};
use crate::{Dictionary, Document, Error, Name, NewObject, ObjectId, Recovery, Result, Value};
use pagura_core::{INDEX_ENTRY_OVERHEAD, ResultExt};
use pagura_write::{XrefFormat, validate_object};
use std::{collections::BTreeMap, fmt};

/// A transaction over one immutable source snapshot.
///
/// `replace` and `add` validate syntax before publishing a staged value;
/// references are resolved against the complete overlay at commit, allowing
/// cycles and forward references. Failed operations leave the overlay intact.
/// Dropping or [`Self::rollback`] discards it. No source byte/cache is mutated.
/// Storage is a separate caller-owned `document_bytes` allowance (ADR 0031).
pub struct Transaction<'a> {
    document: &'a Document,
    objects: BTreeMap<ObjectId, NewObject>,
    bytes: usize,
    next: Option<u32>,
}

impl fmt::Debug for Transaction<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("objects", &self.objects.len())
            .field("retained_bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl<'a> Transaction<'a> {
    /// Starts an empty transaction without loading objects or copying input.
    pub fn new(document: &'a Document) -> Self {
        Self {
            document,
            objects: BTreeMap::new(),
            bytes: 0,
            next: None,
        }
    }

    /// The unchanged source snapshot.
    pub fn document(&self) -> &'a Document {
        self.document
    }

    /// Number of staged objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether saving will return the original bytes exactly.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// The staged replacement, if any.
    pub fn get(&self, id: ObjectId) -> Option<&NewObject> {
        self.objects.get(&id)
    }

    /// Restores the source value, or discards a newly added object.
    /// References to a discarded new object are rejected at commit.
    pub fn reset(&mut self, id: ObjectId) {
        if let Some(object) = self.objects.remove(&id) {
            self.bytes = self.bytes.saturating_sub(retained(&object));
        }
    }

    /// Discards all edits and returns the unchanged source snapshot.
    pub fn rollback(self) -> &'a Document {
        self.document
    }

    /// Replaces an existing source or staged object with the same generation.
    ///
    /// # Errors
    /// Returns `MissingObject` for an unknown identity; syntax and storage
    /// failures leave every staged object unchanged. Reference targets are
    /// validated at commit, not here.
    pub fn replace(&mut self, id: ObjectId, object: NewObject) -> Result<()> {
        if !self.document.contains_object(id) && !self.objects.contains_key(&id) {
            return Err(Error::missing_object("replacement identity is absent").in_object(id));
        }
        self.stage(id, object)
    }

    /// Adds an object above all source numbers and Size declarations.
    /// Free numbers are never reused; failed additions consume no number.
    ///
    /// # Errors
    /// Returns syntax or budget errors, including exhaustion of object numbers.
    pub fn add(&mut self, object: NewObject) -> Result<ObjectId> {
        let number = self.next.map_or_else(|| source_size(self.document), Ok)?;
        let next = number
            .checked_add(1)
            .ok_or(Error::limit("object number range exhausted"))?;
        let id = ObjectId::new(number, 0);
        self.stage(id, object)?;
        self.next = Some(next);
        Ok(id)
    }

    /// Copies and edits a non-stream dictionary, publishing only on success.
    /// Existing staged changes to that dictionary are included.
    ///
    /// # Errors
    /// Returns source/syntax/budget errors or the callback's error atomically.
    pub fn edit_dictionary(
        &mut self,
        id: ObjectId,
        edit: impl FnOnce(&mut Dictionary) -> Result<()>,
    ) -> Result<()> {
        let mut dictionary = self.dictionary(id)?;
        edit(&mut dictionary)?;
        self.replace(id, NewObject::Value(Value::Dictionary(dictionary)))
    }

    /// Commits to a new byte vector; the transaction remains available for retry.
    /// A no-op returns the original file exactly, without signature/recovery
    /// policy checks. Modified output preserves the entire original prefix.
    ///
    /// # Errors
    /// Returns `Unsupported` for encrypted/repair-mode/signed sources outside
    /// the selected policy; syntax, dangling-reference and budget errors
    /// produce no partial result. Does not validate arbitrary PDF schemas.
    pub fn commit(&self, options: &SaveOptions) -> Result<Vec<u8>> {
        let limits = self.document.limits();
        if self.is_empty() {
            return pagura_write::write_incremental(
                self.document.original_bytes(),
                0,
                self.document.trailer(),
                &self.objects,
                XrefFormat::Table,
                &limits,
            );
        }
        source_policy(self.document, options)?;
        let revision = self
            .document
            .revisions()
            .first()
            .ok_or(Error::invalid_argument(
                "incremental source has no revision",
            ))?;
        if self.document.revisions().len() >= limits.revisions {
            return Err(Error::limit("incremental revision budget exceeded"));
        }
        let format = match revision.kind() {
            pagura_document::RevisionKind::Table => XrefFormat::Table,
            pagura_document::RevisionKind::Stream | pagura_document::RevisionKind::Hybrid => {
                XrefFormat::Stream
            }
        };
        let extra = usize::from(format == XrefFormat::Stream);
        let count = self.document.revisions().iter().try_fold(
            self.objects.len().saturating_add(extra),
            |total, revision| total.checked_add(revision.entry_count()),
        );
        if count.is_none_or(|count| count > limits.objects) {
            return Err(Error::limit(
                "incremental cross-reference entry budget exceeded",
            ));
        }
        for (&id, object) in &self.objects {
            validate_object(object, &limits, &|target| {
                self.objects.contains_key(&target) || self.document.contains_object(target)
            })
            .in_object(id)?;
        }
        let root = self
            .document
            .trailer()
            .get(b"Root")
            .and_then(Value::as_reference)
            .ok_or(Error::invalid_argument("source Root must be indirect"))?;
        let catalog = self.dictionary(root)?;
        if !catalog.is_name(b"Type", b"Catalog") {
            return Err(
                Error::invalid_argument("root object must remain a catalog").in_object(root)
            );
        }
        let mut trailer = self.document.trailer().clone();
        trailer.insert(
            Name::new(b"Size"),
            Value::Integer(i64::from(source_size(self.document)?)),
        );
        pagura_write::write_incremental(
            self.document.original_bytes(),
            revision.offset(),
            &trailer,
            &self.objects,
            format,
            &limits,
        )
    }

    fn stage(&mut self, id: ObjectId, object: NewObject) -> Result<()> {
        let limits = self.document.limits();
        validate_object(&object, &limits, &|_| true).in_object(id)?;
        let previous = self.objects.get(&id).map_or(0, retained);
        let bytes = self
            .bytes
            .checked_sub(previous)
            .and_then(|bytes| bytes.checked_add(retained(&object)))
            .filter(|bytes| *bytes <= limits.document_bytes)
            .ok_or(Error::limit("transaction storage budget exceeded").in_object(id))?;
        if !self.objects.contains_key(&id) && self.objects.len() >= limits.objects {
            return Err(Error::limit("transaction object budget exceeded").in_object(id));
        }
        self.objects.insert(id, object);
        self.bytes = bytes;
        Ok(())
    }

    pub(super) fn dictionary(&self, id: ObjectId) -> Result<Dictionary> {
        let value = match self.objects.get(&id) {
            Some(NewObject::Value(value)) => value,
            Some(NewObject::Stream { .. }) => {
                return Err(
                    Error::invalid_argument("expected a non-stream dictionary").in_object(id)
                );
            }
            None => {
                let object = self.document.object(id)?;
                if object.is_stream() {
                    return Err(
                        Error::invalid_argument("expected a non-stream dictionary").in_object(id)
                    );
                }
                object.value()
            }
        };
        let dictionary = value
            .as_dictionary()
            .ok_or(Error::invalid_argument("expected a dictionary").in_object(id))?;
        if dictionary.retained_bytes() > self.document.limits().document_bytes {
            return Err(
                Error::limit("transaction dictionary storage budget exceeded").in_object(id),
            );
        }
        Ok(dictionary.clone())
    }
}

fn retained(object: &NewObject) -> usize {
    let bytes = match object {
        NewObject::Value(value) => value.retained_bytes(),
        NewObject::Stream { dictionary, data } => {
            dictionary.retained_bytes().saturating_add(data.capacity())
        }
    };
    bytes.saturating_add(size_of::<NewObject>() + size_of::<ObjectId>() + INDEX_ENTRY_OVERHEAD)
}

fn source_size(document: &Document) -> Result<u32> {
    let mut size = document
        .object_ids()
        .map(|id| id.number)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(Error::limit("object number range exhausted"))?;
    for revision in document.revisions() {
        let declared = revision
            .trailer()
            .get(b"Size")
            .and_then(Value::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or(Error::invalid_argument(
                "source revision requires valid Size",
            ))?;
        size = size.max(declared);
    }
    Ok(size)
}

fn source_policy(document: &Document, options: &SaveOptions) -> Result<()> {
    if document.recovery() != Recovery::Strict {
        return Err(Error::unsupported(
            features::REPAIRED,
            "modified save requires a strictly opened source",
        ));
    }
    if document
        .trailer()
        .get(b"Encrypt")
        .is_some_and(|value| !value.is_null())
    {
        return Err(Error::unsupported(
            features::ENCRYPTED,
            "encrypted incremental writing is unavailable",
        ));
    }
    if options.signatures == SignaturePolicy::Reject {
        // Until document PR #67 lands, history_complete also depends on the
        // document's diagnostic log. Inspect a fresh strict snapshot so earlier
        // rendering/information reports cannot alter this security decision.
        // This temporary copy/cache is separately bounded by the source limits.
        let original = document.original_bytes();
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(original.len())
            .map_err(|_| Error::limit("signature policy source copy exceeds memory"))?;
        bytes.extend_from_slice(original);
        let snapshot = Document::open(
            bytes,
            &crate::OpenOptions {
                limits: document.limits(),
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )?;
        let signatures = snapshot.signatures()?;
        if !signatures.history_complete
            || !signatures.historical.is_empty()
            || signatures.certification.is_some()
            || signatures.certification_error.is_some()
            || signatures.usage_rights.is_some()
            || signatures.usage_rights_error.is_some()
            || signatures
                .fields
                .iter()
                .any(|field| field.signature.is_some() || field.error.is_some())
        {
            return Err(Error::unsupported(
                features::SIGNED,
                "signed or uncertain source requires explicit preservation policy",
            ));
        }
    }
    Ok(())
}
