//! View reports commit independently from completed object-cache diagnostics.

use pagura_core::{
    Diagnostic, Dictionary, Error, ErrorKind, Limits, ObjectId, Position, Recovery, Value,
};
use pagura_document::source::{
    ByteRange, ByteSource, LoadingLimits, ReadFault, ReadResult, SourceIdentity, SourceLimits,
    SparseBytes,
};
use pagura_document::{Document, OpenJob, OpenOptions, Opening, Validation};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(823, [29; 32]);
const INFO: &[u8] = b"5 0 obj << /Title <FEFF0041FF> /Author 6 0 R A 7 >> endobj\n";

fn document(info: &[u8], recovery: Recovery) -> ReadResult<(Arc<SparseBytes>, Arc<Document>)> {
    let source = Arc::new(SparseBytes::new(
        ID,
        8192,
        &Limits::default(),
        &SourceLimits::default(),
    )?);
    for (offset, bytes) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, b"1 0 obj << /Type /Catalog >> endobj\n".as_slice()),
        (4000, info),
        (6500, b"xref\n0 2\n0 65535 f\n100 0 n\n5 2\n4000 0 n\n6000 0 n\ntrailer << /Size 7 /Root 1 0 R /Info 5 0 R >>\n".as_slice()),
    ] { source.supply(ID, offset, bytes)?; }
    let mut tail = vec![b' '; 1024];
    tail[932..953].copy_from_slice(b"startxref\n6500\n%%EOF\n");
    source.supply(ID, 7168, &tail)?;
    let mut job = OpenJob::new(
        source.clone(),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
        LoadingLimits::default(),
        Validation::RequestedOperation,
    )?;
    for _ in 0..32 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok((source, document)),
            _ => {
                return Err(Error::invalid_argument("unexpected view fixture opening state").into());
            }
        }
    }
    Err(Error::limit("view fixture opening step bound").into())
}

#[test]
fn text_repairs_wait_without_discarding_completed_object_reports() {
    for author in [
        b"6 0 obj (B) endobj\n".as_slice(),
        b"6 0 obj 17 endobj\n".as_slice(),
    ] {
        let (source, document) = document(INFO, Recovery::Repair).unwrap();
        let dependency = Diagnostic::new(
            pagura_core::repairs::KEY_SOLIDUS,
            "dictionary key has no leading solidus",
        )
        .at(Position::File(4045))
        .in_object(ObjectId::new(5, 0));
        let provisional = Diagnostic::new(
            pagura_core::repairs::INVALID_TEXT,
            "invalid text string sequence replaced (7.9.2.2, Annex D.3)",
        )
        .at(Position::File(4000))
        .in_object(ObjectId::new(5, 0));
        for _ in 0..3 {
            let Err(ReadFault::Unavailable(missing)) = document.information() else {
                panic!("Author dependency wait")
            };
            assert_eq!(missing.ranges(), &[ByteRange::new(6000, 6256).unwrap()]);
            assert_eq!(document.diagnostics(), std::slice::from_ref(&dependency));
        }
        let cached = document.object(ObjectId::new(5, 0)).unwrap();
        source.supply(ID, 6000, author).unwrap();
        for _ in 0..3 {
            match document.information() {
                Ok(Some(info)) => {
                    assert_eq!(author, b"6 0 obj (B) endobj\n");
                    assert_eq!(info.title.as_deref(), Some("A\u{fffd}"));
                    assert_eq!(info.author.as_deref(), Some("B"));
                }
                Err(ReadFault::Invalid(error)) => {
                    assert_eq!(author, b"6 0 obj 17 endobj\n");
                    assert_eq!(error.kind, ErrorKind::Syntax);
                    assert_eq!(error.message, "metadata entry must be a text string");
                    assert_eq!(error.position, Position::File(6000));
                    assert_eq!(error.object, Some(ObjectId::new(6, 0)));
                }
                other => panic!("unexpected completed view {other:?}"),
            }
            assert!(std::ptr::eq(
                document.object(ObjectId::new(5, 0)).unwrap(),
                cached
            ));
            assert_eq!(
                document.diagnostics(),
                [dependency.clone(), provisional.clone()]
            );
        }
    }
}

#[test]
fn strict_text_failure_precedes_later_missing_bytes_and_closed_views_cannot_succeed() {
    let (source, document) = document(
        b"5 0 obj << /Title <FEFF0041FF> /Author 6 0 R >> endobj\n",
        Recovery::Strict,
    )
    .unwrap();
    let error = document.information().unwrap_err();
    assert_eq!(
        error.as_error().unwrap().repair,
        Some(pagura_core::repairs::INVALID_TEXT)
    );
    assert_eq!(error.as_error().unwrap().position, Position::File(4000));
    assert_eq!(document.diagnostics(), []);
    assert!(matches!(
        source.read_exact(ByteRange::new(6000, 6001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    source.close();
    let closed = ReadFault::Invalid(source.check().unwrap_err());
    assert_eq!(document.information().unwrap_err(), closed);
    assert_eq!(
        document.object_metadata(&Dictionary::new()).unwrap_err(),
        closed
    );
    assert_eq!(
        document
            .file_specification(&Value::String(b"inert.pdf".to_vec()))
            .unwrap_err(),
        closed
    );
    assert_eq!(document.diagnostics(), []);
}
