//! Logical content references, ISO 32000-2 14.7.2–14.7.5 and 14.8.
use super::{LogicalText, Name, ObjectId, Range, Result, TextPage};
use pagura_core::{Diagnostics, Dictionary, Error, Meter, Position, Value};
use pagura_document::{Document, Page};
use std::collections::{BTreeMap, BTreeSet};
mod content;
mod copy;
mod roles;
mod tree;
pub use roles::StructureNamespace;

/// One structure element contributing to this page, in logical preorder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
/// Storage of one map entry with small keys and values, including the
/// tree's node overhead.
const MAP_ENTRY_BYTES: usize = 80;
/// Storage of one marked-content occurrence besides its glyph ranges: its
/// entries in the occurrence and visibility maps.
const OCCURRENCE_BYTES: usize = 192;

struct Budget {
    work: Meter,
    bytes: Meter,
    visits: Meter,
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
        self.work.charge(amount)
    }
    fn store(&mut self, amount: usize) -> Result<()> {
        self.bytes.charge(amount)
    }
    /// One node of the structure tree walk (`traversal_work`).
    fn visit(&mut self) -> Result<()> {
        self.visits.charge(1)
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
        pagura_core::ErrorKind::Unsupported(feature) => crate::features::ALL.contains(&feature),
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
        .map_or(Position::Unknown, |o| o.source().position());
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
    let limits = document.limits();
    let mut budget = Budget {
        work: Meter::new(limits.text_work, "logical text work budget exceeded"),
        bytes: Meter::new(
            limits.document_bytes,
            "logical text storage budget exceeded",
        ),
        visits: Meter::new(
            limits.traversal_work,
            "structure tree traversal budget exceeded",
        ),
        limits,
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

/// Decodes a text string (7.9.2.2) under the document's recovery mode:
/// repair mode reports invalid sequences and keeps U+FFFD in their place.
fn text_string(doc: &Document, bytes: &[u8], object: Option<ObjectId>) -> Result<String> {
    let mut diagnostics = Diagnostics::new(doc.recovery());
    let text = pagura_core::text_string::decode(bytes, &doc.limits(), &mut diagnostics)
        .map_err(|error| error.in_object_opt(object))?;
    doc.report(
        diagnostics
            .take()
            .into_iter()
            .map(|diagnostic| diagnostic.in_object_opt(object)),
    );
    Ok(text)
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
