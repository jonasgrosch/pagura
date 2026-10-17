#![forbid(unsafe_code)]
//! Cross-process transport checks (host-native-pipes).

use pagura_testkit::fixture;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn binary_flushes_live_responses_and_reports_structured_unsupported() {
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
    std::io::Read::read_exact(&mut output, &mut hello).unwrap();
    assert_eq!(hello, [9, 0, 0, 0, 129, 1, 0, 0, 0, 1, 0, 0, 0]);
    let pdf = fixture("classic", "text-unsupported.pdf");
    let mut open = u32::try_from(pdf.len() + 14)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    open.extend([2, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
    open.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
    open.extend(pdf);
    input.write_all(&open).unwrap();
    let mut tile = vec![4, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
    tile.extend(1_f64.to_le_bytes());
    for value in [0_u32, 0, 64, 64] {
        tile.extend(value.to_le_bytes());
    }
    input
        .write_all(&u32::try_from(tile.len()).unwrap().to_le_bytes())
        .unwrap();
    input.write_all(&tile).unwrap();
    drop(input);
    let mut responses = Vec::new();
    std::io::Read::read_to_end(&mut output, &mut responses).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(responses[4], 130);
    let next = 4 + u32::from_le_bytes(responses[..4].try_into().unwrap()) as usize;
    assert_eq!(responses[next + 4], 255);
    assert_eq!(responses[next + 9], 3);
    let tag_length =
        u32::from_le_bytes(responses[next + 10..next + 14].try_into().unwrap()) as usize;
    let tag = std::str::from_utf8(&responses[next + 14..next + 14 + tag_length]).unwrap();
    assert!(
        tag.contains('.'),
        "Unsupported must carry a stable feature tag"
    );
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
