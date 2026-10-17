//! `host`: framed requests through [`Session`], exactly as standard input
//! delivers them (ADR 0005, `crates/pagura-host/PROTOCOL.md`): hello, open,
//! page_info, viewport generations with a current and a stale tile, a v1
//! render_tile, diagnostics and close. The session answers every valid
//! request once, in order, with well-formed frames.
//!
//! Layout: a flags byte (bit 0: protocol 1.1, bit 1: strict recovery,
//! bit 2: raw frames follow the PDF after [`super::wrapper::SEPARATOR`] and
//! are sent last), the page index, the scale in sixteenths minus one, the
//! tile's x, y, width and height in pixels, then the PDF bytes.

use super::wrapper;
use pagura_host::protocol::{VERSION, VERSION_1_1};
use pagura_host::{Session, SessionEnd};
use std::io::Cursor;

const ERROR: u8 = 255;

/// The parameters of the tile requests.
#[derive(Debug, Clone, Copy)]
struct TileRequest {
    page: u32,
    scale: f64,
    rect: [u32; 4],
}

/// Serves the generated frames and checks the responses.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&[flags, page, scale, x, y, width, height], rest)) = data.split_first_chunk::<7>()
    else {
        return;
    };
    let (pdf, tail) = match (flags & 4 != 0, wrapper::find(rest, wrapper::SEPARATOR)) {
        (true, Some(at)) => {
            let (pdf, tail) = rest.split_at(at);
            (
                pdf,
                tail.get(wrapper::SEPARATOR.len()..).unwrap_or_default(),
            )
        }
        _ => (rest, &[][..]),
    };
    let tile = TileRequest {
        page: u32::from(page),
        scale: (f64::from(scale) + 1.0) / 16.0,
        rect: [x, y, width, height].map(u32::from),
    };
    let generations = flags & 1 != 0;
    let mut frames = Frames::default();
    frames.push(
        1,
        &(if generations { VERSION_1_1 } else { VERSION }).to_le_bytes(),
    );
    let mut open = vec![u8::from(flags & 2 == 0)];
    open.extend_from_slice(&0_u32.to_le_bytes());
    open.extend_from_slice(&u32::try_from(pdf.len()).unwrap_or(u32::MAX).to_le_bytes());
    open.extend_from_slice(pdf);
    frames.push(2, &open);
    frames.push(3, &words(&[1, tile.page]));
    if generations {
        frames.push(7, &words(&[1, 2]));
        frames.push(8, &generation_tile(2, tile));
        frames.push(7, &words(&[1, 3]));
        frames.push(8, &generation_tile(2, tile));
    }
    frames.push(4, &render_tile(tile));
    frames.push(5, &words(&[1]));
    frames.push(6, &words(&[1]));
    let requests = frames.kinds.clone();
    let mut input = frames.bytes;
    input.extend_from_slice(tail);

    let session = Session::new(super::limits());
    let mut output = Vec::new();
    let end = session.serve(&mut Cursor::new(input), &mut output);
    assert!(end.is_ok(), "in-memory transport failed: {end:?}");
    let responses = responses(&output);
    assert!(
        responses.len() >= requests.len(),
        "{} responses to {} valid requests",
        responses.len(),
        requests.len()
    );
    if tail.is_empty() {
        assert_eq!(
            end.ok(),
            Some(SessionEnd::Eof),
            "valid frames ended the session"
        );
        assert_eq!(
            responses.len(),
            requests.len(),
            "responses without requests"
        );
    }
    for (index, (&kind, response)) in requests.iter().zip(&responses).enumerate() {
        assert_eq!(
            response.id,
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            "response out of order"
        );
        assert!(
            response.kind == kind | 0x80 || response.kind == ERROR,
            "kind {} answers request kind {kind}",
            response.kind
        );
    }
    check_tiles(&requests, &responses, tile);
}

