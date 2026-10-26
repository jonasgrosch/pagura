//! Source admission uses independently positioned PDF literals, not an engine writer.

use pagura_core::{Error, ErrorKind, Limits, ObjectId, Recovery, Result, Value};
use pagura_document::source::{
    ByteRange, ByteSource, LoadingLimits, ReadFault, ReadResult, SourceIdentity, SourceLimits,
    SparseBytes,
};
use pagura_document::{Document, OpenJob, OpenOptions, Opening};
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

fn ready(job: &mut OpenJob) -> ReadResult<Arc<Document>> {
    for _ in 0..32 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok(document),
            _ => return Err(Error::invalid_argument("unexpected terminal opening state").into()),
        }
    }
    Err(Error::limit("test opening step bound exceeded").into())
}

#[test]
fn explicit_materialization_reuses_caller_bytes_and_outlives_the_document() {
    let bytes: Arc<[u8]> = literal().into();
    let document = Document::open(Arc::clone(&bytes), &OpenOptions::default()).unwrap();
    let lease = document.materialize().unwrap();
    assert_eq!(lease.range(), ByteRange::new(0, 1024).unwrap());
    assert_eq!(lease.bytes().as_ptr(), bytes.as_ptr());
    assert_eq!(
        document.materialize().unwrap().bytes().as_ptr(),
        lease.bytes().as_ptr()
    );
    drop(document);
    assert_eq!(lease.bytes(), bytes.as_ref());
}

#[test]
fn complete_admission_waits_once_per_generation_then_delivers_the_same_document_model() {
    let source = source().unwrap();
    let mut job = job(source.clone()).unwrap();
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("initial wait")
    };
    assert_eq!(missing.identity(), ID);
    assert_eq!(missing.ranges(), &[ByteRange::new(0, 256).unwrap()]);
    assert_eq!(job.work(), 1);
    let bytes = literal();
    source.supply(ID, 512, &bytes[512..]).unwrap();
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("prefix wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(0, 256).unwrap()]);
    let Err(ReadFault::Unavailable(again)) = job.advance() else {
        panic!("unchanged wait")
    };
    assert!(std::ptr::eq(missing.ranges(), again.ranges()));
    assert_eq!(job.work(), 2);
    source.supply(ID, 0, &bytes[..512]).unwrap();
    // Header: 256 bytes; tail: 256, 512, 1024; xref: 256; final
    // materialization: 1024. Catalog identity and value each read 256 bytes.
    // Each read costs one, including the two waits; document reads share work.
    for expected_work in [259, 2054, 2311] {
        assert!(matches!(job.advance().unwrap(), Opening::Yielded));
        assert_eq!(job.work(), expected_work);
    }
    let Opening::Ready(document) = job.advance().unwrap() else {
        panic!("admitted document")
    };
    assert_eq!(job.work(), 3336 + 2 * (1 + 256));
    assert_eq!(document.materialize().unwrap().bytes(), bytes);
    assert_eq!(document.object_count(), 4);
    assert_eq!(document.page_count().unwrap(), 1);
    assert_eq!(document.page(0).unwrap().id(), ObjectId::new(3, 0));
    let stream = document.object(ObjectId::new(4, 0)).unwrap();
    assert_eq!(
        document.raw_stream_data(stream).unwrap().bytes(),
        b"hello\n"
    );
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
        let document = ready(&mut job(source.clone()).unwrap()).unwrap();
        let root = document.object(ObjectId::new(1, 0)).unwrap();
        let page = document.page(0).unwrap();
        let stream = document.object(ObjectId::new(4, 0)).unwrap();
        let authenticated = document.reauthenticate(&[]).unwrap();
        let lease = document.materialize().unwrap();
        assert_eq!(
            authenticated.materialize().unwrap().bytes().as_ptr(),
            lease.bytes().as_ptr()
        );
        let authenticated_page = authenticated.page(0).unwrap();
        assert_eq!(
            document.raw_stream_data(stream).unwrap().bytes(),
            b"hello\n"
        );
        if conflict {
            assert!(source.supply(ID, 100, b"2").is_err());
        } else {
            source.close();
        }
        assert_eq!(document.check_source(), source.check());
        assert_eq!(document.check_page(page), source.check());
        let expected = ReadFault::Invalid(source.check().unwrap_err());
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
        assert!(matches!(document.materialize(), Err(error) if error == expected));
        // An issued immutable lease survives close; a fresh materialization
        // still checks the provider before returning shared backing.
        assert_eq!(lease.bytes(), bytes);
        source.close();
        assert!(source.retained_bytes() >= LENGTH);
        drop(document);
        assert!(source.retained_bytes() >= LENGTH);
        drop(authenticated);
        assert!(source.retained_bytes() >= LENGTH);
        drop(lease);
        assert_eq!(source.retained_bytes(), 0);
    }
}

