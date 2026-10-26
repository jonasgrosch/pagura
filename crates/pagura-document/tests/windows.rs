//! Independent literal syntax at nonzero source offsets, including sparse holes.

use pagura_core::syntax::Parser;
use pagura_core::{ErrorKind, Limits, Name, Position, Recovery, Result, Value};
use pagura_document::source::{
    ByteRange, ByteSource, CompleteBytes, LoadingLimits, ReadFault, SourceIdentity, SourceLimits,
    SparseBytes, Window,
};
use std::cell::Cell;
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(19, [91; 32]);

fn sparse(length: u64) -> Result<Arc<SparseBytes>> {
    Ok(Arc::new(SparseBytes::new(
        ID,
        length,
        &Limits::default(),
        &SourceLimits::default(),
    )?))
}

fn reader(source: Arc<dyn ByteSource>, at: u64) -> Result<Window> {
    Window::new(source, at, Limits::default(), LoadingLimits::default())
}

#[test]
fn complete_and_sparse_windows_finish_before_unrelated_missing_bytes() {
    let literal = b"<< /N /A#20B /S (text) /V [true false null] >>";
    let source = sparse(1024).unwrap();
    source.supply(ID, 311, literal).unwrap();
    let mut window = reader(source.clone(), 311).unwrap();
    let parsed = window.read(Parser::parse).unwrap();
    let dictionary = parsed.value.as_dictionary().unwrap();
    assert_eq!(dictionary.get(b"N"), Some(&Value::Name(Name::new(b"A B"))));
    assert_eq!(dictionary.get(b"S"), Some(&Value::String(b"text".to_vec())));
    assert_eq!(
        dictionary.get(b"V"),
        Some(&Value::Array(vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Null,
        ]))
    );
    assert_eq!(parsed.offset, literal.len());
    assert_eq!(window.offset(), 311 + u64::try_from(literal.len()).unwrap());
    assert!(parsed.diagnostics.is_empty());
    assert!(matches!(
        source.materialize(),
        Err(ReadFault::Unavailable(_))
    ));

    let mut bytes = vec![b' '; 1024];
    bytes[311..311 + literal.len()].copy_from_slice(literal);
    let complete = CompleteBytes::new(
        ID,
        Arc::from(bytes),
        &Limits::default(),
        &SourceLimits::default(),
    )
    .unwrap();
    let other = reader(Arc::new(complete), 311)
        .unwrap()
        .read(Parser::parse)
        .unwrap();
    assert_eq!(other.value, parsed.value);
    assert_eq!(other.offset, parsed.offset);
    assert_eq!(other.diagnostics, parsed.diagnostics);
}

#[test]
fn split_units_wait_without_publishing_values_or_provisional_repairs() {
    let literal = b"<< /A 1 /A 2 /Text (x\\050y\\051) /Hex <4f0a> >>";
    for split in 0..literal.len() {
        let source = sparse(512).unwrap();
        source.supply(ID, 101, &literal[..split]).unwrap();
        let mut window = reader(source.clone(), 101)
            .unwrap()
            .with_recovery(Recovery::Repair);
        let calls = Cell::new(0);
        let parse = |parser: &mut Parser<'_, _>| {
            calls.set(calls.get() + 1);
            parser.parse()
        };
        let Err(ReadFault::Unavailable(missing)) = window.read(parse) else {
            panic!("unfinished unit at split {split}")
        };
        assert_eq!(window.offset(), 101);
        let work = window.work();
        let attempts = calls.get();
        let Err(ReadFault::Unavailable(again)) = window.read(parse) else {
            panic!("same wait")
        };
        assert_eq!(window.work(), work);
        assert_eq!(calls.get(), attempts);
        assert!(std::ptr::eq(missing.ranges(), again.ranges()));
        assert!(missing.ranges().iter().all(|range| range.start() >= 101));
        source
            .supply(ID, 101 + u64::try_from(split).unwrap(), &literal[split..])
            .unwrap();
        let parsed = window.read(parse).unwrap();
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].position, Position::File(109));
        let dictionary = parsed.value.as_dictionary().unwrap();
        assert_eq!(dictionary.get(b"A"), Some(&Value::Integer(2)));
        assert_eq!(
            dictionary.get(b"Text"),
            Some(&Value::String(b"x(y)".to_vec()))
        );
        assert_eq!(
            dictionary.get(b"Hex"),
            Some(&Value::String(vec![0x4f, 0x0a]))
        );
        assert_eq!(window.offset(), 101 + u64::try_from(literal.len()).unwrap());
    }
}

