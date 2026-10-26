//! Session behavior against literal wire oracles; numbers are written out
//! here rather than taken from the code tables they check.

use super::*;
use crate::SessionLimits;
use crate::protocol::RequestKind;
use pagura::{Limits, Recovery};
use pagura_testkit::{fixture, frame, split_frames};
use std::sync::Arc;

fn hello() -> Vec<u8> {
    frame(1, 1, &1_u32.to_le_bytes())
}

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn open(id: u32, file: &str, strict: bool) -> Vec<u8> {
    let pdf = fixture("classic", file);
    let mut payload = vec![u8::from(!strict), 0, 0, 0, 0];
    payload.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
    payload.extend(pdf);
    frame(2, id, &payload)
}

/// `u32 document, page | f64 scale | u32 x, y, width, height`.
fn tile(document: u32, page: u32, scale: f64, rect: [u32; 4]) -> Vec<u8> {
    let mut payload = words(&[document, page]);
    payload.extend(scale.to_le_bytes());
    payload.extend(words(&rect));
    payload
}

fn generation_tile(generation: u32, scale: f64, page: u32, rect: [u32; 4]) -> Vec<u8> {
    let mut payload = words(&[generation]);
    payload.extend(tile(1, page, scale, rect));
    payload
}

fn run(input: &[u8], config: impl Into<SessionConfig>) -> (SessionEnd, Vec<Vec<u8>>) {
    let mut bytes = Vec::new();
    let end = Session::new(config.into())
        .serve(input, &mut bytes)
        .unwrap();
    let frames = split_frames(&bytes)
        .iter()
        .map(|f| f.body().to_vec())
        .collect();
    (end, frames)
}

/// A request body of `kind`, as `serve` hands it to the decoder.
fn body(kind: u8, payload: &[u8]) -> Vec<u8> {
    frame(kind, 1, payload)[4..].to_vec()
}

fn request(body: &[u8]) -> Request<'_> {
    protocol::decode(body, &Limits::default()).unwrap().request
}

/// A session after a hello for `version`.
fn ready(version: Version) -> Session {
    Session {
        state: State::Ready(version),
        ..Session::default()
    }
}

fn open_rectangle(session: &mut Session) -> Vec<u8> {
    let pdf = fixture("classic", "rectangle.pdf");
    session
        .execute(Request::Open {
            bytes: &pdf,
            recovery: Recovery::Strict,
            password: &[],
        })
        .unwrap();
    pdf
}

#[test]
fn malformed_and_out_of_order_frames_are_terminal() {
    // host-frame-bounds and host-session-order: the following valid hello
    // must never execute after a terminal failure.
    let limits = Limits {
        file_bytes: 1024,
        string_bytes: 8,
        decoded_bytes: 1024,
        pixels: 16,
        ..Limits::default()
    };
    for mut bad in [
        vec![0; 4],
        vec![255; 4],
        frame(0, 2, &[]),
        frame(6, 2, &[0; 4]),
        frame(1, 0, &[1, 0, 0, 0]),
        frame(1, 2, &[2, 0, 0, 0]),
    ] {
        bad.extend(hello());
        let (end, output) = run(&bad, limits);
        assert_eq!(end, SessionEnd::ProtocolError);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0][0], 255);
    }
    for last in [hello(), frame(5, 1, &[0; 4]), frame(5, 0, &[0; 4])] {
        let mut input = hello();
        input.extend(last);
        input.extend(frame(5, 3, &[0; 4]));
        let (end, output) = run(&input, limits);
        assert_eq!(end, SessionEnd::ProtocolError);
        assert_eq!(output.len(), 2);
    }
    let valid = hello();
    for cut in 1..valid.len() {
        let (end, output) = run(&valid[..cut], limits);
        assert_eq!(end, SessionEnd::ProtocolError);
        assert_eq!(output[0][0], 255);
    }
    assert_eq!(run(&[], limits), (SessionEnd::Eof, vec![]));
}

#[test]
fn deterministic_fuzz_frames_never_panic() {
    let limits = Limits {
        file_bytes: 128,
        string_bytes: 128,
        decoded_bytes: 128,
        pixels: 1,
        ..Limits::default()
    };
    let mut state = 0x9384_ace1_u32;
    for size in 0..2048 {
        let mut input = Vec::new();
        for _ in 0..size % 257 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            input.push(state.to_le_bytes()[0]);
        }
        // Half are length-valid to reach the payload decoder.
        if size % 2 == 0 {
            input = frame(u8::try_from(size % 256).unwrap(), 2, &input);
        }
        input.splice(0..0, hello());
        let _ = run(&input, limits);
    }
}

