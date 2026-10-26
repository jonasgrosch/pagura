use super::{RGB_CHUNK_BYTES, drain_rgb, run, write_ppm_header, write_rgb};
use crate::args::{Command, Source};
use crate::failure::Failure;
use pagura::Recovery;
use pagura_testkit::fixture_path;
use std::io::{self, Write};

fn source(area: &str, file: &str, recovery: Recovery) -> Source {
    Source {
        path: fixture_path(area, file),
        password: Vec::new(),
        recovery,
    }
}

fn output(command: &Command) -> Result<String, Failure> {
    let mut out = Vec::new();
    run(command, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn streamed_ppm_header_and_payload_keep_pixel_order_across_chunks_and_calls() {
    let mut actual = Vec::new();
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    write_ppm_header(&mut actual, (3, 2)).unwrap();
    write_rgb(
        &mut actual,
        &[1, 2, 3, 255, 11, 22, 33, 255],
        &mut rgb,
        &mut used,
    )
    .unwrap();
    write_rgb(
        &mut actual,
        &[
            101, 102, 103, 255, 10, 20, 30, 255, 0, 127, 255, 255, 255, 254, 253, 255,
        ],
        &mut rgb,
        &mut used,
    )
    .unwrap();
    assert_eq!(actual, b"P6\n3 2\n255\n");
    drain_rgb(&mut actual, &rgb, &mut used).unwrap();
    assert_eq!(
        actual,
        b"P6\n3 2\n255\n\x01\x02\x03\x0b\x16\x21\x65\x66\x67\x0a\x14\x1e\x00\x7f\xff\xff\xfe\xfd"
    );
    let pixels: Vec<u8> = [[1, 2, 3, 255], [101, 102, 103, 255]]
        .into_iter()
        .cycle()
        .take(RGB_CHUNK_BYTES / 3 + 3)
        .flatten()
        .collect();
    actual.clear();
    write_rgb(&mut actual, &pixels, &mut rgb, &mut used).unwrap();
    assert_eq!(actual.len(), RGB_CHUNK_BYTES);
    drain_rgb(&mut actual, &rgb, &mut used).unwrap();
    let expected = pagura::Pixmap::from_premultiplied(
        u32::try_from(pixels.len() / 4).unwrap(),
        1,
        pixels,
        pagura::Limits::default(),
    )
    .unwrap()
    .to_rgb_over(pagura::Rgba8::WHITE);
    assert_eq!(actual, expected);
}

#[test]
fn final_tail_write_errors_are_returned() {
    struct Failed;
    impl Write for Failed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("sink failed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    write_rgb(&mut Failed, &[1, 2, 3, 255], &mut rgb, &mut used).unwrap();
    let error = drain_rgb(&mut Failed, &rgb, &mut used).unwrap_err();
    assert_eq!(error.to_string(), "sink failed");
}

#[derive(Default)]
struct Counted {
    bytes: Vec<u8>,
    writes: Vec<usize>,
}

impl Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn pixels(count: usize) -> Vec<u8> {
    [[1, 2, 3, 255], [101, 102, 103, 255], [254, 0, 127, 255]]
        .into_iter()
        .cycle()
        .take(count)
        .flatten()
        .collect()
}

fn rgb_bytes(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| pixel[..3].iter().copied())
        .collect()
}

#[test]
fn small_bands_share_complete_chunks_and_only_the_final_tail_is_drained() {
    let rgba = pixels(2 * RGB_CHUNK_BYTES / 3 + 5);
    let mut out = Counted::default();
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    for band in rgba.chunks(37 * 4) {
        write_rgb(&mut out, band, &mut rgb, &mut used).unwrap();
    }
    assert_eq!(out.writes, [RGB_CHUNK_BYTES, RGB_CHUNK_BYTES]);
    assert_eq!(used, 15);
    drain_rgb(&mut out, &rgb, &mut used).unwrap();
    assert_eq!(out.writes, [RGB_CHUNK_BYTES, RGB_CHUNK_BYTES, 15]);
    assert_eq!(out.bytes, rgb_bytes(&rgba));
    assert_eq!(used, 0);
    drain_rgb(&mut out, &rgb, &mut used).unwrap();
    assert_eq!(out.writes.len(), 3);
}

#[test]
fn interrupted_and_partial_writes_keep_all_rgb_bytes_in_order() {
    #[derive(Default)]
    struct Partial {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl Write for Partial {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 || self.calls == 4 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = bytes.len().min(997);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let rgba = pixels(RGB_CHUNK_BYTES / 3 + 17);
    let mut out = Partial::default();
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    for band in rgba.chunks(113 * 4) {
        write_rgb(&mut out, band, &mut rgb, &mut used).unwrap();
    }
    drain_rgb(&mut out, &rgb, &mut used).unwrap();
    assert_eq!(out.bytes, rgb_bytes(&rgba));
    assert!(out.calls > 4);
}

#[test]
fn a_chunk_error_stops_encoding_and_dropping_the_tail_never_retries() {
    #[derive(Default)]
    struct Failed {
        bytes: Vec<u8>,
        calls: usize,
    }
    impl Write for Failed {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                self.bytes.extend_from_slice(&bytes[..3]);
                return Ok(3);
            }
            Err(io::Error::other("sink failed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Failed::default();
    {
        #[expect(
            clippy::large_stack_arrays,
            reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
        )]
        let mut rgb = [0; RGB_CHUNK_BYTES];
        let mut used = 0;
        let error = write_rgb(
            &mut out,
            &pixels(2 * RGB_CHUNK_BYTES / 3 + 5),
            &mut rgb,
            &mut used,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "sink failed");
        assert_eq!(used, RGB_CHUNK_BYTES, "no second chunk was encoded");
    }
    assert_eq!(out.calls, 2);
    assert_eq!(out.bytes, [1, 2, 3]);
}

#[test]
fn empty_encoding_or_drain_does_not_call_the_sink() {
    let mut out = Counted::default();
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    write_rgb(&mut out, &[], &mut rgb, &mut used).unwrap();
    drain_rgb(&mut out, &rgb, &mut used).unwrap();
    assert_eq!(out.writes, []);
}

#[test]
fn inspect_reports_counts_and_diagnostics_as_one_json_line() {
    let trailing = source("classic", "trailing-bytes.pdf", Recovery::Repair);
    let line = output(&Command::Inspect(trailing.clone())).unwrap();
    assert!(line.starts_with("{\"schema\":2,\"pages\":1,"), "{line}");
    assert!(
        line.ends_with(",\"repairs\":[\"file.trailing-bytes\"],\"information\":[]}\n"),
        "{line}"
    );
    let strict = Source {
        recovery: Recovery::Strict,
        ..trailing
    };
    let Err(Failure::Engine(error)) = output(&Command::Inspect(strict)) else {
        panic!("strict inspect accepted trailing bytes");
    };
    assert_eq!(
        error.as_error().unwrap().repair.map(pagura::Repair::as_str),
        Some("file.trailing-bytes")
    );
}

#[test]
fn text_rejects_a_page_outside_the_document_before_printing() {
    let command = Command::Text {
        source: source("text", "extract-tj.pdf", Recovery::Repair),
        page: Some(1),
    };
    let Err(Failure::Request(message)) = output(&command) else {
        panic!("page 1 of a one-page document");
    };
    assert_eq!(message, "page 1 outside the document's 1 pages");
}

#[test]
fn help_version_and_licenses_print_to_the_output() {
    assert!(
        output(&Command::Help)
            .unwrap()
            .contains("Exit status: 0 on")
    );
    assert!(output(&Command::Version).unwrap().starts_with("pagura "));
    let licenses = output(&Command::Licenses).unwrap();
    for (subject, _) in pagura::notices() {
        assert!(licenses.contains(subject), "{subject}");
    }
}