#[test]
fn stream_values_keep_source_or_decoded_storage_after_document_and_source_close() {
    for encoded in [false, true] {
        let mut bytes = literal();
        if encoded {
            bytes[400..500].fill(b' ');
            let object = b"4 0 obj << /Length 13 /Filter /ASCIIHexDecode >> stream\n68656C6C6F0A>\nendstream\nendobj\n";
            bytes[400..400 + object.len()].copy_from_slice(object);
        }
        let source = source().unwrap();
        source.supply(ID, 0, &bytes).unwrap();
        let document = ready(&mut job(source.clone()).unwrap()).unwrap();
        let object = document.object(ObjectId::new(4, 0)).unwrap();
        let raw = document.raw_stream_data(object).unwrap();
        let data = document.stream_data(object).unwrap();
        assert_eq!(data.as_ref(), b"hello\n");
        if encoded {
            assert_eq!(raw.bytes(), b"68656C6C6F0A>");
        } else {
            assert_eq!(raw.range(), ByteRange::new(431, 437).unwrap());
            assert_eq!(raw.bytes().as_ptr(), data.as_ptr());
        }
        source.close();
        assert_eq!(
            document.stream_data(object).unwrap_err(),
            ReadFault::Invalid(source.check().unwrap_err())
        );
        drop(document);
        assert!(source.retained_bytes() >= LENGTH);
        assert_eq!(data.as_ref(), b"hello\n");
        drop(raw);
        if encoded {
            assert_eq!(source.retained_bytes(), 0);
        } else {
            assert!(source.retained_bytes() >= LENGTH);
        }
        drop(data);
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
    let Err(ReadFault::Invalid(error)) = ready(&mut bounded) else {
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
    let document = ready(&mut job(source.clone()).unwrap()).unwrap();
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
        ReadFault::Invalid(source.check().unwrap_err())
    );
    assert!(source.retained_bytes() >= bytes.len());
    drop(document);
    assert_eq!(source.retained_bytes(), 0);
}

#[test]
fn bootstrap_retains_completed_header_tail_and_index_before_complete_admission() {
    let mut bytes = literal();
    bytes[960..].fill(b' ');
    bytes.resize(4096, b' ');
    bytes[4000..4020].copy_from_slice(b"startxref\n700\n%%EOF\n");
    let source =
        Arc::new(SparseBytes::new(ID, 4096, &Limits::default(), &SourceLimits::default()).unwrap());
    source.supply(ID, 0, &bytes[..9]).unwrap();
    source.supply(ID, 3072, &bytes[3072..]).unwrap();
    let mut job = job(source.clone()).unwrap();
    assert!(matches!(job.advance().unwrap(), Opening::Yielded));
    assert!(matches!(job.advance().unwrap(), Opening::Yielded));
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("xref section wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(700, 956).unwrap()]);
    let work = job.work();
    assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
    assert_eq!(job.work(), work);
    source.supply(ID, 700, &bytes[700..900]).unwrap();
    assert!(matches!(job.advance().unwrap(), Opening::Yielded));
    let Err(ReadFault::Unavailable(missing)) = job.advance() else {
        panic!("explicit complete-document admission")
    };
    assert_eq!(
        missing.ranges(),
        &[
            ByteRange::new(9, 700).unwrap(),
            ByteRange::new(900, 3072).unwrap()
        ]
    );
    let work = job.work();
    for range in missing.ranges() {
        source
            .supply(
                ID,
                range.start(),
                &bytes[usize::try_from(range.start()).unwrap()
                    ..usize::try_from(range.end()).unwrap()],
            )
            .unwrap();
    }
    let Opening::Ready(document) = job.advance().unwrap() else {
        panic!("retained index completes without replaying bootstrap")
    };
    // Materialization plus Catalog identity/value windows, without bootstrap replay.
    assert_eq!(job.work(), work + 1 + 4096 + 2 * (1 + 256));
    assert_eq!(document.page_count().unwrap(), 1);
    assert_eq!(document.materialize().unwrap().bytes(), bytes);
    assert_eq!(document.diagnostics(), []);
}

#[test]
fn complete_and_windowed_opening_preserve_frozen_repair_and_revision_controls() {
    for name in [
        "xref-stream",
        "hybrid-precedence",
        "header-offset",
        "header-physical-offsets",
        "prev-cycle",
        "stream-prev-cycle",
        "rebuild-malformed-table",
        "rebuild-stream-length",
        "rebuild-missing-startxref",
        "rebuild-truncated-trailer",
        "rebuild-catalog-offset",
    ] {
        let bytes = pagura_testkit::fixture("document", &format!("{name}.pdf"));
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let options = OpenOptions {
                recovery,
                ..OpenOptions::default()
            };
            let complete = Document::open(bytes.clone(), &options);
            let source = Arc::new(
                SparseBytes::new(
                    ID,
                    u64::try_from(bytes.len()).unwrap(),
                    &options.limits,
                    &SourceLimits::default(),
                )
                .unwrap(),
            );
            source.supply(ID, 0, &bytes).unwrap();
            let ranged =
                ready(&mut OpenJob::new(source, &options, LoadingLimits::default()).unwrap());
            match (complete, ranged) {
                (Ok(complete), Ok(ranged)) => {
                    assert_eq!(
                        ranged.revisions(),
                        complete.revisions(),
                        "{name} {recovery:?}"
                    );
                    assert_eq!(
                        ranged.diagnostics(),
                        complete.diagnostics(),
                        "{name} {recovery:?}"
                    );
                    assert_eq!(
                        ranged.object_count(),
                        complete.object_count(),
                        "{name} {recovery:?}"
                    );
                    assert_eq!(
                        ranged.page_count(),
                        complete.page_count(),
                        "{name} {recovery:?}"
                    );
                    for id in complete.object_ids() {
                        assert_eq!(
                            ranged.object(id),
                            complete.object(id),
                            "{name} {recovery:?} {id:?}"
                        );
                    }
                }
                (Err(complete), Err(ReadFault::Invalid(ranged))) => {
                    assert_eq!(ranged, complete, "{name} {recovery:?}");
                }
                other => panic!("opening differs for {name} {recovery:?}: {other:?}"),
            }
        }
    }
}
