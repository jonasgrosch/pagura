use super::{Build, Stage, State};
use crate::builder::DocumentBuilder;
use crate::source::{
    ByteRange, LoadingLimits, LoadingWork, ReadFault, SourceIdentity, SourceLimits, SparseBytes,
    View,
};
use crate::xref::{XrefEntry, XrefTable};
use crate::{Location, OpenOptions, repairs};
use pagura_core::{Diagnostic, Limits, Name, ObjectId, Position, Result, Value};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(712, [23; 32]);
const ENCRYPT: &[u8] = concat!(
    "5 0 obj << /Filter /Standard /V 4 /R 4 /Length 128 /P 4294967292 ",
    "/O <0000000000000000000000000000000000000000000000000000000000000000> ",
    "/U <0000000000000000000000000000000000000000000000000000000000000000> ",
    "/CF 6 0 R /StmF /Identity /StrF /Identity /EFF /StdCF >> endobj\n"
)
.as_bytes();

fn source() -> Result<Arc<SparseBytes>> {
    Ok(Arc::new(SparseBytes::new(
        ID,
        8192,
        &Limits::default(),
        &SourceLimits::default(),
    )?))
}

fn build(source: Arc<SparseBytes>, table: XrefTable) -> Result<(Build, Arc<LoadingWork>)> {
    let loading = LoadingLimits::default();
    let work = Arc::new(LoadingWork::new(loading.work));
    let view = View::registered(source, Limits::default(), loading, Arc::clone(&work))?;
    let builder = DocumentBuilder::new(
        view,
        &OpenOptions::default(),
        table,
        Vec::new(),
        crate::Validation::RequestedOperation,
    )?;
    Ok((Build::new(builder), work))
}

fn table(entries: &[(u32, usize)]) -> XrefTable {
    let mut table = XrefTable::default();
    for &(number, offset) in entries {
        table.entries.insert(
            number,
            XrefEntry {
                generation: 0,
                location: Location::Offset(offset),
            },
        );
    }
    table
        .trailer
        .insert(Name::new(b"Root"), Value::Reference(ObjectId::new(1, 0)));
    table
}

