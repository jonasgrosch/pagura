//! Source admission uses independently positioned PDF literals, not an engine writer.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Result, Value};
use pagura_document::source::{
    ByteRange, ByteSource, LoadingLimits, ReadFault, SourceIdentity, SourceLimits, SparseBytes,
};
use pagura_document::{OpenJob, OpenOptions, Opening};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(810, [17; 32]);
const LENGTH: usize = 1024;

fn literal() -> Vec<u8> {
    let mut bytes = vec![b' '; LENGTH];
    for (offset, part) in [
        (0, &b"%PDF-1.7\n"[..]),
        (100, &b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n"[..]),
        (200, &b"2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 2 2] >> endobj\n"[..]),
        (300, &b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n"[..]),
        (400, &b"4 0 obj << /Length 6 >> stream\nhello\n\nendstream\nendobj\n"[..]),
        (700, &b"xref\n0 5\n0 65535 f\n100 0 n\n200 0 n\n300 0 n\n400 0 n\ntrailer << /Size 5 /Root 1 0 R >>\n"[..]),
        (960, &b"startxref\n700\n%%EOF\n"[..]),
    ] {
        bytes[offset..offset + part.len()].copy_from_slice(part);
    }
    bytes
}

fn source() -> Result<Arc<SparseBytes>> {
    Ok(Arc::new(SparseBytes::new(
        ID,
        1024,
        &Limits::default(),
        &SourceLimits::default(),
    )?))
}

fn job(source: Arc<SparseBytes>) -> Result<OpenJob> {
    OpenJob::new(
        source,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
        LoadingLimits::default(),
    )
}

#[test]
fn complete_admission_waits_once_per_generation_then_delivers_the_same_document_model() {
    let source = source().unwrap();
    let mut job = job(source.clone()).unwrap();
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("initial wait")
    };
    assert_eq!(missing.identity(), ID);
    assert_eq!(missing.ranges(), &[ByteRange::new(0, 1024).unwrap()]);
    assert_eq!(job.work(), 1);
    let bytes = literal();
    source.supply(ID, 512, &bytes[512..]).unwrap();
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("prefix wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(0, 512).unwrap()]);
    let Err(ReadFault::Unavailable(again)) = job.advance() else {
        panic!("unchanged wait")
    };
    assert!(std::ptr::eq(missing.ranges(), again.ranges()));
    assert_eq!(job.work(), 2);
    source.supply(ID, 0, &bytes[..512]).unwrap();
    let Opening::Ready(document) = job.advance().unwrap() else {
        panic!("admitted document")
    };
    assert_eq!(job.work(), LENGTH + 3);
    assert_eq!(document.original_bytes(), bytes);
    assert_eq!(document.object_count(), 4);
    assert_eq!(document.page_count().unwrap(), 1);
    assert_eq!(document.page(0).unwrap().id(), ObjectId::new(3, 0));
    let stream = document.object(ObjectId::new(4, 0)).unwrap();
    assert_eq!(document.raw_stream_data(stream).unwrap(), b"hello\n");
    assert_eq!(document.diagnostics(), []);
    assert!(matches!(job.advance().unwrap(), Opening::Complete));
    assert!(!job.cancel());
}

#[test]
fn source_terminal_state_precedes_cached_objects_pages_streams_and_reauthentication() {
    for conflict in [false, true] {
        let source = source().unwrap();
        let bytes = literal();
        source.supply(ID, 0, &bytes).unwrap();
        let Opening::Ready(document) = job(source.clone()).unwrap().advance().unwrap() else {
            panic!("ready")
        };
        let root = document.object(ObjectId::new(1, 0)).unwrap();
        let page = document.page(0).unwrap();
        let stream = document.object(ObjectId::new(4, 0)).unwrap();
        let authenticated = document.reauthenticate(&[]).unwrap();
        assert_eq!(
            authenticated.original_bytes().as_ptr(),
            document.original_bytes().as_ptr()
        );
        let authenticated_page = authenticated.page(0).unwrap();
        assert_eq!(document.raw_stream_data(stream).unwrap(), b"hello\n");
        if conflict {
            assert!(source.supply(ID, 100, b"2").is_err());
        } else {
            source.close();
        }
        let expected = source.check().unwrap_err();
        assert_eq!(document.object(root.id()).unwrap_err(), expected);
        assert_eq!(document.pages().unwrap_err(), expected);
        assert_eq!(document.resolve(root.value()).unwrap_err(), expected);
        assert_eq!(
            document
                .resolve(&Value::Reference(ObjectId::new(99, 0)))
                .unwrap_err(),
            expected
        );
        assert_eq!(document.raw_stream_data(stream).unwrap_err(), expected);
        assert_eq!(document.content_streams(page).unwrap_err(), expected);
        assert_eq!(document.reauthenticate(&[]).unwrap_err(), expected);
        assert_eq!(
            authenticated.page(authenticated_page.index()).unwrap_err(),
            expected
        );
        // The admitted immutable allocation survives source close and remains
        // charged while either document owns it.
        assert_eq!(document.original_bytes(), bytes);
        source.close();
        assert!(source.retained_bytes() >= LENGTH);
        drop(document);
        assert!(source.retained_bytes() >= LENGTH);
        drop(authenticated);
        assert_eq!(source.retained_bytes(), 0);
    }
}

#[test]
fn cancellation_and_budget_failure_cannot_become_success_after_later_supply() {
    let source = source().unwrap();
    let mut cancelled = job(source.clone()).unwrap();
    assert!(matches!(
        cancelled.advance(),
        Err(ReadFault::Unavailable(_))
    ));
    assert!(source.retained_bytes() > 0);
    assert!(cancelled.cancel());
    assert_eq!(source.retained_bytes(), 0);
    assert!(!cancelled.cancel());
    source.supply(ID, 0, &literal()).unwrap();
    assert!(matches!(cancelled.advance().unwrap(), Opening::Cancelled));

    let mut bounded = OpenJob::new(
        source,
        &OpenOptions::default(),
        LoadingLimits {
            work: LENGTH,
            ..LoadingLimits::default()
        },
    )
    .unwrap();
    let Err(ReadFault::Invalid(error)) = bounded.advance() else {
        panic!("work admission")
    };
    assert_eq!(error.kind, ErrorKind::Limit);
    let work = bounded.work();
    let Err(ReadFault::Invalid(again)) = bounded.advance() else {
        panic!("terminal failure")
    };
    assert_eq!(again, error);
    assert_eq!(bounded.work(), work);
    assert!(!bounded.cancel());
}

#[test]
fn closure_during_an_unchanged_wait_is_terminal() {
    let source = source().unwrap();
    let mut job = job(source.clone()).unwrap();
    assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
    let work = job.work();
    source.close();
    let Err(ReadFault::Invalid(error)) = job.advance() else {
        panic!("closed wait")
    };
    assert_eq!(error, source.check().unwrap_err());
    assert_eq!(job.work(), work);
    assert_eq!(source.retained_bytes(), 0);
    let Err(ReadFault::Invalid(again)) = job.advance() else {
        panic!("terminal close")
    };
    assert_eq!(again, error);
}

#[test]
fn historical_cache_preserves_registered_source_lifecycle() {
    let bytes = pagura_testkit::fixture("signature-changes", "form-fill-p2.pdf");
    let source = Arc::new(
        SparseBytes::new(
            ID,
            u64::try_from(bytes.len()).unwrap(),
            &Limits::default(),
            &SourceLimits::default(),
        )
        .unwrap(),
    );
    source.supply(ID, 0, &bytes).unwrap();
    let Opening::Ready(document) = job(source.clone()).unwrap().advance().unwrap() else {
        panic!("historical source")
    };
    let id = ObjectId::new(7, 0);
    let old = document.object_at_revision(1, id).unwrap();
    assert_eq!(
        old.dictionary().unwrap().get(b"V").unwrap().as_string(),
        Some(b"old".as_slice())
    );
    assert_eq!(
        document
            .object(id)
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"V")
            .unwrap()
            .as_string(),
        Some(b"new".as_slice())
    );
    assert!(std::ptr::eq(
        old,
        document.object_at_revision(1, id).unwrap()
    ));
    source.close();
    assert_eq!(
        document.object_at_revision(1, id).unwrap_err(),
        source.check().unwrap_err()
    );
    assert!(source.retained_bytes() >= bytes.len());
    drop(document);
    assert_eq!(source.retained_bytes(), 0);
}