#[test]
fn handles_budgets_errors_and_repairs_preserve_the_session() {
    let mut input = hello();
    input.extend(open(2, "trailing-bytes.pdf", true)); // strict open fails
    input.extend(open(3, "trailing-bytes.pdf", false)); // successful handle 1
    input.extend(frame(5, 4, &words(&[1])));
    input.extend(frame(6, 5, &words(&[1])));
    input.extend(frame(5, 6, &words(&[1]))); // stale handle
    input.extend(open(7, "blank.pdf", true)); // handle 2
    input.extend(frame(3, 8, &words(&[2, 0])));
    input.extend(frame(3, 9, &words(&[2, u32::MAX])));
    let (end, responses) = run(&input, SessionConfig::default());
    assert_eq!(end, SessionEnd::Eof);
    assert_eq!(
        responses.iter().map(|r| r[0]).collect::<Vec<_>>(),
        [129, 255, 130, 133, 134, 255, 130, 131, 255]
    );
    // Frozen v1: the strict error has no feature and a refused-repair tag.
    assert_eq!(&responses[1][6..10], &[0; 4]);
    assert_eq!(&responses[1][10..14], &19_u32.to_le_bytes());
    assert_eq!(&responses[1][14..33], b"file.trailing-bytes");
    assert!(
        responses[3]
            .windows(19)
            .any(|s| s == b"file.trailing-bytes")
    );
    assert_eq!(&responses[6][5..9], &2_u32.to_le_bytes());
    // The stale handle and the missing page are reported at their fields.
    assert_eq!(
        &responses[5][responses[5].len() - 9..],
        &[3, 5, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        &responses[8][responses[8].len() - 9..],
        &[3, 9, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn session_budgets_are_independent_of_engine_limits() {
    // Two opens of the same file: the second exceeds the session's source
    // budget, although each file fits the engine's file_bytes.
    let blank = fixture("classic", "blank.pdf").len();
    let mut input = hello();
    input.extend(open(2, "blank.pdf", true));
    input.extend(open(3, "blank.pdf", true));
    let config = SessionConfig::default().with_session(SessionLimits {
        total_source_bytes: blank,
        ..SessionLimits::default()
    });
    let (_, responses) = run(&input, config);
    assert_eq!(responses[1][0], 130);
    assert_eq!((responses[2][0], responses[2][5]), (255, 2));
    // One open document at a time: close frees the slot; handles still increase.
    input.extend(frame(6, 4, &words(&[1])));
    input.extend(open(5, "blank.pdf", true));
    let config = SessionConfig::default().with_session(SessionLimits {
        max_documents: 1,
        ..SessionLimits::default()
    });
    let (_, responses) = run(&input, config);
    assert_eq!(
        responses.iter().map(|r| r[0]).collect::<Vec<_>>(),
        [129, 130, 255, 134, 130]
    );
    assert_eq!(responses[2][5], 2);
    assert_eq!(&responses[4][5..9], &2_u32.to_le_bytes());
    // Engine limits no longer size the session: a tiny decoded_bytes still
    // admits the default caches.
    let engine = Limits {
        decoded_bytes: 1,
        document_bytes: 1,
        objects: 0,
        ..Limits::default()
    };
    assert_eq!(
        SessionConfig::from(engine).session,
        SessionLimits::default()
    );
}

#[test]
fn bad_render_arguments_are_operational_and_never_return_pixels() {
    let mut input = hello();
    input.extend(open(2, "rectangle.pdf", true));
    for (id, scale, rect) in [
        (3, f64::NAN, [0_u32, 0, 1, 1]),
        (4, 1.0, [0, 0, 0, 1]),
        (5, 1.0, [u32::MAX, 0, 1, 1]),
        (6, 1.0, [0, 0, u32::MAX, u32::MAX]),
    ] {
        input.extend(frame(4, id, &tile(1, 0, scale, rect)));
    }
    input.extend(frame(5, 7, &words(&[1])));
    let limits = Limits {
        pixels: 64,
        ..Limits::default()
    };
    let (end, responses) = run(&input, limits);
    assert_eq!(end, SessionEnd::Eof);
    assert_eq!(
        responses.iter().map(|r| r[0]).collect::<Vec<_>>(),
        [129, 130, 255, 255, 255, 255, 133]
    );
    assert_eq!(responses[2][5], 7); // InvalidArgument for NaN.
    assert_eq!(responses[3][5], 7); // Empty tile.
    assert_eq!(responses[4][5], 7); // Overflowing tile edge.
    assert_eq!(responses[5][5], 2); // Limit before pixel allocation.
}

#[test]
fn two_pages_have_independent_geometry_and_second_page_pixels() {
    // host-multipage-navigation: page 2's blue square covers device
    // x=[32,64), y=[0,32) on the 64x64 top-left-origin surface.
    let mut input = hello();
    input.extend(open(2, "two-pages.pdf", true));
    for (id, page) in [(3, 0_u32), (4, 1)] {
        input.extend(frame(3, id, &words(&[1, page])));
    }
    let tiles = [[0_u32, 0, 64, 64], [24, 16, 32, 32]];
    for (rect, id) in tiles.iter().zip(5..7) {
        input.extend(frame(4, id, &tile(1, 1, 1.0, *rect)));
    }
    let (end, responses) = run(&input, SessionConfig::default());
    assert_eq!(end, SessionEnd::Eof);
    assert_eq!(&responses[1][9..13], &2_u32.to_le_bytes());
    let mut geometry = Vec::new();
    for coordinate in [0_f64, 0., 64., 64.] {
        geometry.extend(coordinate.to_le_bytes());
    }
    geometry.extend(0_u16.to_le_bytes());
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend(words(&[64, 64]));
    for response in &responses[2..4] {
        assert_eq!(response[0], 131);
        assert_eq!(&response[5..], geometry);
    }
    for ([x, y, width, height], response) in tiles.into_iter().zip(&responses[4..6]) {
        assert_eq!(response[0], 132);
        assert_eq!(&response[5..13], words(&[width, height]));
        let mut expected = Vec::new();
        for row in y..y + height {
            for column in x..x + width {
                expected.extend(if column >= 32 && row < 32 {
                    [0, 0, 255, 255]
                } else {
                    [255; 4]
                });
            }
        }
        assert_eq!(&response[13..], expected);
    }
}

#[test]
fn generations_skip_before_recording_and_tiles_share_one_scene() {
    let mut session = ready(Version::V1_1);
    open_rectangle(&mut session);
    session.execute(request(&body(7, &words(&[1, 2])))).unwrap();
    // Generation 1 is stale: no page lookup, scale check or recording.
    let stale = body(
        8,
        &generation_tile(1, f64::NAN, u32::MAX, [u32::MAX, 0, 1, 1]),
    );
    assert_eq!(
        session.execute(request(&stale)).unwrap(),
        Response::Skipped(1)
    );
    assert!(session.documents.stored_scene(1, 0).is_none());
    let current = body(8, &generation_tile(2, 1.0, 0, [0, 0, 32, 32]));
    assert!(matches!(
        session.execute(request(&current)).unwrap(),
        Response::GenerationTile { generation: 2, .. }
    ));
    let invalid_scale = body(8, &generation_tile(2, f64::NAN, 0, [0, 0, 1, 1]));
    assert_eq!(
        session
            .execute(request(&invalid_scale))
            .unwrap_err()
            .position,
        Position::Buffer(17)
    );
    let first = session.documents.stored_scene(1, 0).unwrap();
    session.execute(request(&current)).unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &session.documents.stored_scene(1, 0).unwrap()
    ));
    drop(first);
    session.execute(request(&body(6, &words(&[1])))).unwrap();
    assert!(session.documents.stored_scene(1, 0).is_none());
    let mut original = ready(Version::V1);
    assert!(
        original
            .execute(request(&body(7, &words(&[1, 1]))))
            .is_err()
    );
}

#[test]
fn retention_and_supervisor_cancellation_release_on_close() {
    let control = crate::Cancellation::default();
    let mut session = ready(Version::V1_1).with_cancellation(control.clone());
    let pdf = open_rectangle(&mut session);
    session.execute(request(&body(7, &words(&[1, 1])))).unwrap();
    let tile = body(8, &generation_tile(1, 1.0, 0, [0, 0, 64, 64]));
    std::thread::scope(|scope| {
        scope.spawn(|| control.cancel()).join().unwrap();
    });
    assert_eq!(
        session.execute(request(&tile)).unwrap(),
        Response::Skipped(1)
    );
    assert_eq!(session.stats().cache_bytes, 0);
    control.reset();
    assert!(matches!(
        session.execute(request(&tile)).unwrap(),
        Response::GenerationTile { .. }
    ));
    // The store keeps the page's scene and nothing else for this page.
    let scene = session.documents.stored_scene(1, 0).unwrap();
    assert_eq!(
        session.stats(),
        SessionStats {
            source_bytes: u64::try_from(pdf.len()).unwrap(),
            cache_bytes: u64::try_from(scene.bytes).unwrap(),
            documents: 1,
            lists: 1
        }
    );
    drop(scene);
    session.execute(request(&body(6, &words(&[1])))).unwrap();
    assert_eq!(
        session.stats(),
        SessionStats {
            source_bytes: 0,
            cache_bytes: 0,
            documents: 0,
            lists: 0
        }
    );
    let bytes = protocol::encode(7, &Response::Stats(session.stats()), 1024).unwrap();
    assert_eq!(&bytes[..9], &[29, 0, 0, 0, 137, 7, 0, 0, 0]);
    assert_eq!(&bytes[9..], &[0; 24]);
    assert!(protocol::decode(&[9, 1, 0, 0, 0], &Limits::default()).is_ok());
    assert!(protocol::decode(&[9, 1, 0, 0, 0, 0], &Limits::default()).is_err());
    let mut base = ready(Version::V1);
    assert!(base.execute(Request::Stats).is_err());
}

#[test]
fn every_request_is_gated_by_the_version_that_added_it() {
    // Kinds 1-6 in v1, 7-9 in 1.1, 10-15 in 1.2, 16 in 1.3, 17 in 1.4,
    // 18-19 in 1.5, 20 in 1.6, 21 in 1.7, 22-25 in 1.8, 26 in 1.9, 27 in 1.10,
    // 28-30 in 1.11 and 31-40 in 1.12.
    let since = |kind: u8| match kind {
        1..=6 => 1,
        7..=9 => 0x0001_0001,
        10..=15 => 0x0001_0002,
        16 => 0x0001_0003,
        17 => 0x0001_0004,
        18 | 19 => 0x0001_0005,
        20 => 0x0001_0006,
        21 => 0x0001_0007,
        22..=25 => 0x0001_0008,
        26 => 0x0001_0009,
        27 => 0x0001_000a,
        28..=30 => 0x0001_000b,
        31..=40 => 0x0001_000c,
        _ => panic!("no independent version oracle for request {kind}"),
    };
    for &version in Version::ALL {
        let session = ready(version);
        for &kind in RequestKind::ALL {
            let allowed = session.require(kind.since()).is_ok();
            assert_eq!(
                allowed,
                version.code() >= since(kind.code()),
                "{kind:?} in {version:?}"
            );
        }
    }
    let error = ready(Version::V1_3)
        .execute(request(&body(17, &words(&[1]))))
        .unwrap_err();
    assert_eq!(
        error.as_error().unwrap().kind,
        pagura::ErrorKind::InvalidArgument
    );
}

#[test]
fn classic_tiles_match_independent_ppm_and_page_geometry() {
    let mut input = hello();
    input.extend(open(2, "rectangle.pdf", true));
    input.extend(frame(3, 3, &words(&[1, 0])));
    input.extend(frame(4, 4, &tile(1, 0, 1.0, [0, 0, 64, 64])));
    let (_, responses) = run(&input, SessionConfig::default());
    assert_eq!(responses[2][0], 131);
    let mut geometry = Vec::new();
    for coordinate in [0_f64, 0., 64., 64.] {
        geometry.extend(coordinate.to_le_bytes());
    }
    geometry.extend(0_u16.to_le_bytes());
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend(words(&[64, 64]));
    assert_eq!(&responses[2][5..], geometry);
    assert_eq!(responses[3][0], 132);
    let ppm = fixture("classic", "rectangle.ppm");
    let expected: Vec<_> = ppm[b"P6\n64 64\n255\n".len()..]
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    assert_eq!(&responses[3][13..], expected);
}
