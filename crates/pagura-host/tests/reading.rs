#![forbid(unsafe_code)]
//! Reading layouts against generator expectations, independent of the JS codec.

use pagura::{Limits, TextPage};
use pagura_host::protocol;
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{Frame, fixture, frame, split_frames};
use std::io::Write;
use std::process::{Command, Stdio};

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_le_bytes()).collect()
}

fn hello(version: u32) -> Vec<u8> {
    frame(1, 1, &version.to_le_bytes())
}

#[expect(clippy::unwrap_used, reason = "small pinned fixture in test helper")]
fn open(pdf: &[u8]) -> Vec<u8> {
    let mut payload = vec![1];
    payload.extend([0; 4]);
    payload.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
    payload.extend(pdf);
    payload
}

#[expect(clippy::unwrap_used, reason = "in-memory transport in test helper")]
fn serve(session: Session, input: &[u8]) -> Vec<Vec<u8>> {
    let mut output = Vec::new();
    session.serve(input, &mut output).unwrap();
    split_frames(&output)
        .iter()
        .map(|frame| frame.body().to_vec())
        .collect()
}

/// A session whose reading responses carry at most 32 payload bytes.
fn tiny_reading_session() -> Session {
    Session::new(SessionConfig::default().with_session(SessionLimits {
        reading_response_bytes: 32,
        ..SessionLimits::default()
    }))
}

#[test]
fn native_binary_and_session_match_literal_text_layout() {
    let pdf = fixture("text", "extract-simple.pdf");
    let document = pagura::Document::open(pdf.clone(), &pagura::OpenOptions::default()).unwrap();
    let text = pagura::text_page(&document, document.page(0).unwrap()).unwrap();
    let mut input = hello(protocol::VERSION_1_2);
    input.extend(frame(2, 2, &open(&pdf)));
    input.extend(frame(10, 3, &u32s(&[1, 0])));
    input.extend(frame(15, 4, &[]));
    input.extend(frame(6, 5, &u32s(&[1])));
    input.extend(frame(15, 6, &[]));
    let mut output = Vec::new();
    Session::default()
        .serve(input.as_slice(), &mut output)
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pagura-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Write from another thread while this one drains stdout, so neither
    // side blocks on a full pipe however large the responses grow.
    let mut stdin = child.stdin.take().unwrap();
    let native = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let native = child.wait_with_output().unwrap();
        writer.join().unwrap().unwrap();
        native
    });
    assert!(native.status.success());
    assert_eq!(native.stdout, output);
    let frames: Vec<_> = split_frames(&output).iter().map(Frame::body).collect();
    let mut expected = vec![138, 3, 0, 0, 0, 2, 0, 0, 0, b'Z', b'Z', 2, 0, 0, 0];
    for x in [10_f64, 14.] {
        for n in [x, 20., x + 4., 20., x + 4., 26., x, 26.] {
            expected.extend(n.to_le_bytes());
        }
    }
    expected.extend(u32s(&[2, 1, 0, 1, 2, 1, 1]));
    assert_eq!(frames[2], expected);
    assert_eq!(frames[3][0], 143);
    assert_eq!(frames[3].len(), 41);
    let charge = u64::try_from(text.retained_bytes() + 512).unwrap();
    assert_eq!(&frames[3][29..37], &charge.to_le_bytes());
    assert_eq!(&frames[3][37..41], &1_u32.to_le_bytes());
    assert_eq!(&frames[5][5..], &[0; 36]);
}

#[test]
fn all_reading_requests_reject_truncation_and_preserve_older_sessions() {
    let mut search = u32s(&[1, 0, 1, 0, 1]);
    search.push(4);
    search.extend(u32s(&[1]));
    search.push(b'Z');
    let payloads = [
        (10, u32s(&[1, 0])),
        (11, search),
        (12, u32s(&[1, 0])),
        (13, u32s(&[1])),
        (14, u32s(&[1])),
        (15, vec![]),
        (16, u32s(&[1, 0])),
    ];
    for (kind, payload) in &payloads {
        let encoded = frame(*kind, 2, payload);
        let body = &encoded[4..];
        assert!(protocol::decode(body, Limits::default()).is_ok());
        for n in 0..body.len() {
            assert!(protocol::decode(&body[..n], Limits::default()).is_err());
        }
        let mut trailing = body.to_vec();
        trailing.push(0);
        assert!(protocol::decode(&trailing, Limits::default()).is_err());
    }
    for version in [protocol::VERSION, protocol::VERSION_1_1] {
        let mut input = hello(version);
        for (i, (kind, payload)) in payloads.iter().enumerate() {
            input.extend(frame(*kind, u32::try_from(i + 2).unwrap(), payload));
        }
        input.extend(frame(9, 20, &[]));
        let bodies = serve(Session::default(), &input);
        assert!(bodies[1..8].iter().all(|b| b[0] == 255 && b[5] == 7));
    }
}

