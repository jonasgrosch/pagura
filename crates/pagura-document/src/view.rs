//! Shared work accounting and source locations for navigation views.
pub(crate) mod text;

use crate::Document;
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, ObjectId, Position, Repair, Result, Value,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Source {
    pub object: Option<ObjectId>,
    pub position: Position,
}
impl Source {
    pub fn error(self, mut error: Error) -> Error {
        if error.position == Position::Unknown {
            error.position = self.position;
        }
        if error.object.is_none() {
            error.object = self.object;
        }
        error
    }
}

#[derive(Clone, Copy, Default)]
struct Work {
    steps: usize,
    bytes: usize,
    scanned: usize,
}
pub(crate) struct Context<'a> {
    pub doc: &'a Document,
    work: std::rc::Rc<std::cell::Cell<Work>>,
    limits: Limits,
    pages: Option<std::collections::BTreeMap<ObjectId, usize>>,
}
impl<'a> Context<'a> {
    pub fn new(doc: &'a Document) -> Self {
        Self {
            doc,
            work: std::rc::Rc::default(),
            limits: doc.limits(),
            pages: None,
        }
    }
    /// A different source revision using this traversal's aggregate work budget.
    pub fn for_document<'b>(&self, doc: &'b Document) -> Context<'b> {
        Context {
            doc,
            work: self.work.clone(),
            limits: self.limits,
            pages: None,
        }
    }
    pub fn catalog(&mut self) -> Result<(&'a Dictionary, Source)> {
        self.doc.catalog()?;
        let root = self
            .doc
            .trailer()
            .get(b"Root")
            .ok_or(Error::syntax("catalog Root is absent"))?;
        self.dictionary(root, Source::default())
    }
    pub fn source(&self, id: ObjectId) -> Result<Source> {
        let object = self.doc.object(id)?;
        Ok(Source {
            object: Some(id),
            position: object.source().position(),
        })
    }
    pub fn page_index(&mut self, id: ObjectId, source: Source) -> Result<usize> {
        if self.pages.is_none() {
            let pages = self.doc.pages()?;
            self.allocate::<(ObjectId, usize)>(pages.len().saturating_mul(4), source)?;
            let mut index = std::collections::BTreeMap::new();
            for page in pages.iter().filter(|page| page.source_path().is_empty()) {
                index.entry(page.id()).or_insert(page.index());
            }
            self.pages = Some(index);
        }
        self.pages
            .as_ref()
            .and_then(|pages| pages.get(&id))
            .copied()
            .ok_or_else(|| {
                source.error(Error::new(
                    pagura_core::ErrorKind::MissingObject,
                    "destination page is absent from the page tree",
                ))
            })
    }
    pub fn step(&mut self, depth: usize, source: Source) -> Result<()> {
        let limits = self.limits;
        let mut work = self.work.get();
        if depth > limits.depth || work.steps >= limits.value_nodes.min(limits.objects) {
            return Err(source.error(Error::limit("navigation traversal budget exceeded")));
        }
        work.steps += 1;
        self.work.set(work);
        Ok(())
    }
    pub fn allocate<T>(&mut self, count: usize, source: Source) -> Result<()> {
        let mut work = self.work.get();
        work.bytes = count
            .checked_mul(size_of::<T>())
            .and_then(|n| work.bytes.checked_add(n))
            .filter(|&n| n <= self.limits.document_bytes)
            .ok_or_else(|| source.error(Error::limit("navigation result budget exceeded")))?;
        self.work.set(work);
        Ok(())
    }
    pub fn scan(&self, count: usize, source: Source) -> Result<()> {
        let mut work = self.work.get();
        work.scanned = work
            .scanned
            .checked_add(count)
            .filter(|&n| n <= self.limits.file_bytes)
            .ok_or_else(|| source.error(Error::limit("source scan budget exceeded")))?;
        self.work.set(work);
        Ok(())
    }
    pub fn resolve(
        &mut self,
        mut value: &'a Value,
        mut source: Source,
    ) -> Result<(&'a Value, Source)> {
        static NULL: Value = Value::Null;
        let mut references = std::collections::BTreeSet::new();
        for depth in 0..=self.doc.limits().depth {
            if let Value::Reference(id) = value {
                self.step(depth, source)?;
                self.allocate::<usize>(8, source)?;
                if !references.insert(*id) {
                    return Err(source.error(Error::cycle("cyclic navigation reference chain")));
                }
                if !self.doc.slots.contains_key(id) {
                    return Ok((&NULL, source));
                }
                let object = self.doc.object(*id).map_err(|e| source.error(e))?;
                source.object = Some(*id);
                source.position = object.source().position();
                value = object.value();
            } else {
                return Ok((value, source));
            }
        }
        Err(source.error(Error::limit("navigation reference depth exceeded")))
    }
    pub fn dictionary(
        &mut self,
        value: &'a Value,
        source: Source,
    ) -> Result<(&'a Dictionary, Source)> {
        let (value, source) = self.resolve(value, source)?;
        value
            .as_dictionary()
            .map(|d| (d, source))
            .ok_or_else(|| source.error(Error::syntax("navigation object must be a dictionary")))
    }
    /// Preserve the original reference while applying dictionary-null absence
    /// (7.3.9), so subsequent traversal retains its source identity.
    pub fn present(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> Result<Option<&'a Value>> {
        let Some(value) = dictionary.get(key) else {
            return Ok(None);
        };
        let (resolved, _) = self.resolve(value, source)?;
        Ok((!resolved.is_null()).then_some(value))
    }
    pub fn entry(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> Result<Option<&'a Value>> {
        dictionary
            .get(key)
            .map(|v| self.resolve(v, source).map(|(v, _)| v))
            .transpose()
            .map(|v| v.filter(|v| !v.is_null()))
    }
    pub fn type_name(
        &mut self,
        dictionary: &'a Dictionary,
        expected: &[u8],
        source: Source,
    ) -> Result<()> {
        if self
            .entry(dictionary, b"Type", source)?
            .is_some_and(|v| v.as_name().is_none_or(|n| n.as_bytes() != expected))
        {
            return Err(source.error(Error::syntax("invalid navigation dictionary Type")));
        }
        Ok(())
    }
    pub fn required(
        &mut self,
        dictionary: &'a Dictionary,
        key: &[u8],
        source: Source,
    ) -> Result<&'a Value> {
        self.entry(dictionary, key, source)?
            .ok_or_else(|| source.error(Error::syntax("required navigation entry is absent")))
    }
    pub fn repair(&self, tag: Repair, message: &'static str, source: Source) -> Result<()> {
        let mut diagnostics = Diagnostics::new(self.doc.recovery());
        let mut diagnostic = Diagnostic::new(tag, message).at(source.position);
        diagnostic.object = source.object;
        diagnostics.repair(diagnostic)?;
        self.doc.report(diagnostics.take());
        Ok(())
    }
}
pub(crate) fn identity(value: &Value) -> usize {
    std::ptr::from_ref(value).addr()
}
pub(crate) fn number(value: &Value, source: Source) -> Result<f64> {
    value
        .as_number()
        .filter(|n| n.is_finite())
        .ok_or_else(|| source.error(Error::syntax("navigation coordinate must be finite")))
}
