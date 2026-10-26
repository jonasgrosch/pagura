//! Bounded, source-located traversal shared by navigation, forms and
//! signatures: each view charges its steps to `Limits::traversal_work` and
//! its caller-owned results to `Limits::document_bytes`, and attaches the
//! object and position it read to every error and repair.

use crate::Document;
use crate::source::ReadResult;
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, INDEX_ENTRY_OVERHEAD, Meter, ObjectId, Position,
    Repair, Result, Value, text_string,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// Where a value was read: the object holding it and its position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) object: Option<ObjectId>,
    pub(crate) position: Position,
}

impl Source {
    /// Adds this source to `error` where it has none (innermost wins).
    pub(crate) const fn error(self, error: Error) -> Error {
        error.at(self.position).in_object_opt(self.object)
    }

    pub(crate) fn fault(
        self,
        fault: impl Into<crate::source::ReadFault>,
    ) -> crate::source::ReadFault {
        fault.into().map_error(|error| self.error(error))
    }

    pub(crate) const fn diagnostic(self, diagnostic: Diagnostic) -> Diagnostic {
        diagnostic.at(self.position).in_object_opt(self.object)
    }
}

/// Work charged by every context of one inspection, including the contexts
/// for earlier revisions of the same document.
struct Work {
    steps: Meter,
    bytes: Meter,
    scanned: Meter,
}

pub(crate) struct Context<'a> {
    pub(crate) document: &'a Document,
    work: Rc<RefCell<Work>>,
    pages: Option<BTreeMap<ObjectId, usize>>,
}

