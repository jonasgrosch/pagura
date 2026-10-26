//! Independently positioned bytes exercise the public sparse opening boundary.

mod page_jobs;

use pagura_core::{Error, ErrorKind, Limits, ObjectId, Recovery, Result, Value};
use pagura_document::source::{
    ByteRange, ByteSource, LoadingLimits, ReadFault, ReadResult, SourceIdentity, SourceLimits,
    SparseBytes,
};
use pagura_document::{Document, OpenJob, OpenOptions, Opening, Validation};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(817, [25; 32]);
const ROOT: &[u8] = b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n";
const PAGES: &[u8] =
    b"2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 8 8] >> endobj\n";
const PAGE: &[u8] = b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n";
const CONTENT: &[u8] = b"4 0 obj << /Length 6 >> stream\nhello\n\nendstream\nendobj\n";
const INDEX: &[u8] = b"xref\n0 6\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n1500 0 n\n4000 0 n\ntrailer << /Size 6 /Root 1 0 R >>\n";

fn literal() -> Vec<u8> {
    let mut bytes = vec![b' '; 8192];
    for (offset, part) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, ROOT),
        (600, PAGES),
        (900, PAGE),
        (1500, CONTENT),
        (4000, b"5 0 obj null endobj\n".as_slice()),
        (6500, INDEX),
        (8100, b"startxref\n6500\n%%EOF\n".as_slice()),
    ] {
        bytes[offset..offset + part.len()].copy_from_slice(part);
    }
    bytes
}

fn source(bytes: &[u8]) -> Result<Arc<SparseBytes>> {
    let source = Arc::new(SparseBytes::new(
        ID,
        8192,
        &Limits::default(),
        &SourceLimits::default(),
    )?);
    source.supply(ID, 0, &bytes[..9])?;
    source.supply(ID, 6500, &bytes[6500..6500 + INDEX.len()])?;
    source.supply(ID, 7168, &bytes[7168..])?;
    Ok(source)
}

fn opening(
    source: Arc<SparseBytes>,
    validation: Validation,
    recovery: Recovery,
) -> Result<OpenJob> {
    OpenJob::new(
        source,
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
        LoadingLimits::default(),
        validation,
    )
}

fn ready(job: &mut OpenJob) -> ReadResult<Arc<Document>> {
    for _ in 0..32 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok(document),
            _ => return Err(Error::invalid_argument("unexpected opening state").into()),
        }
    }
    Err(Error::limit("test opening step bound exceeded").into())
}

