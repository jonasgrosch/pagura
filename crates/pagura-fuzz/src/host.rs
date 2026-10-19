//! `host`: framed requests through [`Session`], exactly as standard input
//! delivers them (ADR 0005, `crates/pagura-host/PROTOCOL.md`): hello, open,
//! page_info, viewport generations with a current and a stale tile, a v1
//! render_tile, the v1.2 reading requests, diagnostics and close. The session
//! answers every valid request once, in order, and every response, errors
//! included, has its documented payload layout.
//!
//! Layout: a flags byte, the page index, the scale in sixteenths minus one,
//! the tile's x, y, width and height in pixels; with flag bit 3, the search
//! window's length in pages, its match offset and limit, its option bits and
//! the length of its query, then the query; then the PDF bytes. Flags:
//!
//! - bit 0: protocol 1.1 and its generation requests;
//! - bit 1: strict recovery;
//! - bit 2: raw frames follow the PDF after [`super::wrapper::SEPARATOR`]
//!   and are sent last;
//! - bit 3: protocol 1.2 and its reading requests on the tile's page: page
//!   text, a search over the fuzzed window and the same search in full,
//!   page links, outlines, page labels and both accounting requests, and
//!   after close the reading accounting and the page text once more;
//! - bit 4, with bit 3: a v1 handshake (or v1.1 with bit 0) instead, which
//!   must answer every reading request with an operational InvalidArgument
//!   error and keep serving.
//!
//! - bit 5: protocol 1.4 with typed diagnostics;
//! - bit 6: request typed diagnostics even with an older handshake, which
//!   must reject that request and keep serving;
//! - bit 7: protocol 1.3 when bit 5 is clear.
//!
//! An invalid query byte becomes U+FFFD, so that the generated frames stay
//! valid; raw frames reach the terminal protocol errors.

use super::wrapper;
use pagura_host::protocol::{
    ERROR_KIND, VERSION, VERSION_1_1, VERSION_1_2, VERSION_1_3, VERSION_1_4,
};
use pagura_host::{Session, SessionEnd};
use std::io::Cursor;

/// The Limit error category of kind 255.
const LIMIT: u8 = 2;
/// The InvalidArgument error category of kind 255.
const INVALID_ARGUMENT: u8 = 7;
/// The standard Named actions, sent by their exact names.
const NAMED: [&str; 4] = ["NextPage", "PrevPage", "FirstPage", "LastPage"];

/// The flags byte that starts an input, as named bits (see the module
/// documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(u8);

impl Flags {
    /// Protocol 1.1 and its generation requests.
    pub const GENERATIONS: Self = Self(1);
    /// Strict recovery.
    pub const STRICT: Self = Self(1 << 1);
    /// Raw frames follow the PDF after the separator and are sent last.
    pub const RAW_FRAMES: Self = Self(1 << 2);
    /// Protocol 1.2 and its reading requests; the input carries a search.
    pub const READING: Self = Self(1 << 3);
    /// With [`Self::READING`], a v1 (or v1.1) handshake instead.
    pub const OLDER: Self = Self(1 << 4);
    /// Protocol 1.4 with typed diagnostics.
    pub const TYPED_DIAGNOSTICS: Self = Self(1 << 5);
    /// Typed diagnostics requested even with an older handshake.
    pub const UNNEGOTIATED_DIAGNOSTICS: Self = Self(1 << 6);
    /// Protocol 1.3 unless [`Self::TYPED_DIAGNOSTICS`] is set.
    pub const VERSION_1_3: Self = Self(1 << 7);

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any bit of `other` is set.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The byte.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One input in this target's layout, built for tests and seeds.
#[derive(Debug, Clone)]
pub struct Input<'a> {
    /// What the session negotiates and requests.
    pub flags: Flags,
    /// The zero-based page of the tile and of the search window.
    pub page: u8,
    /// The scale in sixteenths, minus one.
    pub scale: u8,
    /// The tile's x, y, width and height in pixels.
    pub tile: [u8; 4],
    /// The search, present exactly with [`Flags::READING`].
    pub search: Option<Search<'a>>,
    /// The PDF file the session opens.
    pub pdf: &'a [u8],
}

