//! Catalog identification and its reported missing-Type repair (7.7.2).

use crate::{Document, ObjectSource, tags};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Position, Result, Value};

/// Recognize only an explicit Catalog or a missing Type with a valid Pages node.
pub(crate) fn candidate(document: &Document, dictionary: &Dictionary) -> Result<bool> {
    if dictionary.is_name(b"Type", b"Catalog") {
        return Ok(true);
    }
    if dictionary.contains_key(b"Type") {
        return Ok(false);
    }
    let Some(pages @ Value::Reference(_)) = dictionary.get(b"Pages") else {
        return Ok(false);
    };
    Ok(document
        .resolve(pages)?
        .as_dictionary()
        .is_some_and(|pages| pages.is_name(b"Type", b"Pages")))
}

pub(crate) fn validate(document: &Document) -> Result<()> {
    let dictionary = document.catalog()?;
    if dictionary.is_name(b"Type", b"Catalog") {
        return Ok(());
    }
    if !candidate(document, dictionary)? {
        return Err(Error::syntax("document Root is not a Catalog"));
    }
    let mut diagnostic = Diagnostic::new(
        tags::CATALOG_TYPE,
        "catalog Type is missing; validated Pages reference identifies the catalog",
    );
    if let Some(id) = document
        .trailer()
        .get(b"Root")
        .and_then(Value::as_reference)
    {
        diagnostic = diagnostic.in_object(id);
        if let ObjectSource::File(range) = document.object(id)?.source() {
            diagnostic = diagnostic.at(Position::File(range.start));
        }
    }
    let mut diagnostics = Diagnostics::new(document.recovery());
    diagnostics.repair(diagnostic)?;
    document.record(diagnostics.take());
    Ok(())
}