#[test]
fn catalog_waits_retain_completed_dependencies_and_publish_inference_once() {
    let source = source().unwrap();
    source
        .supply(ID, 100, b"1 0 obj << /Pages 2 0 R A 7 >> endobj\n")
        .unwrap();
    let (mut build, work) = build(source.clone(), table(&[(1, 100), (2, 600)])).unwrap();
    let Err(ReadFault::Unavailable(missing)) = build.advance(&[]) else {
        panic!("Pages dependency wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(600, 856).unwrap()]);
    let State::Active(active) = &build.state else {
        panic!("unpublished construction")
    };
    let root = std::ptr::from_ref(active.document.object(ObjectId::new(1, 0)).unwrap());
    let diagnostic = Diagnostic::new(
        pagura_core::repairs::KEY_SOLIDUS,
        "dictionary key has no leading solidus",
    )
    .at(Position::File(124))
    .in_object(ObjectId::new(1, 0));
    assert_eq!(
        active.document.diagnostics(),
        std::slice::from_ref(&diagnostic)
    );
    let admitted_work = work.used();
    let retained = active.document.retained.used();
    for _ in 0..3 {
        assert!(matches!(build.advance(&[]), Err(ReadFault::Unavailable(_))));
        let State::Active(active) = &build.state else {
            panic!("waiting construction")
        };
        assert!(std::ptr::eq(
            active.document.object(ObjectId::new(1, 0)).unwrap(),
            root
        ));
        assert_eq!(
            active.document.diagnostics(),
            std::slice::from_ref(&diagnostic)
        );
        assert_eq!(active.document.retained.used(), retained);
        assert_eq!(work.used(), admitted_work);
    }
    source
        .supply(
            ID,
            600,
            b"2 0 obj << /Type /Pages /Count 0 /Kids [] >> endobj\n",
        )
        .unwrap();
    let document = build.advance(&[]).unwrap().unwrap();
    assert!(std::ptr::eq(
        document.object(ObjectId::new(1, 0)).unwrap(),
        root
    ));
    assert_eq!(
        document.diagnostics(),
        [
            diagnostic,
            Diagnostic::new(
                repairs::CATALOG_TYPE,
                "catalog Type is missing; validated Pages reference identifies the catalog"
            )
            .at(Position::File(100))
            .in_object(ObjectId::new(1, 0))
        ]
    );
    assert_eq!(document.page_count().unwrap(), 0);
    assert!(build.advance(&[]).unwrap().is_none());
}

#[test]
fn security_waits_keep_scratch_then_clear_preauthentication_caches_once() {
    let source = source().unwrap();
    // Literal non-matching credentials select the existing EFOpen-only policy:
    // document opening succeeds without granting embedded-data authorization.
    source.supply(ID, 1000, ENCRYPT).unwrap();
    let mut index = table(&[(1, 100), (5, 1000), (6, 4000), (7, 6000)]);
    index
        .trailer
        .insert(Name::new(b"Encrypt"), Value::Reference(ObjectId::new(5, 0)));
    index.trailer.insert(
        Name::new(b"ID"),
        Value::Array(vec![Value::String(vec![0; 16]), Value::String(vec![0; 16])]),
    );
    let (mut build, work) = build(source.clone(), index).unwrap();
    let Err(ReadFault::Unavailable(missing)) = build.advance(&[]) else {
        panic!("CF dictionary wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(4000, 4256).unwrap()]);
    let State::Active(active) = &build.state else {
        panic!("security scratch")
    };
    let encryption = std::ptr::from_ref(active.document.object(ObjectId::new(5, 0)).unwrap());
    assert_eq!(active.document.diagnostics(), []);
    let admitted_work = work.used();
    for _ in 0..3 {
        assert!(matches!(build.advance(&[]), Err(ReadFault::Unavailable(_))));
        let State::Active(active) = &build.state else {
            panic!("security wait")
        };
        assert!(std::ptr::eq(
            active.document.object(ObjectId::new(5, 0)).unwrap(),
            encryption
        ));
        assert_eq!(active.document.diagnostics(), []);
        assert_eq!(work.used(), admitted_work);
    }
    source
        .supply(ID, 4000, b"6 0 obj << /StdCF 7 0 R >> endobj\n")
        .unwrap();
    let Err(ReadFault::Unavailable(missing)) = build.advance(&[]) else {
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
    assert!(build.advance(&[]).unwrap().is_none());
    let State::Active(active) = &build.state else {
        panic!("authenticated construction")
    };
    assert!(matches!(active.stage, Stage::Catalog));
    assert!(active.document.security.is_some());
    assert_eq!(active.document.retained.used(), active.mark);
    assert!(
        active
            .document
            .slots
            .values()
            .all(|slot| slot.object.get().is_none())
    );
    assert_eq!(
        active.document.diagnostics(),
        [
            Diagnostic::new(
                repairs::PERMISSIONS_UNSIGNED,
                "unsigned 32-bit permissions reinterpreted as signed two's complement"
            )
            .at(Position::File(1000))
            .in_object(ObjectId::new(5, 0)),
            Diagnostic::new(
                repairs::CRYPT_EVENT,
                "EFOpen-only filter used as a Standard-handler extension"
            )
            .at(Position::File(1000))
            .in_object(ObjectId::new(5, 0)),
        ]
    );
    let Err(ReadFault::Unavailable(missing)) = build.advance(&[]) else {
        panic!("catalog wait after security")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(100, 356).unwrap()]);
    source
        .supply(ID, 100, b"1 0 obj << /Type /Catalog >> endobj\n")
        .unwrap();
    let document = build.advance(&[]).unwrap().unwrap();
    assert_eq!(document.diagnostics().len(), 2);
    assert!(document.slots[&ObjectId::new(5, 0)].object.get().is_none());
    assert!(!document.security.as_ref().unwrap().authenticated());
}

#[test]
fn source_failure_is_terminal_even_after_a_dependency_completed() {
    let source = source().unwrap();
    source
        .supply(ID, 100, b"1 0 obj << /Pages 2 0 R >> endobj\n")
        .unwrap();
    let (mut build, work) = build(source.clone(), table(&[(1, 100), (2, 600)])).unwrap();
    assert!(matches!(build.advance(&[]), Err(ReadFault::Unavailable(_))));
    source.close();
    let error = build.advance(&[]).unwrap_err();
    assert!(matches!(error, ReadFault::Invalid(_)));
    let admitted_work = work.used();
    for _ in 0..3 {
        assert_eq!(build.advance(&[]).unwrap_err(), error);
        assert_eq!(work.used(), admitted_work);
    }
}
