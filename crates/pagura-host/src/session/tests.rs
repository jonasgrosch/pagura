use super::*;
use pagura_testkit::fixture;

fn frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(payload.len() + 5)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    bytes.push(kind);
    bytes.extend(id.to_le_bytes());
    bytes.extend(payload);
    bytes
}
fn hello() -> Vec<u8> {
    frame(1, 1, &1_u32.to_le_bytes())
}
fn open(id: u32, file: &str, strict: bool) -> Vec<u8> {
    let pdf = fixture("classic", file);
    let mut payload = vec![u8::from(!strict), 0, 0, 0, 0];
    payload.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
    payload.extend(pdf);
    frame(2, id, &payload)
}
fn run(input: &[u8], limits: Limits) -> (SessionEnd, Vec<Vec<u8>>) {
    let mut bytes = Vec::new();
    let end = Session::new(limits)
        .serve(&mut &*input, &mut bytes)
        .unwrap();
    let mut frames = Vec::new();
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        let size = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
        frames.push(remaining[4..4 + size].to_vec());
        remaining = &remaining[4 + size..];
    }
    (end, frames)
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
    input.extend(frame(5, 4, &1_u32.to_le_bytes()));
    input.extend(frame(6, 5, &1_u32.to_le_bytes()));
    input.extend(frame(5, 6, &1_u32.to_le_bytes())); // stale handle
    input.extend(open(7, "blank.pdf", true)); // handle 2
    input.extend(frame(3, 8, &[2, 0, 0, 0, 0, 0, 0, 0]));
    input.extend(frame(3, 9, &[2, 0, 0, 0, 255, 255, 255, 255]));
    let (end, responses) = run(&input, Limits::default());
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
    let mut input = hello();
    input.extend(open(2, "blank.pdf", true));
    input.extend(open(3, "blank.pdf", true));
    let limits = Limits {
        file_bytes: fixture("classic", "blank.pdf").len(),
        ..Limits::default()
    };
    let (_, responses) = run(&input, limits);
    assert_eq!(responses[1][0], 130);
    assert_eq!(responses[2][5], 2);
}

