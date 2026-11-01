//! Page content sequences (7.8.2), including reported direct-stream recovery.

use crate::{Document, IndirectObject, Page, information};
use pagura_core::syntax::PathComponent;
use pagura_core::{Diagnostic, Error, Name, Result, Value};

pub(crate) fn read<'a>(document: &'a Document, page: &Page) -> Result<Vec<&'a IndirectObject>> {
    let Some(mut value) = page.dictionary().get(b"Contents") else {
        return Ok(Vec::new());
    };
    // A reference to an undefined object is null (7.3.10), and a null entry
    // is an absent one (7.3.9): the page has no content (Table 31). The
    // undefined reference is reported, so the empty page is never silent.
    if document.resolve(value)?.is_null() {
        if undefined(document, value) {
            document.report([Diagnostic::information(
                information::CONTENTS_UNDEFINED,
                "Contents refers to an undefined object; the page has no content",
            )
            .in_object(page.id())]);
        }
        return Ok(Vec::new());
    }
    let mut owner = document.object(page.id())?;
    let mut path = page.source_path().to_vec();
    path.push(PathComponent::Key(Name::new(b"Contents")));
    for _ in 0..=document.limits().depth {
        let Value::Reference(id) = value else { break };
        owner = document.object(*id)?;
        if owner.is_stream() {
            return Ok(vec![owner]);
        }
        value = owner.value();
        path.clear();
    }
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Dictionary(_) => Ok(vec![one(document, value, owner, &path)?]),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let before = path.len();
                path.push(PathComponent::Index(index));
                let result = one(document, value, owner, &path);
                path.truncate(before);
                result
            })
            .collect(),
        Value::Reference(_) => {
            Err(Error::limit("Contents reference chain too long").in_object(owner.id()))
        }
        _ => Err(Error::syntax("Contents must be a stream or array").in_object(page.id())),
    }
}

fn one<'a>(
    document: &'a Document,
    value: &Value,
    owner: &'a IndirectObject,
    path: &[PathComponent],
) -> Result<&'a IndirectObject> {
    if let Some(mut id) = value.as_reference() {
        for _ in 0..=document.limits().depth {
            let object = document.object(id)?;
            if object.is_stream() {
                return Ok(object);
            }
            let Some(next) = object.value().as_reference() else {
                return Err(Error::syntax("Contents entry is not a stream").in_object(id));
            };
            id = next;
        }
        return Err(Error::limit("Contents reference chain too long").in_object(id));
    }
    owner
        .direct_stream(path)
        .filter(|stream| stream.value() == value)
        .ok_or_else(|| Error::syntax("Contents entry is not a stream").in_object(owner.id()))
}

/// Whether `value`'s reference chain, which resolved to null, ends at an
/// undefined object rather than at an explicit null.
fn undefined<'a>(document: &'a Document, mut value: &'a Value) -> bool {
    for _ in 0..=document.limits().depth {
        let Value::Reference(id) = value else {
            return false;
        };
        let Ok(object) = document.object(*id) else {
            return !document.slots.contains_key(id);
        };
        value = object.value();
    }
    false
}