#[test]
fn malformed_available_prefix_is_terminal_even_when_later_bytes_are_missing() {
    let source = sparse(512).unwrap();
    source.supply(ID, 103, b"12x").unwrap();
    let mut window = reader(source.clone(), 103).unwrap();
    let Err(ReadFault::Invalid(error)) = window.read(Parser::unsigned) else {
        panic!("invalid unsigned token")
    };
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.position, Position::File(106));
    let work = window.work();
    source.supply(ID, 106, b" ").unwrap();
    let Err(ReadFault::Invalid(again)) = window.read(Parser::unsigned) else {
        panic!("terminal result")
    };
    assert_eq!(again, error);
    assert_eq!(window.work(), work);
    assert_eq!(window.offset(), 103);
}

#[test]
fn complete_eof_and_extendable_boundary_have_distinct_meanings() {
    let truncated = sparse(5).unwrap();
    truncated.supply(ID, 2, b"(ab").unwrap();
    let Err(ReadFault::Invalid(error)) = reader(truncated, 2).unwrap().read(Parser::parse) else {
        panic!("truncated literal at EOF")
    };
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.position, Position::File(5));

    let source = sparse(6).unwrap();
    source.supply(ID, 2, b"(ab").unwrap();
    let mut window = reader(source.clone(), 2).unwrap();
    let Err(ReadFault::Unavailable(missing)) = window.read(Parser::parse) else {
        panic!("literal can be extended")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(5, 6).unwrap()]);
    source.supply(ID, 5, b")").unwrap();
    assert_eq!(
        window.read(Parser::parse).unwrap().value,
        Value::String(b"ab".to_vec())
    );
}

#[test]
fn window_growth_and_repeated_units_share_one_cumulative_work_budget() {
    let mut bytes = vec![b' '; 512];
    bytes[0] = b'(';
    bytes[1..301].fill(b'x');
    bytes[301] = b')';
    let source = Arc::new(
        CompleteBytes::new(
            ID,
            Arc::from(bytes),
            &Limits::default(),
            &SourceLimits::default(),
        )
        .unwrap(),
    );
    let mut window = reader(source.clone(), 0).unwrap();
    assert_eq!(
        window.read(Parser::parse).unwrap().value,
        Value::String(vec![b'x'; 300])
    );
    assert_eq!(window.offset(), 302);
    assert_eq!(window.work(), 1 + 256 + 1 + 512);
    for loading in [
        LoadingLimits {
            work: 769,
            ..LoadingLimits::default()
        },
        LoadingLimits {
            window_bytes: 301,
            ..LoadingLimits::default()
        },
    ] {
        let mut bounded = Window::new(source.clone(), 0, Limits::default(), loading).unwrap();
        assert!(
            matches!(bounded.read(Parser::parse), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit)
        );
        let work = bounded.work();
        assert!(bounded.read(Parser::parse).is_err());
        assert_eq!(bounded.work(), work);
    }

    let source = sparse(6).unwrap();
    source.supply(ID, 0, b"(a)(b)").unwrap();
    let mut window = Window::new(
        source,
        0,
        Limits::default(),
        LoadingLimits {
            work: 10,
            ..LoadingLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        window.read(Parser::parse).unwrap().value,
        Value::String(b"a".to_vec())
    );
    assert_eq!(window.offset(), 3);
    assert!(
        matches!(window.read(Parser::parse), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit)
    );
    assert_eq!(window.offset(), 3);
}

#[test]
fn unchanged_availability_does_not_hide_source_closure_or_conflicting_supply() {
    for conflict in [false, true] {
        let source = sparse(8).unwrap();
        source.supply(ID, 0, b"(ab").unwrap();
        let mut window = reader(source.clone(), 0).unwrap();
        assert!(matches!(
            window.read(Parser::parse),
            Err(ReadFault::Unavailable(_))
        ));
        let generation = source.generation();
        let work = window.work();
        if conflict {
            assert!(source.supply(ID, 1, b"X").is_err());
        } else {
            source.close();
        }
        assert_eq!(source.generation(), generation);
        assert!(
            matches!(window.read(Parser::parse), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::InvalidArgument)
        );
        assert_eq!(window.work(), work);
    }
}

#[test]
fn declared_length_and_window_positions_obey_the_same_target_size_boundary() {
    let length = u64::from(u32::MAX) + 17;
    let limits = Limits {
        file_bytes: usize::MAX,
        ..Limits::default()
    };
    let source = SparseBytes::new(ID, length, &limits, &SourceLimits::default());
    if usize::BITS == 32 {
        assert_eq!(source.unwrap_err().kind, ErrorKind::Limit);
        return;
    }
    let source = Arc::new(source.unwrap());
    source.supply(ID, length - 4, b"true").unwrap();
    let mut window = Window::new(source, length - 4, limits, LoadingLimits::default()).unwrap();
    assert_eq!(
        window.read(Parser::parse).unwrap().value,
        Value::Boolean(true)
    );
    assert_eq!(window.offset(), length);
    assert!(window.work() < 10);

    assert_eq!(
        SparseBytes::new(ID, length, &Limits::default(), &SourceLimits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
