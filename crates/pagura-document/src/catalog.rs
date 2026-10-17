//! Catalog identification and reported Type inference (7.7.2).

use crate::{Document, ObjectSource, tags};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Position, Result, Value};

/// Types of other standard dictionaries. A Root carrying one of them is a
/// different object, never a catalog with a damaged Type.
const CONFLICTING_TYPES: [&[u8]; 11] = [
    b"Page",
    b"Pages",
    b"ObjStm",
    b"XRef",
    b"Font",
    b"XObject",
    b"Annot",
    b"Metadata",
    b"Action",
    b"Filespec",
    b"Outlines",
];

/// Infer a missing/unknown Type through Pages; conflicting known types fail.
pub(crate) fn candidate(document: &Document, dictionary: &Dictionary) -> Result<bool> {
    if dictionary.is_name(b"Type", b"Catalog") {
        return Ok(true);
    }
    if dictionary.get(b"Type").is_some_and(|value| {
        value
            .as_name()
            .is_none_or(|name| CONFLICTING_TYPES.contains(&name.as_bytes()))
    }) {
        return Ok(false);
    }
    let Some(pages @ Value::Reference(_)) = dictionary.get(b"Pages") else {
        return Ok(false);
    };
    document
        .resolve(pages)?
        .as_dictionary()
        .map_or(Ok(false), |pages| crate::pages::branch(document, pages))
}

pub(crate) fn validate(document: &Document) -> Result<()> {
    let dictionary = document.catalog()?;
    if dictionary.is_name(b"Type", b"Catalog") {
        return Ok(());
    }
    if !candidate(document, dictionary)? {
        return Err(Error::syntax("document Root is not a Catalog"));
    }
    let mut diagnostic = if dictionary.contains_key(b"Type") {
        Diagnostic::new(
            tags::CATALOG_TYPE_INVALID,
            "catalog Type is unknown; validated Pages reference identifies the catalog",
        )
    } else {
        Diagnostic::new(
            tags::CATALOG_TYPE,
            "catalog Type is missing; validated Pages reference identifies the catalog",
        )
    };
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

#[cfg(test)]
mod tests {
    use crate::{Document, OpenOptions, tags};
    use pagura_core::{ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn inferred_types_are_reported_and_source_dictionaries_are_preserved() {
        for (name, tag) in [
            ("catalog-type-invalid", tags::CATALOG_TYPE_INVALID),
            ("pages-type-missing", tags::CATALOG_TYPE),
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert_eq!(strict.repair, Some(tag));
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            assert_eq!(document.original_bytes(), bytes);
            if name == "pages-type-missing" {
                for number in 1..=3 {
                    assert!(
                        !document
                            .object(ObjectId::new(number, 0))
                            .unwrap()
                            .dictionary()
                            .unwrap()
                            .contains_key(b"Type")
                    );
                }
                assert_eq!(
                    document
                        .diagnostics()
                        .iter()
                        .filter(|item| item.repair == tags::PAGE_TYPE)
                        .count(),
                    2
                );
            } else {
                assert!(
                    document
                        .catalog()
                        .unwrap()
                        .is_name(b"Type", b"Unrecognized")
                );
            }
        }
        for name in ["catalog-type-wrong", "catalog-pages-wrong-type"] {
            assert!(
                Document::open(
                    fixture("document", &format!("{name}.pdf")),
                    &OpenOptions::default()
                )
                .is_err()
            );
        }
    }
}