/// The search window of an [`Input`].
#[derive(Debug, Clone, Copy)]
pub struct Search<'a> {
    /// The window's length in pages.
    pub pages: u8,
    /// The first match reported.
    pub offset: u8,
    /// The most matches reported.
    pub limit: u8,
    /// The protocol's search option bits.
    pub options: u8,
    /// At most 255 bytes, which need not be UTF-8.
    pub query: &'a [u8],
}

impl Input<'_> {
    /// The input's bytes; a query longer than 255 bytes is cut.
    pub fn to_bytes(&self) -> Vec<u8> {
        let [x, y, width, height] = self.tile;
        let mut bytes = vec![
            self.flags.bits(),
            self.page,
            self.scale,
            x,
            y,
            width,
            height,
        ];
        if let Some(search) = &self.search {
            let query = &search.query[..search.query.len().min(255)];
            let length = u8::try_from(query.len()).unwrap_or(u8::MAX);
            bytes.extend([
                search.pages,
                search.offset,
                search.limit,
                search.options,
                length,
            ]);
            bytes.extend_from_slice(query);
        }
        bytes.extend_from_slice(self.pdf);
        bytes
    }
}

/// The parameters of the tile requests.
#[derive(Debug, Clone, Copy)]
struct TileRequest {
    page: u32,
    scale: f64,
    rect: [u32; 4],
}

/// The fuzzed search window, starting on the tile's page.
#[derive(Debug, Clone)]
struct SearchRequest {
    start: u32,
    end: u32,
    offset: u32,
    limit: u32,
    options: u8,
    query: String,
}

impl SearchRequest {
    /// The window's length, offset, limit, options and query length, then
    /// the query, on `page`; and the rest of the input.
    fn read(page: u8, data: &[u8]) -> Option<(Self, &[u8])> {
        let (&[span, offset, limit, options, length], rest) = data.split_first_chunk::<5>()?;
        let (query, rest) = rest.split_at_checked(usize::from(length))?;
        let search = Self {
            start: u32::from(page),
            end: u32::from(page) + u32::from(span),
            offset: u32::from(offset),
            limit: u32::from(limit),
            options,
            query: String::from_utf8_lossy(query).into_owned(),
        };
        Some((search, rest))
    }
}

/// Indices of the requests whose responses are compared.
#[derive(Debug)]
struct Plan {
    open: usize,
    v1_tile: usize,
    /// The current tile, the newer generation and the stale tile.
    generations: Option<[usize; 3]>,
    reading: Option<Reading>,
    diagnostics: usize,
    close: usize,
}

#[derive(Debug)]
struct Reading {
    text: usize,
    search: usize,
    complete: usize,
    links: usize,
    outlines: usize,
    labels: usize,
    /// Kind 9, compared with the reading accounting that follows it.
    session_stats: usize,
    stats: usize,
    closed_stats: usize,
    closed_text: usize,
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
    let flags = Flags(flags);
    let (search, rest) = if flags.contains(Flags::READING) {
        let Some((search, rest)) = SearchRequest::read(page, rest) else {
            return;
        };
        (Some(search), rest)
    } else {
        (None, rest)
    };
    let raw_frames = flags.contains(Flags::RAW_FRAMES);
    let (pdf, tail) = match (raw_frames, wrapper::find(rest, wrapper::SEPARATOR)) {
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
    let version = if flags.contains(Flags::TYPED_DIAGNOSTICS) {
        VERSION_1_4
    } else if flags.contains(Flags::VERSION_1_3) {
        VERSION_1_3
    } else if search.is_some() && !flags.contains(Flags::OLDER) {
        VERSION_1_2
    } else if flags.contains(Flags::GENERATIONS) {
        VERSION_1_1
    } else {
        VERSION
    };
    let (frames, plan) = requests(version, flags, pdf, tile, search.as_ref());
    let requests = frames.kinds;
    let mut input = frames.bytes;
    input.extend_from_slice(tail);

    let session = Session::new(super::limits().into());
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
            response.kind == kind | 0x80 || response.kind == ERROR_KIND,
            "kind {} answers request kind {kind}",
            response.kind
        );
    }
    for response in &responses {
        assert!(
            layout(response).is_some(),
            "malformed response {}: {:?}",
            response.kind,
            response.payload
        );
    }
    let hello = responses.first().filter(|r| r.kind == 129);
    assert!(
        hello.is_none_or(|r| r.payload == version.to_le_bytes()),
        "hello negotiated another version"
    );
    check_session(&requests, &responses, &plan, version);
    check_tiles(&responses, &plan, tile);
    if let (Some(reading), Some(search)) = (&plan.reading, &search)
        && matches!(version, VERSION_1_2 | VERSION_1_3 | VERSION_1_4)
    {
        check_reading(&responses, &plan, reading, search);
    }
}