#[test]
fn content_bounds_are_additive_and_match_independent_rectangle_geometry() {
    let pdf = fixture("classic", "two-pages.pdf");
    for version in [
        protocol::VERSION,
        protocol::VERSION_1_1,
        protocol::VERSION_1_2,
        protocol::VERSION_1_3,
        protocol::VERSION_1_4,
    ] {
        let mut input = hello(version);
        input.extend(frame(2, 2, &open(&pdf)));
        input.extend(frame(16, 3, &u32s(&[1, 0])));
        input.extend(frame(16, 4, &u32s(&[1, 1])));
        input.extend(frame(3, 5, &u32s(&[1, 0])));
        input.extend(frame(16, 6, &u32s(&[1, 2])));
        input.extend(frame(16, 7, &u32s(&[99, 0])));
        input.extend(frame(16, 8, &u32s(&[1, 0])));
        input.extend(frame(9, 9, &[]));
        let frames = serve(Session::default(), &input);
        assert_eq!(frames[4][0], 131);
        if matches!(version, protocol::VERSION_1_3 | protocol::VERSION_1_4) {
            assert_eq!(&frames[8][25..29], &2_u32.to_le_bytes());
            for (id, coordinates) in [(3_u32, [8_f64, 8., 24., 24.]), (4, [32., 32., 64., 64.])] {
                let mut expected = vec![144];
                expected.extend(id.to_le_bytes());
                expected.push(1);
                for value in coordinates {
                    expected.extend(value.to_le_bytes());
                }
                assert_eq!(frames[usize::try_from(id - 1).unwrap()], expected);
            }
        } else {
            for result in &frames[2..4] {
                assert_eq!((result[0], result[5]), (255, 7));
            }
        }
        for result in &frames[5..7] {
            assert_eq!((result[0], result[5]), (255, 7));
        }
    }
    let blank = fixture("classic", "blank.pdf");
    let mut input = hello(protocol::VERSION_1_3);
    input.extend(frame(2, 2, &open(&blank)));
    input.extend(frame(16, 3, &u32s(&[1, 0])));
    assert_eq!(serve(Session::default(), &input)[2], &[144, 3, 0, 0, 0, 0]);
    // 33 payload bytes exceed a 32-byte reading budget.
    let mut input = hello(protocol::VERSION_1_3);
    input.extend(frame(2, 2, &open(&pdf)));
    input.extend(frame(16, 3, &u32s(&[1, 0])));
    let bodies = serve(tiny_reading_session(), &input);
    assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
}

#[test]
fn text_cache_budget_eviction_and_close_release_accounting() {
    let pdf = fixture("text", "extract-simple.pdf");
    let document = pagura::Document::open(pdf.clone(), &pagura::OpenOptions::default()).unwrap();
    let page: TextPage = pagura::text_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(page.text(), "ZZ");
    let mut input = hello(protocol::VERSION_1_2);
    input.extend(frame(2, 2, &open(&pdf)));
    input.extend(frame(10, 3, &u32s(&[1, 0])));
    input.extend(frame(15, 4, &[]));
    let uncached = Session::new(SessionConfig::default().with_session(SessionLimits {
        text_cache_bytes: 0,
        ..SessionLimits::default()
    }));
    let frames = serve(uncached, &input);
    assert_eq!(frames[2][0], 138);
    assert_eq!(&frames[3][29..41], &[0; 12]);
    let frames = serve(tiny_reading_session(), &input);
    assert_eq!(frames[2][0], 255);
    assert_eq!(frames[2][5], 2);
}