#[test]
fn requested_opening_and_reauthentication_leave_unrelated_bytes_absent() {
    let bytes = literal();
    let source = source(&bytes).unwrap();
    let mut job = opening(
        source.clone(),
        Validation::RequestedOperation,
        Recovery::Strict,
    )
    .unwrap();
    let Err(ReadFault::Unavailable(missing)) = ready(&mut job) else {
        panic!("catalog wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(100, 356).unwrap()]);
    let work = job.work();
    for _ in 0..3 {
        assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
        assert_eq!(job.work(), work);
    }
    source.supply(ID, 100, ROOT).unwrap();
    let document = ready(&mut job).unwrap();
    assert_eq!(document.validation(), Validation::RequestedOperation);
    assert_eq!(document.object_count(), 5);
    assert_eq!(
        document
            .resolve(&Value::Reference(ObjectId::new(99, 0)))
            .unwrap(),
        &Value::Null
    );
    assert!(matches!(
        document.materialize(),
        Err(ReadFault::Unavailable(_))
    ));
    let authenticated = document.reauthenticate(&[]).unwrap();
    assert_eq!(authenticated.validation(), Validation::RequestedOperation);
    assert!(matches!(
        source.read_exact(ByteRange::new(2000, 2001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    let Err(ReadFault::Unavailable(missing)) = document.page_count() else {
        panic!("Pages wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(600, 856).unwrap()]);
    source.supply(ID, 600, PAGES).unwrap();
    let Err(ReadFault::Unavailable(missing)) = document.page(0) else {
        panic!("Page wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(900, 1156).unwrap()]);
    source.supply(ID, 900, PAGE).unwrap();
    assert_eq!(document.page_count().unwrap(), 1);
    let page = document.page(0).unwrap();
    assert_eq!(page.id(), ObjectId::new(3, 0));
    assert!(matches!(
        document.content_streams(page),
        Err(ReadFault::Unavailable(_))
    ));
    source.supply(ID, 1500, CONTENT).unwrap();
    let content = document.content_streams(page).unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(
        document.stream_data(content[0]).unwrap().as_ref(),
        b"hello\n"
    );
    assert_eq!(document.diagnostics(), []);
    source.close();
    assert_eq!(
        document.page(0).unwrap_err(),
        ReadFault::Invalid(source.check().unwrap_err())
    );
    assert!(document.catalog().is_err());
    assert!(authenticated.catalog().is_err());
}

#[test]
fn complete_identity_checks_and_requested_dependency_checks_keep_their_scopes() {
    let mut bytes = literal();
    bytes[4000] = b'9';
    let options = OpenOptions {
        recovery: Recovery::Strict,
        ..OpenOptions::default()
    };
    let complete = Document::open(bytes.clone(), &options).unwrap_err();
    assert_eq!(complete.kind, ErrorKind::Syntax);
    assert_eq!(complete.object, Some(ObjectId::new(5, 0)));
    let source = source(&bytes).unwrap();
    source.supply(ID, 100, ROOT).unwrap();
    let mut job = opening(
        source.clone(),
        Validation::RequestedOperation,
        Recovery::Strict,
    )
    .unwrap();
    let document = ready(&mut job).unwrap();
    let root = document.object(ObjectId::new(1, 0)).unwrap();
    assert!(matches!(
        document.object(ObjectId::new(5, 0)),
        Err(ReadFault::Unavailable(_))
    ));
    source.supply(ID, 3968, &bytes[3968..4256]).unwrap();
    // The first nearby token candidate also needs its preceding byte.
    let Err(ReadFault::Unavailable(missing)) = document.object(ObjectId::new(5, 0)) else {
        panic!("nearby header predecessor wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(3967, 3968).unwrap()]);
    assert_eq!(document.diagnostics(), []);
    source.supply(ID, 3967, &bytes[3967..3968]).unwrap();
    let invalid = document.object(ObjectId::new(5, 0)).unwrap_err();
    assert_eq!(invalid.as_error().unwrap().kind, ErrorKind::Syntax);
    assert_eq!(
        invalid.as_error().unwrap().object,
        Some(ObjectId::new(5, 0))
    );
    assert!(std::ptr::eq(
        document.object(ObjectId::new(1, 0)).unwrap(),
        root
    ));
    assert_eq!(document.object(ObjectId::new(5, 0)).unwrap_err(), invalid);
    assert_eq!(document.validation(), Validation::RequestedOperation);
}

#[test]
fn reconstruction_requests_complete_input_once_without_becoming_sparse_success() {
    let mut bytes = literal();
    bytes[8110..8114].copy_from_slice(b"6501");
    let source = source(&bytes).unwrap();
    source.supply(ID, 100, ROOT).unwrap();
    let mut job = opening(
        source.clone(),
        Validation::RequestedOperation,
        Recovery::Repair,
    )
    .unwrap();
    let Err(ReadFault::Unavailable(missing)) = ready(&mut job) else {
        panic!("complete reconstruction wait")
    };
    assert!(
        missing
            .ranges()
            .iter()
            .any(|range| range.start() <= 2000 && range.end() > 2000)
    );
    let work = job.work();
    for _ in 0..3 {
        assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
        assert_eq!(job.work(), work);
    }
    source.supply(ID, 0, &bytes).unwrap();
    let document = ready(&mut job).unwrap();
    assert_eq!(document.validation(), Validation::RequestedOperation);
    assert_eq!(document.page_count().unwrap(), 1);
    assert_eq!(document.materialize().unwrap().bytes(), bytes);
    assert_eq!(document.diagnostics().iter().filter(|diagnostic| diagnostic.repair() == Some(pagura_document::repairs::XREF_REBUILT)).count(), 1);
    assert!(matches!(job.advance().unwrap(), Opening::Complete));
}

#[test]
fn public_security_dependencies_resume_without_materializing_the_pdf() {
    const ENCRYPT: &[u8] = concat!(
        "5 0 obj << /Filter /Standard /V 4 /R 4 /Length 128 /P 4294967292 ",
        "/O <0000000000000000000000000000000000000000000000000000000000000000> ",
        "/U <0000000000000000000000000000000000000000000000000000000000000000> ",
        "/CF 6 0 R /StmF /Identity /StrF /Identity /EFF /StdCF >> endobj\n"
    )
    .as_bytes();
    const INDEX: &[u8] = concat!(
        "xref\n0 8\n0 65535 f\n100 0 n\n600 0 n\n0 0 f\n0 0 f\n1000 0 n\n4000 0 n\n6000 0 n\n",
        "trailer << /Size 8 /Root 1 0 R /Encrypt 5 0 R ",
        "/ID [<00000000000000000000000000000000> <00000000000000000000000000000000>] >>\n"
    )
    .as_bytes();
    let bytes = literal();
    let source =
        Arc::new(SparseBytes::new(ID, 8192, &Limits::default(), &SourceLimits::default()).unwrap());
    for (offset, part) in [
        (0, &bytes[..9]),
        (100, ROOT),
        (1000, ENCRYPT),
        (6500, INDEX),
        (7168, &bytes[7168..]),
    ] {
        source.supply(ID, offset, part).unwrap();
    }
    let mut job = opening(
        source.clone(),
        Validation::RequestedOperation,
        Recovery::Repair,
    )
    .unwrap();
    let Err(ReadFault::Unavailable(missing)) = ready(&mut job) else {
        panic!("CF wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(4000, 4256).unwrap()]);
    let work = job.work();
    for _ in 0..3 {
        assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
        assert_eq!(job.work(), work);
    }
    source
        .supply(ID, 4000, b"6 0 obj << /StdCF 7 0 R >> endobj\n")
        .unwrap();
    let Err(ReadFault::Unavailable(missing)) = ready(&mut job) else {
        panic!("crypt filter wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(6000, 6256).unwrap()]);
    source
        .supply(
            ID,
            6000,
            b"7 0 obj << /CFM /V2 /Length 16 /AuthEvent /EFOpen >> endobj\n",
        )
        .unwrap();
    let document = ready(&mut job).unwrap();
    assert_eq!(document.validation(), Validation::RequestedOperation);
    let reports = document.diagnostics();
    assert_eq!(reports.len(), 2);
    assert_eq!(
        reports[0].repair(),
        Some(pagura_document::repairs::PERMISSIONS_UNSIGNED)
    );
    assert_eq!(
        reports[1].repair(),
        Some(pagura_document::repairs::CRYPT_EVENT)
    );
    for report in reports {
        assert_eq!(report.position, pagura_core::Position::File(1000));
        assert_eq!(report.object, Some(ObjectId::new(5, 0)));
    }
    assert_eq!(
        document
            .reauthenticate(b"wrong")
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Password
    );
    assert!(matches!(
        source.read_exact(ByteRange::new(2000, 2001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    assert!(matches!(
        document.materialize(),
        Err(ReadFault::Unavailable(_))
    ));
}

#[test]
fn out_of_file_prev_is_pdf_syntax_without_a_source_range_request() {
    let mut bytes = literal();
    let index = b"xref\n0 2\n0 65535 f\n100 0 n\ntrailer << /Size 2 /Root 1 0 R /Prev 9000 >>\n";
    bytes[6500..6700].fill(b' ');
    bytes[6500..6500 + index.len()].copy_from_slice(index);
    for validation in [Validation::CompleteDocument, Validation::RequestedOperation] {
        let source = source(&bytes).unwrap();
        let mut job = opening(source.clone(), validation, Recovery::Strict).unwrap();
        let error = ready(&mut job).unwrap_err();
        let invalid = error.as_error().unwrap();
        assert_eq!(invalid.kind, ErrorKind::Syntax);
        assert_eq!(invalid.position, pagura_core::Position::File(9000));
        assert_eq!(invalid.message, "offset outside input");
        let work = job.work();
        source.supply(ID, 0, &bytes).unwrap();
        for _ in 0..3 {
            assert_eq!(job.advance().unwrap_err(), error);
            assert_eq!(job.work(), work);
        }
    }
}