/// The frames: hello, open of handle 1, page_info, the generation requests,
/// render_tile, the reading requests, diagnostics, close and the reading
/// requests on the closed handle.
fn requests(
    version: u32,
    flags: Flags,
    pdf: &[u8],
    tile: TileRequest,
    search: Option<&SearchRequest>,
) -> (Frames, Plan) {
    let mut frames = Frames::default();
    frames.push(1, &version.to_le_bytes());
    let mut open = vec![u8::from(!flags.contains(Flags::STRICT))];
    open.extend_from_slice(&0_u32.to_le_bytes());
    open.extend_from_slice(&u32::try_from(pdf.len()).unwrap_or(u32::MAX).to_le_bytes());
    open.extend_from_slice(pdf);
    let open = frames.push(2, &open);
    frames.push(3, &words(&[1, tile.page]));
    let generations = flags.contains(Flags::GENERATIONS).then(|| {
        frames.push(7, &words(&[1, 2]));
        let current = frames.push(8, &generation_tile(2, tile));
        let newer = frames.push(7, &words(&[1, 3]));
        let stale = frames.push(8, &generation_tile(2, tile));
        [current, newer, stale]
    });
    let v1_tile = frames.push(4, &render_tile(tile));
    let page = words(&[1, tile.page]);
    // Fields are evaluated, and so the frames pushed, in the order written;
    // the requests on the closed handle follow close.
    let mut reading = search.map(|search| Reading {
        text: frames.push(10, &page),
        search: frames.push(11, &search_payload(search, search.offset, search.limit)),
        complete: frames.push(11, &search_payload(search, 0, 1024)),
        links: frames.push(12, &page),
        outlines: frames.push(13, &words(&[1])),
        labels: frames.push(14, &words(&[1])),
        session_stats: frames.push(9, &[]),
        stats: frames.push(15, &[]),
        closed_stats: 0,
        closed_text: 0,
    });
    let typed = flags.intersects(Flags::TYPED_DIAGNOSTICS | Flags::UNNEGOTIATED_DIAGNOSTICS);
    let diagnostics = frames.push(if typed { 17 } else { 5 }, &words(&[1]));
    let close = frames.push(6, &words(&[1]));
    if let Some(reading) = &mut reading {
        reading.closed_stats = frames.push(15, &[]);
        reading.closed_text = frames.push(10, &page);
    }
    let plan = Plan {
        open,
        v1_tile,
        generations,
        reading,
        diagnostics,
        close,
    };
    (frames, plan)
}

