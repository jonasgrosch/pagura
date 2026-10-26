//! Frozen header/tail literals; no writer or opened document supplies expectations.

use super::{TAIL_WINDOW, parse_header, startxref};
use crate::repairs;
use crate::source::{
    ByteSource, LoadingLimits, ReadFault, SourceIdentity, SourceLimits, SparseBytes, Window,
};
use pagura_core::syntax::{InputEnd, Origin, Parser, WindowRead};
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery, Result};
use std::sync::Arc;

#[test]
fn every_header_line_prefix_waits_without_publishing_a_repair() {
    for (literal, expected_offset, expected_repairs) in [
        (&b"%PDF-1.7\n"[..], 0, vec![]),
        (&b"junk%PDF-2.0\r"[..], 4, vec![repairs::HEADER_OFFSET]),
        (
            &b"junk%PDF-1.7 extra\n"[..],
            4,
            vec![repairs::HEADER_OFFSET, repairs::HEADER_FORMAT],
        ),
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert_eq!(header(literal, &mut diagnostics).unwrap(), expected_offset);
        let expected = diagnostics.take();
        assert_eq!(
            expected
                .iter()
                .map(|diagnostic| diagnostic.repair().unwrap())
                .collect::<Vec<_>>(),
            expected_repairs
        );
        for split in 0..literal.len() {
            let read = Parser::new(&literal[..split], Origin::File, Limits::default())
                .read_window(InputEnd::Extendable, |parser| {
                    let mut diagnostics = Diagnostics::new(Recovery::Repair);
                    let offset = parse_header(parser, &mut diagnostics)?;
                    Ok((offset, diagnostics.take()))
                })
                .unwrap();
            assert!(
                matches!(read, WindowRead::Incomplete { .. }),
                "split {split}"
            );
        }
        let read = Parser::new(literal, Origin::File, Limits::default())
            .read_window(InputEnd::Extendable, |parser| {
                let mut diagnostics = Diagnostics::new(Recovery::Repair);
                let offset = parse_header(parser, &mut diagnostics)?;
                Ok((offset, diagnostics.take()))
            })
            .unwrap();
        let WindowRead::Complete(parsed) = read else {
            panic!("finished header")
        };
        assert_eq!(parsed.value, (expected_offset, expected));
        assert_eq!(parsed.offset, literal.len());
        assert_eq!(parsed.diagnostics, []);
    }
}

#[test]
fn header_search_and_line_extents_stay_bounded_at_real_eof_and_open_edges() {
    let mut last_start = vec![b'x'; 1023];
    last_start.extend_from_slice(b"%PDF-1.7\n");
    assert_eq!(
        header(&last_start, &mut Diagnostics::new(Recovery::Repair)).unwrap(),
        1023
    );
    for literal in [
        b"%PDF-1.7".to_vec(),
        vec![b'x'; 1031],
        [b"%PDF-1.7".as_slice(), &[b'x'; 1024]].concat(),
    ] {
        let error = header(&literal, &mut Diagnostics::new(Recovery::Repair)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, Position::File(0));
        let read = Parser::new(&literal, Origin::File, Limits::default())
            .read_window(InputEnd::Extendable, |parser| {
                parse_header(parser, &mut Diagnostics::new(Recovery::Repair))
            });
        if literal.len() == 8 {
            assert!(matches!(read.unwrap(), WindowRead::Incomplete { .. }));
        } else {
            assert_eq!(read.unwrap_err(), error);
        }
    }
}

