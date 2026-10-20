//! Conservative interoperability for duplicated merged widget dictionaries.
//! Never infer identity from a field name, rectangle, or appearance alone.
use pagura::editing::{SaveOptions, Transaction};
use pagura::forms::AcroForm;
use pagura::{Document, Error, Name, Page, Result, Value};
use std::collections::BTreeMap;

// Independent host-side comparison budget, charged by retained source bytes.
const COMPARISON_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Matcher<'a> {
    form: &'a AcroForm<'a>,
    identities: BTreeMap<pagura::ObjectId, usize>,
    bytes: Vec<usize>,
    remaining: usize,
}
impl<'a> Matcher<'a> {
    pub(super) fn new(document: &'a Document, form: &'a AcroForm<'a>) -> Result<Self> {
        if form.widgets.len() > super::MAX_OPTIONS || form.fields.len() > super::MAX_FIELDS {
            return Err(Error::limit("browser form widget budget exceeded"));
        }
        let mut identities = BTreeMap::new();
        let mut bytes = Vec::new();
        for (index, widget) in form.widgets.iter().enumerate() {
            bytes.push(widget.dictionary.retained_bytes());
            if let Some(id) = widget.object
                && document
                    .object(id)?
                    .dictionary()
                    .is_some_and(|d| std::ptr::eq(d, widget.dictionary))
            {
                identities.insert(id, index);
            }
        }
        Ok(Self {
            form,
            identities,
            bytes,
            remaining: COMPARISON_BYTES,
        })
    }
    /// (Annotation-array index, canonical form-widget index).
    pub(super) fn page(&mut self, document: &Document, page: &Page) -> Result<Vec<(usize, usize)>> {
        let mut matches = Vec::new();
        for (offset, value) in annotations(document, page)?.iter().enumerate() {
            let Some(id) = value.as_reference() else {
                continue;
            };
            if let Some(&index) = self.identities.get(&id) {
                matches.push((offset, index));
                continue;
            }
            let dictionary = document.resolve_dictionary(value)?;
            if !dictionary.is_name(b"Subtype", b"Widget") {
                continue;
            }
            let size = dictionary.retained_bytes();
            let mut found = None;
            for &index in self.identities.values() {
                let widget = &self.form.widgets[index];
                // Only complete merged-field copies can be reconciled. Separate
                // widgets have Parent identities whose semantics must stay exact.
                if !widget.merged || dictionary.contains_key(b"Parent") {
                    continue;
                }
                self.remaining = self
                    .remaining
                    .checked_sub(size.max(self.bytes[index]))
                    .ok_or(Error::limit("duplicate widget comparison budget exceeded"))?;
                if dictionary == widget.dictionary {
                    if found.is_some() {
                        return Err(Error::invalid_argument("ambiguous duplicated form widget"));
                    }
                    found = Some(index);
                }
            }
            if let Some(index) = found {
                matches.push((offset, index));
            }
        }
        Ok(matches)
    }
}

pub(super) fn annotations<'a>(document: &'a Document, page: &'a Page) -> Result<&'a [Value]> {
    let Some(value) = page.dictionary().get(b"Annots") else {
        return Ok(&[]);
    };
    let values = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("Annots must be an array"))?;
    if values.len() > super::MAX_OPTIONS {
        return Err(Error::limit("browser page annotation budget exceeded"));
    }
    Ok(values)
}

/// Repoint exact duplicate page widgets to their canonical form-tree identity.
/// This appends a revision; original bytes and all unrelated annotations survive.
pub(super) fn normalize(document: &Document) -> Result<Option<Vec<u8>>> {
    let Some(form) = document.acro_form()? else {
        return Ok(None);
    };
    let mut matcher = Matcher::new(document, &form)?;
    let mut transaction = Transaction::new(document);
    for page in document.pages() {
        let source = annotations(document, page)?;
        let mut replacements = Vec::new();
        for (offset, index) in matcher.page(document, page)? {
            let id = form.widgets[index]
                .object
                .ok_or(Error::invalid_argument("widget has no identity"))?;
            if source[offset].as_reference() != Some(id) {
                replacements.push((offset, id));
            }
        }
        if !replacements.is_empty() {
            let mut values = source.to_vec();
            for (offset, id) in replacements {
                values[offset] = Value::Reference(id);
            }
            transaction.edit_dictionary(page.id(), |dictionary| {
                dictionary.insert(Name::new(b"Annots"), Value::Array(values));
                Ok(())
            })?;
        }
    }
    if transaction.is_empty() {
        Ok(None)
    } else {
        transaction.commit(&SaveOptions::default()).map(Some)
    }
}
