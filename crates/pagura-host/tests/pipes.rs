#![forbid(unsafe_code)]
//! Cross-process transport checks (host-native-pipes).

use pagura_testkit::{Manifest, frame};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[test]
fn binary_flushes_live_responses_and_reports_structured_unsupported() {
    let manifest = Manifest::load("classic");
    let case = manifest
        .cases()
        .find(|case| case.string("expected") == Some("unsupported"))
        .expect("the classic manifest has an unsupported case");
    let feature = case.string("feature").unwrap();
    let pdf = std::fs::read(case.path().unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pagura-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    // Literal handshake independent of the encoder. Read before EOF proves flush.
    input
        .write_all(&[9, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0])
        .unwrap();
    let mut hello = [0; 13];
    output.read_exact(&mut hello).unwrap();
    assert_eq!(hello, [9, 0, 0, 0, 129, 1, 0, 0, 0, 1, 0, 0, 0]);
    let mut open = vec![0, 0, 0, 0, 0];
    open.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
    open.extend(pdf);
    input.write_all(&frame(2, 2, &open)).unwrap();
    let mut opened = [0; 17];
    output.read_exact(&mut opened).unwrap();
    assert_eq!(opened[4], 130);
    input
        .write_all(&[13, 0, 0, 0, 3, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let mut info = [0; 59];
    output.read_exact(&mut info).unwrap();
    assert_eq!(info[4], 131);
    let width = u32::from_le_bytes(info[51..55].try_into().unwrap());
    let height = u32::from_le_bytes(info[55..59].try_into().unwrap());
    let mut tile = vec![1, 0, 0, 0, 0, 0, 0, 0];
    tile.extend(1_f64.to_le_bytes());
    for value in [0_u32, 0, width, height] {
        tile.extend(value.to_le_bytes());
    }
    input.write_all(&frame(4, 4, &tile)).unwrap();
    drop(input);
    let mut responses = Vec::new();
    output.read_to_end(&mut responses).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(responses[4], 255);
    assert_eq!(responses[9], 3);
    let tag_length = u32::from_le_bytes(responses[10..14].try_into().unwrap()) as usize;
    let tag = std::str::from_utf8(&responses[14..14 + tag_length]).unwrap();
    assert_eq!(tag, feature);
}

#[test]
fn binary_exits_after_one_terminal_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_pagura-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(&[255; 4])?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout[4], 255);
    assert_eq!(output.stdout[9], 2);
}
