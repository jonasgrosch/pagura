//! Logical content references, ISO 32000-2 14.7.2–14.7.5 and 14.8.
use super::{LogicalText, Name, ObjectId, Range, Result, TextPage};
use pagura_core::{Dictionary, Error, Feature, Position, Value};
use pagura_document::{Document, Page};
use std::collections::{BTreeMap, BTreeSet};
mod content;
mod copy;
mod roles;
mod tree;
pub use roles::StructureNamespace;

/// One structure element contributing to this page, in logical preorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureElement {
    /// Indirect identity, or None for a direct grouping dictionary.
    pub object: Option<ObjectId>,
    /// Parent index in this page's structure sequence; None for a root child.
    pub parent: Option<usize>,
    /// Original /S name.
    pub role: Name,
    /// Recognised standard type after role mapping; unknown custom types retain
    /// their role but have no recognised type (14.7.3).
    pub standard_type: Option<Name>,
    /// Standard namespace in which the role was interpreted.
    pub namespace: StructureNamespace,
    /// UTF-8 range in LogicalText::text, including descendant text.
    pub range: Range<usize>,
}

pub(super) const CONTENT: Feature = Feature::new("text.structure-content");
pub(super) const NAMESPACE: Feature = Feature::new("text.structure-namespace");
pub(super) const REPLACEMENT: Feature = Feature::new("text.structure-replacement");
struct ReplacementText {
    text: String,
    kind: super::layout::Replacement,
}
struct Node<'a> {
    dictionary: &'a Dictionary,
    element: StructureElement,
    end: usize,
    replacement: Option<ReplacementText>,
    artifact: bool,
    relevant: bool,
    other_page: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reference {
    Marked(ObjectId, i64),
    Object(ObjectId),
}
struct Item {
    node: usize,
    reference: Reference,
    ranges: Vec<Range<usize>>,
    visible: bool,
}
struct Plan<'a> {
    tagged: bool,
    nodes: Vec<Node<'a>>,
    items: Vec<Item>,
}
struct Budget {
    work: usize,
    bytes: usize,
    limits: pagura_core::Limits,
    depth_exhausted: bool,
}
impl Budget {
    fn depth(&mut self, depth: usize) -> Result<()> {
        if depth > self.limits.depth {
            // Optional structure traversal stops at the configured depth. This
            // does not relax parser, capture, number-tree or rendering limits.
            self.depth_exhausted = true;
            return Err(Error::limit("structure tree depth exceeded"));
        }
        Ok(())
    }
    fn work(&mut self, amount: usize) -> Result<()> {
        super::charge(&mut self.work, amount, self.limits.operations)
    }
    fn store(&mut self, amount: usize) -> Result<()> {
        super::charge(&mut self.bytes, amount, self.limits.document_bytes)
    }
}

// Only optional structure interpretation is recoverable here. Compilation has
// already succeeded; resource failures and unrelated unsupported capabilities
// must retain their original semantics.
fn can_fallback(error: &Error) -> bool {
    match error.kind {
        pagura_core::ErrorKind::Syntax
        | pagura_core::ErrorKind::Cycle
        | pagura_core::ErrorKind::MissingObject => true,
        pagura_core::ErrorKind::Unsupported(feature) => {
            [CONTENT, NAMESPACE, REPLACEMENT].contains(&feature)
        }
        _ => false,
    }
}

pub(super) struct Failure {
    pub(super) error: Error,
    pub(super) recoverable: bool,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self {
            recoverable: can_fallback(&error),
            error,
        }
    }
}

pub(super) fn apply(
    text: &TextPage,
    document: &Document,
    page: &Page,
) -> std::result::Result<Option<LogicalText>, Failure> {
    let Some(root) = entry(document, document.catalog()?, b"StructTreeRoot")? else {
        if entry(document, page.dictionary(), b"StructParents")?.is_some()
            || tree::tagged(document).map_err(|e| located(document, page.id(), e))?
        {
            return Err(located(
                document,
                page.id(),
                Error::syntax("tagged page has no structure tree root (14.7.2)"),
            )
            .into());
        }
        return Ok(None);
    };
    let object = root.as_reference().unwrap_or(page.id());
    let result = apply_root(text, document, page, root).map(Some);
    result.map_err(|mut failure| {
        failure.error = located(document, object, failure.error);
        failure
    })
}
fn located(document: &Document, fallback: ObjectId, error: Error) -> Error {
    let object = error.object.unwrap_or(fallback);
    let position = document
        .object(object)
        .map_or(Position::Unknown, |o| match o.source() {
            pagura_document::ObjectSource::File(range) => Position::File(range.start),
            pagura_document::ObjectSource::Compressed { stream, .. } => Position::Stream {
                object: ObjectId::new(*stream, 0),
                offset: 0,
            },
        });
    error.in_object(object).at(position)
}

fn apply_root(
    text: &TextPage,
    document: &Document,
    page: &Page,
    root: &Value,
) -> std::result::Result<LogicalText, Failure> {
    let root = document.resolve_dictionary(root)?;
    if name(document, root, b"Type")?.map(Name::as_bytes) != Some(b"StructTreeRoot") {
        return Err(Error::syntax("invalid structure tree root Type (14.7.2)").into());
    }
    let mut budget = Budget {
        work: 0,
        bytes: 0,
        limits: document.limits(),
        depth_exhausted: false,
    };
    let result = (|| {
        let mut plan = tree::read(document, root, page.id(), &mut budget)?;
        content::resolve(&mut plan, document, root, page, text, &mut budget)?;
        copy::export(text, &plan, &mut budget)
    })();
    result.map_err(|error| Failure {
        recoverable: budget.depth_exhausted || can_fallback(&error),
        error,
    })
}

// Null-valued dictionary entries are absent, including indirect null (7.3.9).
// Keep the original reference for identities such as Pg, Stm and NS.
fn entry<'a>(
    doc: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a Value>> {
    match dictionary.get(key) {
        Some(value) if !doc.resolve(value)?.is_null() => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn name<'a>(doc: &'a Document, dictionary: &'a Dictionary, key: &[u8]) -> Result<Option<&'a Name>> {
    entry(doc, dictionary, key)?
        .map(|v| {
            doc.resolve(v).and_then(|v| {
                v.as_name()
                    .ok_or(Error::syntax("structure name property has the wrong type"))
            })
        })
        .transpose()
}