/// Pixels of the same tile agree between the v1.1 and v1 requests, and the
/// tile requested after a newer generation was selected is skipped before
/// rendering.
fn check_tiles(requests: &[u8], responses: &[Response<'_>], tile: TileRequest) {
    let v1 = responses
        .get(requests.len().saturating_sub(3))
        .filter(|r| r.kind == 132)
        .and_then(|r| tile_pixels(r, 0, tile));
    if requests.len() == 10 {
        let current = responses.get(4).filter(|r| r.kind == 136).and_then(|r| {
            assert_eq!(
                r.payload.first(),
                Some(&1),
                "the current generation was skipped"
            );
            tile_pixels(r, 5, tile)
        });
        if let (Some(current), Some(v1)) = (current, v1) {
            assert!(current == v1, "the same tile rendered differently");
        }
        if responses.get(5).is_some_and(|r| r.kind == 135) {
            let stale = responses.get(6).map(|r| (r.kind, r.payload));
            assert_eq!(
                stale,
                Some((136, &[0, 2, 0, 0, 0][..])),
                "a stale tile was not skipped"
            );
        }
    }
}

/// The pixels of a tile response whose dimensions follow `skip` bytes of
/// payload; they must be the requested size.
fn tile_pixels<'a>(response: &Response<'a>, skip: usize, tile: TileRequest) -> Option<&'a [u8]> {
    let (size, rgba) = response.payload.get(skip..)?.split_first_chunk::<8>()?;
    let [w0, w1, w2, w3, h0, h1, h2, h3] = *size;
    let size = [
        u32::from_le_bytes([w0, w1, w2, w3]),
        u32::from_le_bytes([h0, h1, h2, h3]),
    ];
    assert_eq!(size, [tile.rect[2], tile.rect[3]], "tile size");
    let expected = u64::from(size[0]) * u64::from(size[1]) * 4;
    assert_eq!(u64::try_from(rgba.len()).ok(), Some(expected), "tile bytes");
    Some(rgba)
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn render_tile(tile: TileRequest) -> Vec<u8> {
    let mut payload = words(&[1, tile.page]);
    payload.extend_from_slice(&tile.scale.to_le_bytes());
    payload.extend_from_slice(&words(&tile.rect));
    payload
}

fn generation_tile(generation: u32, tile: TileRequest) -> Vec<u8> {
    let mut payload = generation.to_le_bytes().to_vec();
    payload.extend_from_slice(&render_tile(tile));
    payload
}

/// Request frames with identifiers 1, 2, …
#[derive(Debug, Default)]
struct Frames {
    bytes: Vec<u8>,
    kinds: Vec<u8>,
}

impl Frames {
    fn push(&mut self, kind: u8, payload: &[u8]) {
        self.kinds.push(kind);
        let id = u32::try_from(self.kinds.len()).unwrap_or(u32::MAX);
        let length = u32::try_from(payload.len() + 5).unwrap_or(u32::MAX);
        self.bytes.extend_from_slice(&length.to_le_bytes());
        self.bytes.push(kind);
        self.bytes.extend_from_slice(&id.to_le_bytes());
        self.bytes.extend_from_slice(payload);
    }
}

#[derive(Debug)]
struct Response<'a> {
    kind: u8,
    id: u32,
    payload: &'a [u8],
}

/// Splits the output into frames; it must consist of whole frames.
fn responses(mut output: &[u8]) -> Vec<Response<'_>> {
    let mut responses = Vec::new();
    while !output.is_empty() {
        let frame = output.split_first_chunk::<4>().and_then(|(length, rest)| {
            let length = usize::try_from(u32::from_le_bytes(*length)).ok()?;
            let (body, rest) = rest.split_at_checked(length)?;
            let (&kind, body) = body.split_first()?;
            let (id, payload) = body.split_first_chunk::<4>()?;
            let id = u32::from_le_bytes(*id);
            Some((Response { kind, id, payload }, rest))
        });
        assert!(frame.is_some(), "the output ends inside a frame");
        let Some((response, rest)) = frame else {
            break;
        };
        responses.push(response);
        output = rest;
    }
    responses
}