#[test]
fn bad_render_arguments_are_operational_and_never_return_pixels() {
    let mut input = hello();
    input.extend(open(2, "rectangle.pdf", true));
    for (id, scale, dimensions) in [
        (3, f64::NAN, [0_u32, 0, 1, 1]),
        (4, 1.0, [0, 0, 0, 1]),
        (5, 1.0, [u32::MAX, 0, 1, 1]),
        (6, 1.0, [0, 0, u32::MAX, u32::MAX]),
    ] {
        let mut payload = vec![1, 0, 0, 0, 0, 0, 0, 0];
        payload.extend(scale.to_le_bytes());
        for dimension in dimensions {
            payload.extend(dimension.to_le_bytes());
        }
        input.extend(frame(4, id, &payload));
    }
    input.extend(frame(5, 7, &1_u32.to_le_bytes()));
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
        let mut payload = 1_u32.to_le_bytes().to_vec();
        payload.extend(page.to_le_bytes());
        input.extend(frame(3, id, &payload));
    }
    let tiles = [(0_u32, 0_u32, 64_u32, 64_u32), (24, 16, 32, 32)];
    for (tile, id) in tiles.iter().zip(5..7) {
        let mut payload = vec![1, 0, 0, 0, 1, 0, 0, 0];
        payload.extend(1_f64.to_le_bytes());
        for dimension in [tile.0, tile.1, tile.2, tile.3] {
            payload.extend(dimension.to_le_bytes());
        }
        input.extend(frame(4, id, &payload));
    }
    let (end, responses) = run(&input, Limits::default());
    assert_eq!(end, SessionEnd::Eof);
    assert_eq!(&responses[1][9..13], &2_u32.to_le_bytes());
    let mut geometry = Vec::new();
    for coordinate in [0_f64, 0., 64., 64.] {
        geometry.extend(coordinate.to_le_bytes());
    }
    geometry.extend(0_u16.to_le_bytes());
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend(64_u32.to_le_bytes());
    geometry.extend(64_u32.to_le_bytes());
    for response in &responses[2..4] {
        assert_eq!(response[0], 131);
        assert_eq!(&response[5..], geometry);
    }
    for ((x, y, width, height), response) in tiles.into_iter().zip(&responses[4..6]) {
        assert_eq!(response[0], 132);
        assert_eq!(&response[5..9], &width.to_le_bytes());
        assert_eq!(&response[9..13], &height.to_le_bytes());
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
fn generations_skip_before_compilation_and_cached_tiles_share_one_list() {
    let mut session = Session::default();
    session
        .order(1, &Request::Hello(protocol::VERSION_1_1))
        .unwrap();
    let pdf = fixture("classic", "rectangle.pdf");
    session
        .execute(Request::Open {
            bytes: &pdf,
            recovery: pagura::Recovery::Strict,
            password: &[],
        })
        .unwrap();
    session
        .execute(Request::Generation {
            document: 1,
            generation: 2,
        })
        .unwrap();
    let stale = Request::GenerationTile {
        generation: 1,
        document: 1,
        page: u32::MAX,
        scale: f64::NAN,
        tile: pagura::Tile {
            x: u32::MAX,
            y: 0,
            width: 1,
            height: 1,
        },
    };
    assert_eq!(session.execute(stale).unwrap(), Response::Skipped(1));
    assert!(session.cache.get((1, 0)).is_none());
    let current = Request::GenerationTile {
        generation: 2,
        document: 1,
        page: 0,
        scale: 1.0,
        tile: pagura::Tile {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        },
    };
    assert!(matches!(
        session.execute(current).unwrap(),
        Response::GenerationTile { .. }
    ));
    let invalid_scale = Request::GenerationTile {
        generation: 2,
        document: 1,
        page: 0,
        scale: f64::NAN,
        tile: pagura::Tile {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
    };
    assert_eq!(
        session.execute(invalid_scale).unwrap_err().position,
        Position::Buffer(17)
    );
    let first = session.cache.get((1, 0)).unwrap();
    session.execute(current).unwrap();
    assert!(Arc::ptr_eq(&first, &session.cache.get((1, 0)).unwrap()));
    session.execute(Request::Close(1)).unwrap();
    assert!(session.cache.get((1, 0)).is_none());
    let mut original = Session::default();
    original
        .order(1, &Request::Hello(protocol::VERSION))
        .unwrap();
    assert!(
        original
            .execute(Request::Generation {
                document: 1,
                generation: 1
            })
            .is_err()
    );
}

#[test]
fn retention_and_supervisor_cancellation_release_on_close() {
    let control = crate::Cancellation::default();
    let mut session = Session::default().with_cancellation(control.clone());
    session
        .order(1, &Request::Hello(protocol::VERSION_1_1))
        .unwrap();
    let pdf = fixture("classic", "rectangle.pdf");
    session
        .execute(Request::Open {
            bytes: &pdf,
            recovery: pagura::Recovery::Strict,
            password: &[],
        })
        .unwrap();
    session
        .execute(Request::Generation {
            document: 1,
            generation: 1,
        })
        .unwrap();
    let tile = Request::GenerationTile {
        generation: 1,
        document: 1,
        page: 0,
        scale: 1.,
        tile: pagura::Tile {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
    };
    std::thread::scope(|scope| {
        scope.spawn(|| control.cancel()).join().unwrap();
    });
    assert_eq!(session.execute(tile).unwrap(), Response::Skipped(1));
    assert_eq!(session.stats().cache_bytes, 0);
    control.reset();
    assert!(matches!(
        session.execute(tile).unwrap(),
        Response::GenerationTile { .. }
    ));
    assert_eq!(
        session.stats(),
        protocol::SessionStats {
            source_bytes: pdf.len() as u64,
            cache_bytes: 4608,
            documents: 1,
            lists: 1
        }
    );
    session.execute(Request::Close(1)).unwrap();
    assert_eq!(
        session.stats(),
        protocol::SessionStats {
            source_bytes: 0,
            cache_bytes: 0,
            documents: 0,
            lists: 0
        }
    );
    let bytes = protocol::encode(7, &Response::Stats(session.stats()), 1024).unwrap();
    assert_eq!(&bytes[..9], &[29, 0, 0, 0, 137, 7, 0, 0, 0]);
    assert_eq!(&bytes[9..], &[0; 24]);
    assert!(protocol::decode(&[9, 1, 0, 0, 0], Limits::default()).is_ok());
    assert!(protocol::decode(&[9, 1, 0, 0, 0, 0], Limits::default()).is_err());
    let mut base = Session::default();
    base.order(1, &Request::Hello(protocol::VERSION)).unwrap();
    assert!(base.execute(Request::Stats).is_err());
}

#[test]
fn classic_tiles_match_independent_ppm_and_page_geometry() {
    let mut input = hello();
    input.extend(open(2, "rectangle.pdf", true));
    input.extend(frame(3, 3, &[1, 0, 0, 0, 0, 0, 0, 0]));
    let mut payload = vec![1, 0, 0, 0, 0, 0, 0, 0];
    payload.extend(1_f64.to_le_bytes());
    for value in [0_u32, 0, 64, 64] {
        payload.extend(value.to_le_bytes());
    }
    input.extend(frame(4, 4, &payload));
    let (_, responses) = run(&input, Limits::default());
    assert_eq!(responses[2][0], 131);
    let mut geometry = Vec::new();
    for coordinate in [0_f64, 0., 64., 64.] {
        geometry.extend(coordinate.to_le_bytes());
    }
    geometry.extend(0_u16.to_le_bytes());
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend(64_u32.to_le_bytes());
    geometry.extend(64_u32.to_le_bytes());
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
