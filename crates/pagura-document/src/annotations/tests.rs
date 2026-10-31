use crate::{Document, OpenOptions};
use pagura_core::{Diagnostic, Error, ObjectId, Recovery, Result, Value};
use pagura_testkit::fixture;

fn open(name: &str, recovery: Recovery) -> Result<Document> {
    Document::open(
        fixture("recovery_annotation_references", &format!("{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
}

#[test]
fn exact_defined_null_members_are_reported_at_the_supplied_object() -> Result<()> {
    for (name, generation) in [("defined-zero", 0), ("defined-seven", 7)] {
        let document = open(name, Recovery::Repair)?;
        let id = ObjectId::new(9, generation);
        let original = Value::Reference(id);
        let source = document.object(id)?.source().position();
        for _ in 0..2 {
            assert!(
                document
                    .annotation_entry(&original, ObjectId::new(3, 0))?
                    .is_none()
            );
        }
        assert_eq!(document.object(id)?.value(), &Value::Null);
        assert_eq!(
            document.diagnostics(),
            [Diagnostic::new(
                crate::repairs::ANNOTATION_NULL_REFERENCE,
                "defined null object skipped in page Annots array (12.5.2)",
            )
            .in_object(id)
            .at(source)]
        );
        let links = document.page(0)?.links(&document)?;
        assert_eq!(links.len(), 1);
        assert!(std::ptr::eq(
            links[0].dictionary,
            document.object(ObjectId::new(6, 0))?.dictionary().unwrap()
        ));
    }
    Ok(())
}

#[test]
fn strict_keeps_the_complete_original_expected_dictionary_error() -> Result<()> {
    for (name, generation) in [("defined-zero", 0), ("defined-seven", 7)] {
        let document = open(name, Recovery::Strict)?;
        let id = ObjectId::new(9, generation);
        let original = Value::Reference(id);
        assert_eq!(
            document.annotation_entry(&original, ObjectId::new(3, 0))?,
            Some(&original)
        );
        assert_eq!(
            document.resolve_dictionary(&original).unwrap_err(),
            Error::syntax("expected a dictionary").in_object(id)
        );
        assert_eq!(document.diagnostics(), []);
    }
    Ok(())
}

#[test]
fn absent_generations_and_chains_never_become_recovery_evidence() -> Result<()> {
    for (name, id) in [
        ("wrong-generation", ObjectId::new(9, 1)),
        ("undefined", ObjectId::new(99, 0)),
        ("free", ObjectId::new(9, 1)),
        ("reference-chain", ObjectId::new(9, 0)),
        ("reference-cycle", ObjectId::new(9, 0)),
        ("integer", ObjectId::new(9, 0)),
        ("string", ObjectId::new(9, 0)),
        ("array", ObjectId::new(9, 0)),
        ("malformed-dictionary", ObjectId::new(9, 0)),
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = open(name, recovery)?;
            let original = Value::Reference(id);
            assert_eq!(
                document.annotation_entry(&original, ObjectId::new(3, 0))?,
                Some(&original),
                "{name}"
            );
            assert_eq!(document.diagnostics(), [], "{name}");
        }
    }
    Ok(())
}