impl<'a> Context<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        let limits = document.limits();
        Self {
            document,
            work: Rc::new(RefCell::new(Work {
                steps: Meter::new(
                    limits.traversal_work,
                    "navigation traversal budget exceeded",
                ),
                bytes: Meter::new(limits.document_bytes, "navigation result budget exceeded"),
                scanned: Meter::new(limits.file_bytes, "source scan budget exceeded"),
            })),
            pages: None,
        }
    }

    /// A context for another revision of the document, charging this one's
    /// budgets.
    pub(crate) fn for_document<'b>(&self, document: &'b Document) -> Context<'b> {
        Context {
            document,
            work: Rc::clone(&self.work),
            pages: None,
        }
    }

    pub(crate) fn catalog(&mut self) -> ReadResult<(&'a Dictionary, Source)> {
        self.document.catalog()?;
        let root = self
            .document
            .trailer()
            .get(b"Root")
            .ok_or(Error::syntax("catalog Root is absent"))?;
        self.dictionary(root, Source::default())
    }

    pub(crate) fn source(&mut self, id: ObjectId) -> ReadResult<Source> {
        let object = self.document.object(id)?;
        Ok(Source {
            object: Some(id),
            position: object.source().position(),
        })
    }

    pub(crate) fn page_index(&mut self, id: ObjectId, source: Source) -> ReadResult<usize> {
        if self.pages.is_none() {
            let pages = self.document.pages()?;
            self.allocate::<(ObjectId, usize)>(pages.len(), source)?;
            self.allocate::<u8>(pages.len().saturating_mul(INDEX_ENTRY_OVERHEAD), source)?;
            let mut index = BTreeMap::new();
            for page in pages.iter().filter(|page| page.source_path().is_empty()) {
                index.entry(page.id()).or_insert(page.index());
            }
            self.pages = Some(index);
        }
        Ok(self
            .pages
            .as_ref()
            .and_then(|pages| pages.get(&id))
            .copied()
            .ok_or_else(|| {
                source.error(Error::missing_object(
                    "destination page is absent from the page tree",
                ))
            })?)
    }

    pub(crate) fn step(&mut self, depth: usize, source: Source) -> Result<()> {
        if depth > self.document.limits().depth {
            return Err(source.error(Error::limit("navigation traversal budget exceeded")));
        }
        self.work
            .borrow_mut()
            .steps
            .charge(1)
            .map_err(|error| source.error(error))
    }

    /// Charges caller-owned storage for `count` values of `T`.
    pub(crate) fn allocate<T>(&mut self, count: usize, source: Source) -> Result<()> {
        self.work
            .borrow_mut()
            .bytes
            .charge_items::<T>(count)
            .map_err(|error| source.error(error))
    }

    /// Charges one entry of a visited set or index (ADR 0012's estimate).
    pub(crate) fn index_entry(&mut self, source: Source) -> Result<()> {
        self.allocate::<u8>(INDEX_ENTRY_OVERHEAD, source)
    }

    /// Charges `count` source bytes read again outside object loading.
    pub(crate) fn scan(&self, count: usize, source: Source) -> Result<()> {
        self.work
            .borrow_mut()
            .scanned
            .charge(count)
            .map_err(|error| source.error(error))
    }

    pub(crate) fn resolve(
        &mut self,
        mut value: &'a Value,
        mut source: Source,
    ) -> ReadResult<(&'a Value, Source)> {
        static NULL: Value = Value::Null;
        let mut references = std::collections::BTreeSet::new();
        for depth in 0..=self.document.limits().depth {
            let Value::Reference(id) = value else {
                return Ok((value, source));
            };
            self.step(depth, source)?;
            self.index_entry(source)?;
            if !references.insert(*id) {
                return Err(
                    (source.error(Error::cycle("cyclic navigation reference chain"))).into(),
                );
            }
            if !self.document.slots.contains_key(id) {
                return Ok((&NULL, source));
            }
            let object = self
                .document
                .object(*id)
                .map_err(|error| source.fault(error))?;
            source = Source {
                object: Some(*id),
                position: object.source().position(),
            };
            value = object.value();
        }
        Err((source.error(Error::limit("navigation reference depth exceeded"))).into())
    }

    pub(crate) fn dictionary(
        &mut self,
        value: &'a Value,
        source: Source,
    ) -> ReadResult<(&'a Dictionary, Source)> {
        let (value, source) = self.resolve(value, source)?;
        Ok(value
            .as_dictionary()
            .map(|dictionary| (dictionary, source))
            .ok_or_else(|| source.error(Error::syntax("navigation object must be a dictionary")))?)
    }

    /// Preserve the original reference while applying dictionary-null absence
    /// (7.3.9), so subsequent traversal retains its source identity.
    pub(crate) fn present(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> ReadResult<Option<&'a Value>> {
        let Some(value) = dictionary.get(key) else {
            return Ok(None);
        };
        let (resolved, _) = self.resolve(value, source)?;
        Ok((!resolved.is_null()).then_some(value))
    }

    pub(crate) fn entry(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> ReadResult<Option<&'a Value>> {
        dictionary
            .get(key)
            .map(|value| self.resolve(value, source).map(|(value, _)| value))
            .transpose()
            .map(|value| value.filter(|value| !value.is_null()))
    }

    pub(crate) fn type_name(
        &mut self,
        dictionary: &'a Dictionary,
        expected: &[u8],
        source: Source,
    ) -> ReadResult<()> {
        if self
            .entry(dictionary, b"Type", source)?
            .is_some_and(|value| {
                value
                    .as_name()
                    .is_none_or(|name| name.as_bytes() != expected)
            })
        {
            return Err((source.error(Error::syntax("invalid navigation dictionary Type"))).into());
        }
        Ok(())
    }

    pub(crate) fn required(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> ReadResult<&'a Value> {
        Ok(self
            .entry(dictionary, key, source)?
            .ok_or_else(|| source.error(Error::syntax("required navigation entry is absent")))?)
    }

    pub(crate) fn repair(
        &mut self,
        tag: Repair,
        message: &'static str,
        source: Source,
    ) -> Result<()> {
        self.document
            .repair(source.diagnostic(Diagnostic::new(tag, message)))
    }

    /// Decodes a text string (7.9.2.2) through [`text_string::decode`], with
    /// its repairs located at `source` and its length charged as a result.
    pub(crate) fn text(&mut self, bytes: &[u8], source: Source) -> Result<String> {
        let mut diagnostics = Diagnostics::new(self.document.recovery());
        let text = text_string::decode(bytes, &self.document.limits(), &mut diagnostics)
            .map_err(|error| source.error(error))?;
        self.document.report(
            diagnostics
                .take()
                .into_iter()
                .map(|diagnostic| source.diagnostic(diagnostic)),
        );
        self.allocate::<u8>(text.len(), source)?;
        Ok(text)
    }
}

/// The identity of a value borrowed from the document: its address.
///
/// Sound because cached values are never moved or evicted while the document
/// lives: object and object-stream caches are write-once cells, and views
/// only borrow from them. Two equal identities are therefore the same source
/// value, and an identity is never reused for another one during a borrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ValueIdentity(usize);

impl ValueIdentity {
    pub(crate) fn of(value: &Value) -> Self {
        Self(std::ptr::from_ref(value).addr())
    }
}

pub(crate) fn identity(value: &Value) -> ValueIdentity {
    ValueIdentity::of(value)
}

pub(crate) fn number(value: &Value, source: Source) -> Result<f64> {
    value
        .as_number()
        .filter(|number| number.is_finite())
        .ok_or_else(|| source.error(Error::syntax("navigation coordinate must be finite")))
}