#[test]
fn a_sparse_header_and_final_tail_do_not_require_the_intervening_body() {
    const ID: SourceIdentity = SourceIdentity::new(71, [83; 32]);
    let source =
        Arc::new(SparseBytes::new(ID, 4096, &Limits::default(), &SourceLimits::default()).unwrap());
    source.supply(ID, 0, b"%PDF-1.7\n").unwrap();
    let mut window = Window::new(
        source.clone(),
        0,
        Limits::default(),
        LoadingLimits::default(),
    )
    .unwrap();
    assert_eq!(
        window
            .read(|parser| parse_header(parser, &mut Diagnostics::new(Recovery::Strict)))
            .unwrap()
            .value,
        0
    );
    let header_work = window.work();
    let mut tail = vec![b' '; 1024];
    tail[..19].copy_from_slice(b"startxref\n11\n%%EOF\n");
    let newest = b"startxref\n2500\n%%EOF\n";
    tail[1024 - newest.len()..].copy_from_slice(newest);
    source.supply(ID, 3072, &tail[..512]).unwrap();
    window.seek(3072).unwrap();
    assert_eq!(window.work(), header_work);
    let read = |parser: &mut Parser<'_, _>| {
        // The last marker is knowable only after the whole final tail arrives.
        parser.has_bytes(1024)?;
        let result = startxref(
            parser.data(),
            3072,
            4096,
            &Limits::default(),
            &mut Diagnostics::new(Recovery::Strict),
        )?;
        parser.seek(1024)?;
        Ok(result)
    };
    assert!(matches!(window.read(read), Err(ReadFault::Unavailable(_))));
    let work = window.work();
    assert!(matches!(window.read(read), Err(ReadFault::Unavailable(_))));
    assert_eq!(window.work(), work);
    source.supply(ID, 3584, &tail[512..]).unwrap();
    let parsed = window.read(read).unwrap();
    assert_eq!(parsed.value, Some(2500));
    assert_eq!(parsed.diagnostics, []);
    assert_eq!(window.offset(), 4096);
    assert!(window.work() > work);
    assert!(matches!(
        source.materialize(),
        Err(ReadFault::Unavailable(_))
    ));
}

#[test]
fn final_tail_uses_the_last_marker_and_absolute_file_positions() {
    let tail = b"startxref\n11\n%%EOF\nstartxref\n4700\n%%EOF\n";
    let origin = 5000;
    let mut whole = vec![b' '; origin];
    whole.extend_from_slice(tail);
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut diagnostics = Diagnostics::new(recovery);
        assert_eq!(
            startxref(
                tail,
                origin,
                whole.len(),
                &Limits::default(),
                &mut diagnostics
            )
            .unwrap(),
            Some(4700)
        );
        assert_eq!(
            find_startxref(&whole, &Limits::default(), &mut diagnostics).unwrap(),
            Some(4700)
        );
        assert_eq!(diagnostics.as_slice(), []);
    }
    for literal in [
        &b"startxref\n9000\n%%EOF\n"[..],
        &b"startxref\n1\nmissing"[..],
        &b"startxref\n1\n%%EOFjunk"[..],
    ] {
        let mut bytes = vec![b' '; origin];
        bytes.extend_from_slice(literal);
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let mut expected = Diagnostics::new(recovery);
            let full = find_startxref(&bytes, &Limits::default(), &mut expected);
            let mut diagnostics = Diagnostics::new(recovery);
            let ranged = startxref(
                literal,
                origin,
                bytes.len(),
                &Limits::default(),
                &mut diagnostics,
            );
            assert_eq!(ranged, full);
            assert_eq!(diagnostics.as_slice(), expected.as_slice());
            if let Err(error) = ranged {
                assert!(matches!(error.position, Position::File(at) if at >= origin));
            }
        }
    }
}

/// Finds the `%PDF-` header and returns its offset (7.5.2).
fn header(bytes: &[u8], diagnostics: &mut Diagnostics) -> Result<usize> {
    parse_header(
        &mut Parser::new(bytes, Origin::File, Limits::default()),
        diagnostics,
    )
}

/// Finds the offset given after the last `startxref` (clause 7.5.5).
fn find_startxref(
    bytes: &[u8],
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Option<usize>> {
    let window = bytes.len().saturating_sub(TAIL_WINDOW);
    startxref(&bytes[window..], window, bytes.len(), limits, diagnostics)
}
