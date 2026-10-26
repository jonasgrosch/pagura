//! Hand-authored section grammar and field values, independent of a PDF writer.

use super::{Location, RevisionKind, Section, XrefEntry, parse_section, section};
use crate::repairs;
use crate::source::{
    LoadingLimits, ReadFault, ReadResult, SourceIdentity, SourceLimits, SparseBytes, Window,
};
use pagura_core::syntax::ParsedWindow;
use pagura_core::{Diagnostic, Diagnostics, ErrorKind, Limits, ObjectId, Position, Recovery};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(23, [72; 32]);
const START: usize = 512;
const FILE_LENGTH: usize = 2048;

fn stream(eol: &[u8]) -> Vec<u8> {
    let mut bytes = b"7 0 obj\n<< /Type /XRef /W [1 2 2] /Size 8 /Index [0 3 7 1] /Root 1 0 R /Length 20 >>\nstream".to_vec();
    bytes.extend_from_slice(eol);
    // Type, big-endian offset and generation. Object 7 is this xref stream.
    bytes.extend_from_slice(&[
        0, 0, 0, 255, 255, 1, 0, 17, 0, 0, 1, 4, 176, 0, 2, 1, 2, 0, 0, 0,
    ]);
    bytes.extend_from_slice(b"\nendstream\nendobj ");
    bytes
}

fn parse(
    window: &mut Window,
    recovery: Recovery,
) -> ReadResult<ParsedWindow<(Section, Vec<Diagnostic>)>> {
    window.read(|parser| {
        let mut diagnostics = Diagnostics::new(recovery);
        let section = parse_section(parser, FILE_LENGTH, &Limits::default(), &mut diagnostics)?;
        Ok((section, diagnostics.take()))
    })
}

fn source() -> pagura_core::Result<Arc<SparseBytes>> {
    Ok(Arc::new(SparseBytes::new(
        ID,
        2048,
        &Limits::default(),
        &SourceLimits::default(),
    )?))
}

#[test]
fn table_and_stream_sections_share_complete_and_sparse_grammar_at_every_split() {
    let table = b"xref\n0 3\n0000000000 65535 f \n0000000017 00000 n \n0000001200 00002 n \n7 1\n0000000512 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R >>".to_vec();
    for (literal, kind) in [
        (table, RevisionKind::Table),
        (stream(b"\r\n"), RevisionKind::Stream),
    ] {
        let mut bytes = vec![b' '; FILE_LENGTH];
        bytes[START..START + literal.len()].copy_from_slice(&literal);
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let complete = section(&bytes, START, &Limits::default(), &mut diagnostics).unwrap();
        assert_eq!(complete.kind, kind);
        assert_eq!(diagnostics.as_slice(), []);
        let expected = std::collections::BTreeMap::from([
            (
                0,
                XrefEntry {
                    generation: 65535,
                    location: Location::Free,
                },
            ),
            (
                1,
                XrefEntry {
                    generation: 0,
                    location: Location::Offset(17),
                },
            ),
            (
                2,
                XrefEntry {
                    generation: 2,
                    location: Location::Offset(1200),
                },
            ),
            (
                7,
                XrefEntry {
                    generation: 0,
                    location: Location::Offset(512),
                },
            ),
        ]);
        assert_eq!(complete.entries, expected);

        for split in 0..literal.len() {
            let source = source().unwrap();
            source.supply(ID, 512, &literal[..split]).unwrap();
            let mut window = Window::new(
                source.clone(),
                512,
                Limits::default(),
                LoadingLimits::default(),
            )
            .unwrap();
            assert!(
                matches!(
                    parse(&mut window, Recovery::Strict),
                    Err(ReadFault::Unavailable(_))
                ),
                "{kind:?} split {split}"
            );
            source
                .supply(ID, 512 + u64::try_from(split).unwrap(), &literal[split..])
                .unwrap();
            let parsed = parse(&mut window, Recovery::Strict).unwrap();
            let (sparse, repairs) = parsed.value;
            assert_eq!(sparse.entries, expected);
            assert_eq!(sparse.trailer, complete.trailer);
            assert_eq!(sparse.kind, complete.kind);
            assert_eq!(START + sparse.end, complete.end);
            assert_eq!(repairs, []);
            assert_eq!(parsed.diagnostics, []);
        }
    }
}