/// Operational errors keep the session usable: after a successful open,
/// diagnostics answer (or exceed the frame budget) and close succeeds.
/// Older handshakes reject every reading request and v1 the accounting
/// request of v1.1, as operational InvalidArgument errors.
fn check_session(requests: &[u8], responses: &[Response<'_>], plan: &Plan, version: u32) {
    if responses.get(plan.open).is_some_and(|r| r.kind == 130) {
        let diagnostics = responses.get(plan.diagnostics);
        assert!(
            diagnostics.is_some_and(|r| {
                if requests.get(plan.diagnostics) == Some(&17) {
                    if version == VERSION_1_4 {
                        r.kind == 145 || category(r) == Some(LIMIT)
                    } else {
                        category(r) == Some(INVALID_ARGUMENT)
                    }
                } else {
                    r.kind == 133 || category(r) == Some(LIMIT)
                }
            }),
            "diagnostics failed after a successful open"
        );
        let close = responses.get(plan.close).map(|r| r.kind);
        assert_eq!(close, Some(134), "close failed after a successful open");
    }
    let rejected = |kind: u8| match kind {
        9 => version == VERSION,
        10..=15 => !matches!(version, VERSION_1_2 | VERSION_1_3 | VERSION_1_4),
        17 => version != VERSION_1_4,
        _ => false,
    };
    for (index, (&kind, response)) in requests.iter().zip(responses).enumerate() {
        if rejected(kind) {
            assert_eq!(
                category(response),
                Some(INVALID_ARGUMENT),
                "request {index} of kind {kind} in a session of version {version:#x}"
            );
        }
    }
    if let Some(stats) = plan.reading.as_ref().map(|r| r.session_stats) {
        assert!(
            version == VERSION || responses.get(stats).is_some_and(|r| r.kind == 137),
            "session accounting failed"
        );
    }
}

/// Pixels of the same tile agree between the v1.1 and v1 requests, and the
/// tile requested after a newer generation was selected is skipped before
/// rendering.
fn check_tiles(responses: &[Response<'_>], plan: &Plan, tile: TileRequest) {
    let v1 = responses
        .get(plan.v1_tile)
        .filter(|r| r.kind == 132)
        .and_then(|r| tile_pixels(r, 0, tile));
    let Some([current, newer, stale]) = plan.generations else {
        return;
    };
    let current = responses
        .get(current)
        .filter(|r| r.kind == 136)
        .and_then(|r| {
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
    if responses.get(newer).is_some_and(|r| r.kind == 135) {
        let stale = responses.get(stale).map(|r| (r.kind, r.payload));
        assert_eq!(
            stale,
            Some((136, &[0, 2, 0, 0, 0][..])),
            "a stale tile was not skipped"
        );
    }
}

/// The reading responses of a v1.2 session agree with the document and
/// with each other.
fn check_reading(
    responses: &[Response<'_>],
    plan: &Plan,
    reading: &Reading,
    search: &SearchRequest,
) {
    let success = |index: usize, kind: u8| {
        responses
            .get(index)
            .filter(|r| r.kind == kind)
            .map(|r| Fields(r.payload))
    };
    let pages = success(plan.open, 130).and_then(|mut f| {
        f.u32()?;
        f.u32()
    });
    let text = success(reading.text, 138).and_then(|mut f| page_text(&mut f));
    let found = success(reading.search, 139).and_then(|mut f| search_result(&mut f));
    if let Some(found) = &found {
        check_search(found, search, text);
        // A finished window extracted every page, so the complete search,
        // with its larger response, can only exceed a budget; one that
        // stopped at its limit may not have reached a failing page.
        let complete = responses.get(reading.complete);
        assert!(
            !found.done || complete.is_some_and(|r| r.kind == 139 || category(r) == Some(LIMIT)),
            "the complete search failed where a window of it succeeded"
        );
        let complete = success(reading.complete, 139).and_then(|mut f| search_result(&mut f));
        if let Some(complete) = complete.filter(|c| c.done) {
            check_cursor(found, &complete, search);
        }
    }
    for (index, kind) in [(reading.links, 140), (reading.outlines, 141)] {
        let Some(mut fields) = success(index, kind) else {
            continue;
        };
        let mut destinations = Vec::new();
        let parsed = if kind == 140 {
            links(&mut fields, &mut destinations)
        } else {
            outlines(&mut fields, &mut destinations)
        };
        let count = pages.unwrap_or(0);
        assert!(
            parsed.is_none() || destinations.iter().all(|&page| page < count),
            "destination pages {destinations:?} of {count}"
        );
    }
    if let Some(labels) = success(reading.labels, 142).and_then(|mut f| f.u32()) {
        assert_eq!(Some(labels), pages, "labels for every page");
    }
    let stats = success(reading.session_stats, 137);
    if let (Some(stats), Some(mut reading_stats)) = (stats, success(reading.stats, 143)) {
        assert_eq!(
            reading_stats.take(24),
            Some(stats.0),
            "reading accounting changed the session fields"
        );
        let text_bytes = reading_stats.u64();
        let text_pages = reading_stats.u32();
        assert!(
            text_bytes.is_some() && text_pages.is_some_and(|p| p <= 8),
            "{text_pages:?} retained text pages"
        );
    }
    let closed = success(reading.closed_stats, 143).map(|f| f.0);
    assert!(
        closed.is_none_or(|bytes| bytes.iter().all(|&b| b == 0)),
        "close kept retained bytes: {closed:?}"
    );
    let closed_text = responses.get(reading.closed_text);
    assert_eq!(
        closed_text.and_then(category),
        Some(INVALID_ARGUMENT),
        "page text of a closed document"
    );
}

/// A window's matches lie in the window, page by page; it is done exactly
/// when it ends at the window's end; it stops early only at its limit; and
/// matches on its first page are ranges of that page's text that fold to
/// the query.
fn check_search(found: &Found<'_>, search: &SearchRequest, text: Option<&str>) {
    assert!(
        found
            .matches
            .iter()
            .all(|m| (search.start..search.end).contains(&m.page)),
        "matches outside pages {}..{}",
        search.start,
        search.end
    );
    assert!(
        found.matches.windows(2).all(|m| m[0].page <= m[1].page),
        "matches out of page order"
    );
    assert_eq!(
        found.done,
        found.next_page == search.end,
        "done disagrees with the cursor"
    );
    let count = u32::try_from(found.matches.len()).unwrap_or(u32::MAX);
    assert!(
        count <= search.limit && (count == search.limit || found.done),
        "{count} matches with limit {} and done {}",
        search.limit,
        found.done
    );
    let Some(text) = text else {
        return;
    };
    let case_insensitive = search.options & 1 == 0;
    let query = super::extraction::fold(&search.query, case_insensitive);
    for m in found.matches.iter().filter(|m| m.page == search.start) {
        let matched = text.get(m.start..m.end).filter(|s| !s.is_empty());
        assert!(
            matched.is_some(),
            "match {}..{} in {} bytes of text",
            m.start,
            m.end,
            text.len()
        );
        if let Some(matched) = matched
            && search.options & 4 == 0
        {
            assert_eq!(
                super::extraction::fold(matched, case_insensitive),
                query,
                "match for {:?} with options {}",
                search.query,
                search.options
            );
        }
    }
}

/// The window returns the complete search's matches from its offset on, up
/// to its limit, and resuming at its cursor returns exactly the rest.
fn check_cursor(found: &Found<'_>, complete: &Found<'_>, search: &SearchRequest) {
    // Each match's ordinal on its page, the unit of the cursor's offset.
    let mut ordinals = Vec::new();
    let mut previous = None;
    for m in &complete.matches {
        let ordinal = match previous {
            Some((page, ordinal)) if page == m.page => u32::saturating_add(ordinal, 1),
            _ => 0,
        };
        ordinals.push(ordinal);
        previous = Some((m.page, ordinal));
    }
    let after = |page: u32, offset: u32| -> Vec<&[u8]> {
        complete
            .matches
            .iter()
            .zip(&ordinals)
            .filter(|(m, o)| m.page > page || (m.page == page && **o >= offset))
            .map(|(m, _)| m.record)
            .collect()
    };
    let expected = after(search.start, search.offset);
    let returned: Vec<&[u8]> = found.matches.iter().map(|m| m.record).collect();
    assert!(
        expected.starts_with(&returned),
        "the window returned other matches than the complete search"
    );
    assert!(
        after(found.next_page, found.next_offset) == expected[returned.len()..],
        "resuming at {}:{} does not return the remaining matches",
        found.next_page,
        found.next_offset
    );
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

/// The error category of a kind 255 response.
fn category(response: &Response<'_>) -> Option<u8> {
    (response.kind == ERROR_KIND)
        .then(|| response.payload.first().copied())
        .flatten()
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

fn search_payload(search: &SearchRequest, offset: u32, limit: u32) -> Vec<u8> {
    let mut payload = words(&[1, search.start, search.end, offset, limit]);
    payload.push(search.options);
    let length = u32::try_from(search.query.len()).unwrap_or(u32::MAX);
    payload.extend_from_slice(&length.to_le_bytes());
    payload.extend_from_slice(search.query.as_bytes());
    payload
}

/// Request frames with identifiers 1, 2, …
#[derive(Debug, Default)]
struct Frames {
    bytes: Vec<u8>,
    kinds: Vec<u8>,
}

impl Frames {
    /// Appends a frame and returns its index among the requests.
    fn push(&mut self, kind: u8, payload: &[u8]) -> usize {
        let index = self.kinds.len();
        self.kinds.push(kind);
        let id = u32::try_from(self.kinds.len()).unwrap_or(u32::MAX);
        let length = u32::try_from(payload.len() + 5).unwrap_or(u32::MAX);
        self.bytes.extend_from_slice(&length.to_le_bytes());
        self.bytes.push(kind);
        self.bytes.extend_from_slice(&id.to_le_bytes());
        self.bytes.extend_from_slice(payload);
        index
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

/// A response payload, read field by field; `None` when it is malformed.
#[derive(Debug, Clone, Copy)]
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(head)
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        self.array().map(|[byte]: [u8; 1]| byte)
    }
    /// A boolean is exactly 0 or 1.
    fn flag(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }
    fn count(&mut self) -> Option<usize> {
        usize::try_from(self.u32()?).ok()
    }
    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }
    /// Geometry is finite.
    fn f64(&mut self) -> Option<f64> {
        self.array()
            .map(f64::from_le_bytes)
            .filter(|v| v.is_finite())
    }
    fn text(&mut self) -> Option<&'a str> {
        let length = self.count()?;
        std::str::from_utf8(self.take(length)?).ok()
    }
    /// `count` quads of eight finite values each.
    fn quads(&mut self, count: usize) -> Option<()> {
        for _ in 0..count.checked_mul(8)? {
            self.f64()?;
        }
        Some(())
    }
    fn end(self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

/// The documented payload of a response of any kind, read to its end.
fn layout(response: &Response<'_>) -> Option<()> {
    let mut f = Fields(response.payload);
    match response.kind {
        129 | 135 => {
            f.u32()?;
        }
        130 => {
            f.u32()?;
            f.u32()?;
        }
        131 => {
            for _ in 0..4 {
                f.f64()?;
            }
            let rotation = f.u16()?;
            (rotation % 90 == 0 && rotation < 360).then_some(())?;
            f.f64()?;
            f.u32()?;
            f.u32()?;
        }
        132 => pixels(&mut f)?,
        133 | 145 => {
            for _ in 0..f.count()? {
                if response.kind == 145 {
                    f.flag()?;
                }
                super::is_tag(f.text()?).then_some(())?;
                (!f.text()?.is_empty()).then_some(())?;
                context(&mut f)?;
            }
        }
        134 => {}
        136 => {
            let rendered = f.flag()?;
            f.u32()?;
            if rendered {
                pixels(&mut f)?;
            }
        }
        137 => {
            f.take(24)?;
        }
        138 => {
            page_text(&mut f)?;
        }
        139 => {
            search_result(&mut f)?;
        }
        140 => links(&mut f, &mut Vec::new())?,
        141 => outlines(&mut f, &mut Vec::new())?,
        142 => {
            for _ in 0..f.count()? {
                f.text()?;
            }
        }
        143 => {
            f.take(36)?;
        }
        144 => {
            if f.flag()? {
                for _ in 0..4 {
                    f.f64()?;
                }
            }
        }
        ERROR_KIND => error(&mut f)?,
        _ => return None,
    }
    f.end()
}

/// `u32 width,height`, then exactly `width*height*4` bytes.
fn pixels(f: &mut Fields<'_>) -> Option<()> {
    let (width, height) = (f.u32()?, f.u32()?);
    (width > 0 && height > 0).then_some(())?;
    let bytes = u64::from(width) * u64::from(height) * 4;
    f.take(usize::try_from(bytes).ok()?).map(|_| ())
}

/// `u8 category | text feature_tag | text repair_tag | text message |
/// context`: the feature tag is present exactly for Unsupported, a refused
/// repair preserves the original category, and both tags are well-formed.
fn error(f: &mut Fields<'_>) -> Option<()> {
    let category = f.u8()?;
    let feature = f.text()?;
    let repair = f.text()?;
    let message = f.text()?;
    let valid = (1..=7).contains(&category)
        && (category == 3) != feature.is_empty()
        && (feature.is_empty() || super::is_tag(feature))
        && (repair.is_empty() || super::is_tag(repair))
        && !message.is_empty();
    valid.then_some(())?;
    context(f)
}

/// `u8 has_object [u32 number | u16 generation] | u8 position_kind` and the
/// position's fields.
fn context(f: &mut Fields<'_>) -> Option<()> {
    if f.flag()? {
        f.u32()?;
        f.u16()?;
    }
    match f.u8()? {
        0 => {}
        1 | 3 => {
            f.u64()?;
        }
        2 => {
            f.u64()?;
            f.u32()?;
            f.u16()?;
        }
        _ => return None,
    }
    Some(())
}

/// Copy text, the quad table and characters that partition the text into
/// Unicode scalars, each naming quads inside the table.
fn page_text<'a>(f: &mut Fields<'a>) -> Option<&'a str> {
    let text = f.text()?;
    let quads = f.count()?;
    f.quads(quads)?;
    let mut start = 0;
    for _ in 0..f.count()? {
        let end = f.count()?;
        let (first, count) = (f.count()?, f.count()?);
        let mut scalars = text.get(start..end)?.chars();
        (scalars.next().is_some() && scalars.next().is_none()).then_some(())?;
        (first.checked_add(count)? <= quads).then_some(())?;
        start = end;
    }
    (start == text.len()).then_some(text)
}

/// A search response: its cursor and its matches.
#[derive(Debug)]
struct Found<'a> {
    next_page: u32,
    next_offset: u32,
    done: bool,
    matches: Vec<Match<'a>>,
}

/// One match and its whole record, for comparing matches exactly.
#[derive(Debug)]
struct Match<'a> {
    page: u32,
    start: usize,
    end: usize,
    record: &'a [u8],
}

fn search_result<'a>(f: &mut Fields<'a>) -> Option<Found<'a>> {
    let (next_page, next_offset, done) = (f.u32()?, f.u32()?, f.flag()?);
    (!done || next_offset == 0).then_some(())?;
    let mut matches = Vec::new();
    for _ in 0..f.count()? {
        let before = f.0;
        let (page, start, end) = (f.u32()?, f.count()?, f.count()?);
        (start <= end).then_some(())?;
        let quads = f.count()?;
        f.quads(quads)?;
        let record = before.get(..before.len() - f.0.len())?;
        matches.push(Match {
            page,
            start,
            end,
            record,
        });
    }
    Some(Found {
        next_page,
        next_offset,
        done,
        matches,
    })
}

/// Links: at least one quad each and a target.
fn links(f: &mut Fields<'_>, destinations: &mut Vec<u32>) -> Option<()> {
    for _ in 0..f.count()? {
        let quads = f.count()?;
        (quads > 0).then_some(())?;
        f.quads(quads)?;
        target(f, destinations)?;
    }
    Some(())
}

/// Outline items in preorder from depth zero, with flags, a style of bits
/// 0–1, colours in 0..1 and a target.
fn outlines(f: &mut Fields<'_>, destinations: &mut Vec<u32>) -> Option<()> {
    let mut deepest = 0;
    for _ in 0..f.count()? {
        let depth = f.count()?;
        (depth <= deepest).then_some(())?;
        deepest = depth + 1;
        f.text()?;
        f.flag()?;
        (f.u8()? <= 3).then_some(())?;
        for _ in 0..3 {
            (0.0..=1.0).contains(&f.f64()?).then_some(())?;
        }
        target(f, destinations)?;
    }
    Some(())
}

/// Actions in preorder, each after its parent, of the seven documented
/// types; destination pages are collected.
fn target(f: &mut Fields<'_>, destinations: &mut Vec<u32>) -> Option<()> {
    for index in 0..f.count()? {
        (f.count()? <= index).then_some(())?;
        match f.u8()? {
            1 => destination(f, destinations)?,
            2 => {
                f.text()?;
                if f.flag()? {
                    f.text()?;
                }
                f.flag()?;
            }
            3 => {
                let name = f.text()?;
                (NAMED.contains(&name) || escaped(name)).then_some(())?;
            }
            4..=6 => {}
            7 => escaped(f.text()?).then_some(())?,
            _ => return None,
        }
    }
    Some(())
}

/// `u32 page; u8 fit, coordinate_mask; f64 coordinates[]`, with a mask the
/// fit allows.
fn destination(f: &mut Fields<'_>, destinations: &mut Vec<u32>) -> Option<()> {
    destinations.push(f.u32()?);
    let (fit, mask) = (f.u8()?, f.u8()?);
    let allowed = match fit {
        0 => mask <= 7,
        1 | 5 => mask == 0,
        2 | 3 | 6 | 7 => mask <= 1,
        4 => mask == 15,
        _ => false,
    };
    allowed.then_some(())?;
    for _ in 0..mask.count_ones() {
        f.f64()?;
    }
    Some(())
}

/// An escaped PDF name: a slash and printable ASCII.
fn escaped(name: &str) -> bool {
    name.strip_prefix('/')
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_graphic()))
}
