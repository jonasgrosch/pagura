use super::DocumentBuilder;
use crate::xref::{XrefEntry, XrefTable};
use crate::{Location, OpenOptions, memory};
use pagura_core::{ErrorKind, Limits, Name, ObjectId, Value};
use std::sync::Arc;

#[test]
fn structure_charges_belong_to_no_object() {
    let mut table = XrefTable::default();
    table.entries.insert(
        1,
        XrefEntry {
            generation: 0,
            location: Location::Offset(9),
        },
    );
    table
        .trailer
        .insert(Name::new(b"Root"), Value::Reference(ObjectId::new(1, 0)));
    let options = OpenOptions {
        limits: Limits {
            document_bytes: memory::slot_bytes(),
            ..Limits::default()
        },
        ..OpenOptions::default()
    };
    let bytes: Arc<[u8]> = Arc::from(b"%PDF-1.7\n".as_slice());
    let bytes = crate::source::View::local(bytes, &options.limits).unwrap();
    let Err(error) = DocumentBuilder::new(bytes, &options, table, Vec::new()) else {
        panic!("the trailer does not fit beside one slot");
    };
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.object, None);
}

#[test]
fn sparse_objects_wait_for_length_and_tail_without_reading_payload_or_publishing_repairs() {
    use crate::ObjectSource;
    use crate::source::{
        ByteRange, LoadingLimits, LoadingWork, ReadFault, SourceIdentity, SourceLimits,
        SparseBytes, View,
    };
    use pagura_core::{Diagnostic, Position};

    const ID: SourceIdentity = SourceIdentity::new(710, [21; 32]);
    const STREAM: ObjectId = ObjectId::new(4, 0);
    const LENGTH: ObjectId = ObjectId::new(5, 0);
    let source =
        Arc::new(SparseBytes::new(ID, 8192, &Limits::default(), &SourceLimits::default()).unwrap());
    // An independently positioned source: key A lacks its solidus; Length
    // and the terminator are separate dependencies, and payload stays absent.
    source
        .supply(ID, 100, b"4 0 obj << /Length 5 0 R A 7 >> stream\n")
        .unwrap();
    let loading = LoadingLimits::default();
    let work = Arc::new(LoadingWork::new(loading.work));
    let bytes = View::registered(
        source.clone(),
        Limits::default(),
        loading,
        Arc::clone(&work),
    )
    .unwrap();
    let mut table = XrefTable::default();
    for (number, offset) in [(4, 100), (5, 7000)] {
        table.entries.insert(
            number,
            XrefEntry {
                generation: 0,
                location: Location::Offset(offset),
            },
        );
    }
    // This test isolates the post-index object owner. The table is literal;
    // the ordinary OpenJob supplies this same owner after authoritative xref.
    let document = DocumentBuilder::new(bytes, &OpenOptions::default(), table, Vec::new())
        .unwrap()
        .freeze();
    let Err(ReadFault::Unavailable(missing)) = document.object(STREAM) else {
        panic!("indirect Length wait");
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(7000, 7256).unwrap()]);
    let first_work = work.used();
    for _ in 0..3 {
        assert!(matches!(
            document.object(STREAM),
            Err(ReadFault::Unavailable(_))
        ));
        assert_eq!(work.used(), first_work);
        assert_eq!(document.diagnostics(), []);
    }
    source.supply(ID, 7000, b"5 0 obj 3957 endobj\n").unwrap();
    let Err(ReadFault::Unavailable(missing)) = document.object(STREAM) else {
        panic!("stream tail wait");
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(4096, 4352).unwrap()]);
    let length = document.object(LENGTH).unwrap();
    assert_eq!(length.value(), &Value::Integer(3957));
    let second_work = work.used();
    for _ in 0..3 {
        assert!(matches!(
            document.object(STREAM),
            Err(ReadFault::Unavailable(_))
        ));
        assert!(std::ptr::eq(document.object(LENGTH).unwrap(), length));
        assert_eq!(work.used(), second_work);
        assert_eq!(document.diagnostics(), []);
    }
    source.supply(ID, 4096, b"\nendstream\nendobj\n").unwrap();
    let object = document.object(STREAM).unwrap();
    assert_eq!(object.stream_range(), Some(139..4096));
    assert_eq!(object.source(), &ObjectSource::File(100..4113));
    let diagnostic = Diagnostic::new(
        pagura_core::repairs::KEY_SOLIDUS,
        "dictionary key has no leading solidus",
    )
    .at(Position::File(125))
    .in_object(STREAM);
    for _ in 0..3 {
        assert!(std::ptr::eq(document.object(STREAM).unwrap(), object));
        assert_eq!(document.diagnostics(), std::slice::from_ref(&diagnostic));
    }
    let Err(ReadFault::Unavailable(missing)) = document.raw_stream_data(object) else {
        panic!("payload is still absent");
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(139, 4096).unwrap()]);
    source.supply(ID, 139, &[b'P'; 3957]).unwrap();
    let payload = document.raw_stream_data(object).unwrap();
    assert_eq!(payload.bytes(), &[b'P'; 3957]);
    source.close();
    assert!(matches!(
        document.object(STREAM),
        Err(ReadFault::Invalid(_))
    ));
    assert!(matches!(
        document.object(LENGTH),
        Err(ReadFault::Invalid(_))
    ));
    assert_eq!(payload.bytes(), &[b'P'; 3957]);
}