#[test]
fn partial_cr_never_publishes_a_bare_cr_repair_or_strict_refusal() {
    let literal = stream(b"\r");
    let split = literal
        .windows(7)
        .position(|part| part == b"stream\r")
        .unwrap()
        + 7;
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let source = source().unwrap();
        source.supply(ID, 512, &literal[..split]).unwrap();
        let mut window = Window::new(
            source.clone(),
            512,
            Limits::default(),
            LoadingLimits::default(),
        )
        .unwrap()
        .with_recovery(recovery);
        assert!(matches!(
            parse(&mut window, recovery),
            Err(ReadFault::Unavailable(_))
        ));
        source
            .supply(ID, 512 + u64::try_from(split).unwrap(), &literal[split..])
            .unwrap();
        if recovery == Recovery::Strict {
            let Err(ReadFault::Invalid(error)) = parse(&mut window, recovery) else {
                panic!("bare CR strict refusal")
            };
            assert_eq!(error.repair, Some(repairs::STREAM_EOL));
            assert_eq!(error.object, Some(ObjectId::new(7, 0)));
            assert_eq!(error.position, Position::File(START + split - 1));
        } else {
            let parsed = parse(&mut window, recovery).unwrap();
            let (_, diagnostics) = parsed.value;
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].repair(), Some(repairs::STREAM_EOL));
            assert_eq!(diagnostics[0].position, Position::File(START + split - 1));
        }
    }
}

#[test]
fn irreparable_xref_fields_remain_terminal_without_fetching_later_holes() {
    for literal in [&b"xref\n0x"[..], b"xref\n1 1\n4096 0 n "] {
        let source = source().unwrap();
        source.supply(ID, 512, literal).unwrap();
        let mut window =
            Window::new(source, 512, Limits::default(), LoadingLimits::default()).unwrap();
        let Err(ReadFault::Invalid(error)) = parse(&mut window, Recovery::Strict) else {
            panic!("invalid field is not an input wait")
        };
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert!(matches!(error.position, Position::File(at) if at >= START));
    }
}

#[test]
fn owned_revision_chain_keeps_a_primary_section_while_its_supplement_waits() {
    use super::chain::{Chain, Kind};
    use crate::source::ByteSource;

    let primary = b"xref\n1 1\n22 0 n\n7 1\n700 0 n\ntrailer << /Size 8 /Root 1 0 R /Prev 100 /XRefStm 700 >>".to_vec();
    let mut supplement =
        b"7 0 obj\n<< /Type /XRef /Size 8 /Index [1 1] /W [1 2 1] /Length 4 >>\nstream\n".to_vec();
    supplement.extend_from_slice(&[1, 0, 33, 0]);
    supplement.extend_from_slice(b"\nendstream\nendobj ");
    let older = b"xref\n0 3\n0 65535 f\n7 0 n\n80 0 n\ntrailer << /Size 8 >>".to_vec();
    let limits = Limits {
        objects: 6,
        ..Limits::default()
    };
    let mut chain = Chain::new(1000, 0, &limits).unwrap();
    let source = source().unwrap();
    for (offset, kind, literal) in [
        (1000, Kind::Section, primary),
        (700, Kind::Stream, supplement),
        (100, Kind::Section, older),
    ] {
        let request = chain.request().unwrap();
        assert_eq!((request.offset, request.kind), (offset, kind));
        let limits = Limits {
            objects: request.objects,
            ..limits
        };
        let mut window = Window::new(
            source.clone(),
            u64::try_from(offset).unwrap(),
            limits,
            LoadingLimits::default(),
        )
        .unwrap();
        let read = |parser: &mut pagura_core::syntax::Parser<'_, _>| {
            let mut diagnostics = Diagnostics::new(Recovery::Strict);
            let section = match kind {
                Kind::Section => parse_section(parser, FILE_LENGTH, &limits, &mut diagnostics),
                Kind::Stream => {
                    super::stream::parse(parser, FILE_LENGTH, &limits, &mut diagnostics)
                }
            }?;
            Ok((section, diagnostics.take()))
        };
        assert!(matches!(window.read(read), Err(ReadFault::Unavailable(_))));
        assert_eq!(chain.request(), Some(request));
        source
            .supply(ID, u64::try_from(offset).unwrap(), &literal)
            .unwrap();
        let parsed = window.read(read).unwrap();
        let (mut section, diagnostics) = parsed.value;
        assert_eq!(diagnostics, []);
        assert_eq!(parsed.diagnostics, []);
        section.end += offset;
        chain.accept(section).unwrap();
    }
    assert_eq!(chain.request(), None);
    let table = chain.into_table();
    assert_eq!(table.entries[&1].location, Location::Offset(33));
    assert_eq!(table.entries[&2].location, Location::Offset(80));
    assert_eq!(table.entries[&7].location, Location::Offset(700));
    assert_eq!(table.entries[&0].location, Location::Free);
    assert_eq!(table.revisions.len(), 2);
    assert_eq!(table.revisions[0].kind(), RevisionKind::Hybrid);
    assert_eq!(table.revisions[0].entry_count(), 3);
    assert_eq!(table.revisions[1].entry_count(), 3);
    assert!(matches!(
        source.materialize(),
        Err(ReadFault::Unavailable(_))
    ));
}
