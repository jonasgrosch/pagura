# Pagura session protocol v1 through v1.4

ADR 0005 defines a synchronous command over stdin/stdout with the same layouts
on native and wasm32-wasip1. This document freezes the v1 byte layout, including the refused-repair field.
Future incompatible changes require a new protocol version. No JSON, newline,
platform-sized integers, implicit padding, compression, or unsolicited frames.
All integers and IEEE-754 binary64 values are **little endian**. PDF and password
strings are raw bytes. Text in responses is UTF-8. Page indexes start at zero.

Each frame is `u32 body_length | u8 kind | u32 request_id | payload`.
`body_length` includes kind and identifier, excludes its own four bytes, and is
at least 5. Identifiers must be nonzero and strictly increase, without wrapping;
they need not be consecutive. Every response echoes the identifier. An error
before a complete body is read uses identifier zero. Complete but malformed
bodies use the identifier from that body. There is exactly one response per
complete valid request. Callers serialize requests; the host flushes responses.

The first request is `hello(1)` for v1, `hello(65537)` (0x00010001) for v1.1,
`hello(65538)` (0x00010002) for v1.2, `hello(65539)` (0x00010003) for v1.3,
or `hello(65540)` (0x00010004) for v1.4.
Repeated hello, unsupported versions,
non-increasing IDs, unknown kinds, invalid recovery bytes, trailing payload,
truncation and oversized frames emit one error and **end the session**. Clean
EOF between frames is successful. EOF inside a frame is malformed. Native exit
status is 0 at clean EOF and 1 after protocol/transport failure. I/O failures
cannot always produce an error frame. PDF errors, missing handles/pages, bad
render arguments and resource-limit failures during operations return error
responses and keep the session alive. A failed open consumes no handle.

`blob` is `u32 byte_length | bytes`; `text` is a UTF-8 blob.

Every code space below (versions, kinds, categories, position and diagnostic
codes, open flags, tile status, fits, actions and option bits) is an enum in
`pagura_host::protocol` (ADR 0031). `web/protocol_constants.mjs` is generated
from those enums and the default session budgets with
`cargo run -p pagura-host --features codegen --example protocol_constants`; a
pagura-host test fails when the checked-in file is stale.

| Request kind | Request payload | Success kind | Success payload |
|---|---|---|---|
| 1 hello | `u32 version` | 129 | `u32 version` (echoes the negotiated version) |
| 2 open | `u8 recovery` (0 strict, 1 repair), `blob password`, `blob pdf` | 130 | `u32 handle`, `u32 page_count` |
| 3 page_info | `u32 handle`, `u32 page` | 131 | `f64 crop_x0,y0,x1,y1`, `u16 clockwise_rotation`, `f64 user_unit`, `u32 width,height` |
| 4 render_tile | `u32 handle,page`, `f64 scale`, `u32 x,y,width,height` | 132 | `u32 width,height`, exactly `width*height*4` bytes straight RGBA8 |
| 5 diagnostics | `u32 handle` | 133 | `u32 count`, then `count` diagnostic records |
| 6 close | `u32 handle` | 134 | empty |

Page info describes the normalized effective CropBox, rotation in degrees, and
device size at scale 1 including UserUnit and rotation (ISO 32000-2 7.7.3.3).
Scale is positive finite pixels per default user-space unit (1/72 inch). Tile
coordinates have a top-left device origin; tiles must be nonempty and inside
the page (ADR 0004). Pixels are row-major straight RGBA8 for ImageData, rendered
onto opaque white. A failed render never returns a partial tile. Rendering
support is determined by the engine; page_info success is not rendering success.
All /Contents streams form one sequence (ISO 32000-2 7.8.2).

Handles start at 1, increase, and are never reused, including after close.
Handles are valid only within one session. A document is retained only after
open and page-tree validation succeed. Open's repair mode is forwarded directly
to the document API (ADR 0003); legacy diagnostic snapshots contain repairs
discovered so far, in discovery order, including repairs made during lazy
rendering. Protocol 1.4 also exposes informational diagnostics (see below).

## Viewport generations (v1.1)

Version 1.1 is additive. The byte layouts and semantics of request kinds 1–6
and their responses remain unchanged, and native/stdio clients can keep using
v1. New kinds require hello(65537); using them in a v1 session returns an
InvalidArgument error without ending the session.

| Request kind | Payload | Success kind | Payload |
|---|---|---|---|
| 7 generation | `u32 handle,generation` | 135 | `u32 generation` |
| 8 generation_tile | `u32 generation`, then the complete render_tile payload | 136 | `u8 status`, `u32 generation`, then tile data only for status 1 |

Generation values are nonzero, monotonically increasing per open document;
selecting the same value is idempotent. Close releases the selection. A future
or unselected generation is InvalidArgument. For a generation older than the
selected value, status 0 is returned **before page lookup, compilation or
painting**, with no dimensions or pixels. Status 1 is followed by
`u32 width,height | straight RGBA8 bytes`, identically to a v1 tile. Error
responses still use kind 255 and retain the existing structured fields.

The browser increments its local generation as soon as the viewport changes.
Before transmitting a queued tile it checks that generation and skips obsolete
requests locally. It sends kind 7 before the first tile of the newest view.
This matters for a synchronous transport: a later generation frame cannot
overtake an earlier frame already being processed. A tile already executing
is allowed to finish; its stale pixels are discarded. There is no interruption
inside interpretation/rasterization. Generations do not wrap: reopen the session
if the u32 space is exhausted. Buffer error offsets refer to the actual v1.1
payload fields, including the additional generation word.

## Session accounting (v1.1)

Request kind 9 has no payload. Response kind 137 contains:
`u64 source_bytes | u64 cache_bytes | u32 documents | u32 lists`.
This additive request requires a v1.1 handshake; v1 rejects it with an
operational InvalidArgument error. No existing layouts change.

Source bytes count original input retained across open documents; cache bytes
are compiled-list policy charges described below. Close releases both counts
for that handle. The counters exclude document-internal caches, temporary
allocations, allocator overhead, request/response buffers and JavaScript data.
They are not total live bytes. Document's internal ADR 0012 counter currently
has no public accessor. The shim separately reports linear-memory byte length
as a sideband message, outside the protocol; WebAssembly memory cannot shrink
when allocations are freed. Neither value is process RSS.

## Compiled page retention

The session reuses immutable display lists keyed by document handle and page
index across tiles and zoom levels. Close releases all lists for that handle.
The LRU retains at most eight entries, with an accounting budget of
SessionLimits::list_cache_bytes (default 64 MiB); zero disables retention.
Oversized lists render without being retained.

The charge is 4096 bytes per painted mark and a
fixed 512-byte header charge. These are explicit **estimates**, not measured
retained heap sizes: the renderer's public API exposes mark count but no allocation count.
The accounting budget and entry cap bound retention policy; they are not a
hard ceiling on allocator or process memory, especially for expanded resources.
Per-operation renderer Limits still apply. Exact cache memory accounting would
require an allocation-size API from the renderer.

## Structured failures and diagnostics

Error kind 255 has payload `u8 category | text feature_tag | text repair_tag |
text message | context`. The two tags are distinct fields, including their
u32 lengths even when empty. Categories are 1 Syntax, 2 Limit, 3 Unsupported,
4 MissingObject, 5 Cycle, 6 Password, 7 InvalidArgument.

- `feature_tag` is nonempty only for Unsupported, carrying Error::feature().
- `repair_tag` carries Error::repair when strict mode rejects a repair
  (ADR 0003), preserving the original category, feature, message and context.
  It is empty when no repair was refused. The repair **was refused**, not
  applied. For example,
  strict open of classic/trailing-bytes.pdf has category 1, an empty feature
  tag and repair tag `file.trailing-bytes`.
- **Compatibility change for every protocol version:** a refused-repair tag
  can now accompany any category (for example Limit, Cycle or Unsupported),
  not only Syntax as the v1 text said. Clients written against v1 through
  v1.3 must accept a nonempty `repair_tag` with any category.
- Message, object and position retain exactly the facade Error context. Tags
  are never inferred from message text. The JS HostError exposes `tag` for
  the feature and `repair` for the refused repair (empty strings when absent).

Legacy diagnostic records describe repairs that **were applied**:
`text repair_tag | text message | context`. Request 5 / response 133 keep this
layout and return repairs only, including in protocol 1.4. Informational
diagnostics never appear in this legacy view.

Protocol 1.4 adds request **17 diagnostic details**, with payload `u32 handle`,
and response **145**, with payload `u32 count` followed by `count` records:
`u8 kind | text tag | text message | context`. Kind 0 means Repair; kind 1 means
Information. Unknown kind bytes are invalid. Tags remain stable and the kind
is explicit, never inferred from tag text. Strict mode rejects repairs only;
`font.substituted` and `font.cid-glyph-missing` are information in both recovery
modes. Information and repairs share the document's diagnostic cap and retain
discovery order. Older negotiated versions reject request 17 with operational
InvalidArgument and keep serving. Handles and frame budgets follow request 5.

`context` is:

- `u8 has_object` (0 or 1), then if 1: `u32 object_number | u16 generation`;
- `u8 position_kind`: 0 Unknown (no further bytes), 1 File (`u64 offset`),
  2 Stream (`u64 offset | u32 stream_object_number | u16 generation`),
  3 Buffer (`u64 offset`). Buffer offsets in protocol errors refer to the body;
  framing errors use Buffer(0). File/Stream positions preserve engine semantics.

## Budgets and capabilities

Session::new accepts a SessionConfig: the facade Limits that each document
receives, and SessionLimits for the session as a whole (ADR 0031). No session
budget borrows a Limits field. Maximum body length is
SessionLimits::max_frame_bytes, by default `min(u32::MAX, max(file_bytes +
string_bytes + 14, pixels*4 + 18, reading_response_bytes + 5, 1024))` with
saturating arithmetic. The 1024-byte minimum allows structured errors even with
zero operation budgets. It is checked before reserving request memory; the body
then grows with the bytes received, through fallible reservations. Each
pdf/password blob is also checked against file_bytes/string_bytes. Retained
source bytes **across all open documents** must fit total_source_bytes
(default 512 MiB), and at most max_documents (default 256) documents are open
at once; close releases both. Handles cannot overflow.
Tiles are checked against pixels and the response frame cap before painting.
Responses are bounded during encoding; an unencodable success is replaced by a
Limit error. Tile pixels and reading payloads are written after the encoded
fields without being copied into the frame. The same limits apply on native
and wasm. Limits do not constitute
an OS memory sandbox: document caches and temporary buffers also consume memory.

The browser grants **no preopened directories, sockets, arguments or
environment**. Only stdin, stdout and stderr exist. The project-owned shim
implements fd_read/fd_write, clock_time_get, random_get, proc_exit, args_get,
args_sizes_get, environ_get, environ_sizes_get and fd_prestat_get. Unimplemented
imports trap with their exact name. The UI reads a user-selected file and
transfers bytes; PDF content never controls file access, URLs or script execution.
EOF can be signalled on the shared stdin mailbox; termination releases the
entire session. There is no cancellation within a synchronous request in v1.

## Supervisor cancellation and execution recovery

Native embedders can attach a cloneable Cancellation flag through
Session::with_cancellation. Generated tiles observe it before compilation,
between compilation and painting, and after painting before publishing pixels.
Cancellation produces the existing kind 136 skipped response. A compiled list
cancelled before admission is not cached; successfully admitted lists remain
valid. Reset the flag only after acknowledgement, before the next generation.
v1 requests retain their original behavior. This is a host-boundary check,
not a checkpoint inside the engine. The current WASI command cannot observe an
external flag during rendering: no unsafe memory exports or extra descriptors
are introduced. Engine checkpoints and their WASI control bridge need separate
APIs. A skipped result carries no pixels and cannot become a partial success.

The browser client rejects active and queued requests on fatal execution,
Limit errors or its startup/request deadline, terminates the execution thread,
and never reuses failed memory. The viewer creates a fresh instance and reopens
its retained source at the same page, zoom and relative scroll anchor, with the
same recovery policy and password. It makes one automatic attempt per selected
file; subsequent failure requires reopening explicitly. Errors remain visible
after a successful recovery. Ordinary Syntax/Unsupported/Password errors keep
their structured handling. The browser shim checks its 512 MiB linear-memory
budget at I/O boundaries; the 30-second client deadline also covers an engine
that never returns to an import. These are detection boundaries, not a hard
peak-memory allocation limit or an in-engine interruption guarantee.

## Reading operations (v1.2)

`hello(65538)` (`0x00010002`) enables all v1/v1.1 operations and the following
additions. Kinds 1–9 and all their response layouts remain byte-for-byte
unchanged. Valid new requests in older sessions return operational
InvalidArgument; the session remains usable. Reading errors use the unchanged
kind 255 layout. No request interrupts a running operation.

| Kind | Request payload | Success kind | Success payload |
|---|---|---|---|
| 10 page_text | `u32 document,page` | 138 | `text copy_text; u32 quads; Quad[quads]; u32 characters; Character[characters]` |
| 11 search | `u32 document,start_page,end_page,match_offset,limit; u8 flags; text query` | 139 | `u32 next_page,next_offset; u8 done; u32 count; Match[count]` |
| 12 page_links | `u32 document,page` | 140 | `u32 count; Link[count]` |
| 13 outlines | `u32 document` | 141 | `u32 count; Outline[count]` |
| 14 page_labels | `u32 document` | 142 | `u32 count; text label[count]` |
| 15 reading_stats | empty | 143 | unchanged kind-137 fields, followed by `u64 text_bytes; u32 text_pages` |

All counts are u32, strings are length-prefixed UTF-8, booleans are exactly 0
or 1, and all geometry is finite f64 in default user space. A Quad is eight
f64 values `(x0,y0,...,x3,y3)`. It precedes page rotation, UserUnit and zoom.
Text quads use the engine's metric cell corner order; link QuadPoints retain
source vertex order (which can differ). They are selection/hit geometry,
not glyph ink. The host does not transform or quantize coordinates.

A Character is `u32 end_byte,first_quad,quad_count`. Its start byte is the
previous character's end (zero for the first). The range
`[first_quad,first_quad+quad_count)` indexes the shared page quad table and must
fit it. The table contains source glyph quads once in content order, including
unreferenced hidden glyphs. This keeps decoding linear even when many ActualText
characters share a large source range; clients must not eagerly expand those
ranges into duplicated arrays.
Characters partition copy_text in reading order into Unicode scalars; offsets
are UTF-8 byte offsets, half-open, including inserted spaces/LFs and U+FFFD for
unknown Unicode. The UTF-8 text is authoritative for copying. Unknown Unicode
remains a barrier to engine search. An inserted separator or a graphics-only
ActualText replacement can have zero quads; a replacement can refer to many
source glyph quads. Repeated replacement characters can share quads. No
synthetic coordinates are invented. Text and replacement policy follow ADR
0014, ISO 32000-2 9.10 and 14.9.4; this is not tagged-PDF reading order.

A Match is `u32 page,start_byte,end_byte,quad_count; Quad[quad_count]`.
The half-open page window must satisfy `start <= end <= page_count` and span
at most 16 pages. Limit is 1..1024; the query must be nonempty UTF-8 and at most
min(Limits.string_bytes,4096) bytes. Flags: bit 0 case-sensitive, bit 1 whole
words, bit 2 join hyphenated line breaks; other bits are invalid. Whitespace
folding, Unicode uppercase expansions, unknown barriers and word boundaries
are the engine policy documented in ADR 0014, not browser regex semantics.

Match offset is the zero-based match ordinal on start_page for this exact
query/options. Zero starts a new window. The response is in page then engine
match order. Continue with next_page/next_offset and the same query/options
and end_page. At most limit matches are returned. Done means next_page equals
end_page; next_offset is then zero. The cursor skips no remaining matches,
including overlapping text ranges. An offset beyond the current page's match
count is InvalidArgument. An empty window returns done with no matches and
requires offset zero. Extraction/search errors fail the whole request and do
not return partial matches; a caller wanting page-specific errors uses
one-page windows. Engine find computes that page's bounded match vector before
host pagination, so the wire limit is not a smaller engine allocation budget.
The client increments its own query epoch, skips obsolete queued requests and
drops already-running requests' obsolete results; no generation fields are
added to older frames. It searches one page at a time and yields between pages.

A Link is `u32 quad_count; Quad[quad_count]; Target`. QuadPoints are used when
valid; otherwise one quad is constructed from the annotation rectangle
(12.5.6.5, Table 176). Reported engine repairs remain available via diagnostics.
An Outline is `u32 depth; text title; u8 initially_open,style; f64 red,green,blue;
Target`. It is preorder with depth zero at the root. Style bit 0 italic, bit 1
bold; colours are DeviceRGB in 0..1. Labels contain exactly one decoded label
per page in page order; absent PageLabels gives decimal indexes (12.4.2).

A Target is `u32 action_count; Action[action_count]`. Zero means no target.
An Action begins with `u32 parent_plus_one; u8 type`; zero parent means root,
otherwise parent_plus_one-1 indexes an earlier action. Direct destinations are
one root action. All Next actions are preserved in engine preorder as inert
data; serializing them never authorizes execution.

| Action type | Data after type |
|---|---|
| 1 internal destination | `u32 page; u8 fit,coordinate_mask; f64 present_coordinates[]` |
| 2 URI | `text uri; u8 has_base; [text base]; u8 is_map` |
| 3 named action | `text name` |
| 4 remote GoToR | empty (inert) |
| 5 Launch | empty (inert) |
| 6 JavaScript | empty (inert; no script bytes are sent) |
| 7 other action | `text escaped_PDF_name` (inert) |

Destination fits preserve Table 149 semantics: 0 XYZ (left,top,zoom), 1 Fit,
2 FitH (top), 3 FitV (left), 4 FitR (left,bottom,right,top), 5 FitB,
6 FitBH (top), 7 FitBV (left). Mask bit i selects coordinate i; absent means
PDF null, including XYZ zoom zero normalized by the engine. XYZ allows mask
0..7; H/V/BH/BV allow mask 0..1; Fit/FitB require zero; FitR requires 15.
Named destinations are resolved by the engine before serialization. Standard
Named actions have their exact names; extension names are escaped PDF names.
URI/Base must be UTF-8; malformed UTF-8 is an operational Syntax error. Base
is not resolved by the engine and is never inferred from the host filesystem
or viewer URL. The viewer only activates one action: multi-action targets,
GoToR, Launch, JavaScript, unknown Named actions and coordinate-appending IsMap
URIs remain explicitly inert. External http/https/mailto targets require user
confirmation after scheme validation. PDF strings never enter HTML markup.

Page text has at most one million semantic characters; larger pages return
Limit before encoding. Every reading response payload is capped at
SessionLimits::reading_response_bytes (default 32 MiB), and the ordinary frame
cap also applies. Output is built with fallible bounded
reservations. Oversized responses yield Limit with no partial output. Invalid
handles/pages/windows/cursors/options are operational InvalidArgument; engine
Syntax, Unsupported, Cycle, Password, Limit and refused repairs preserve the
existing error fields. Truncation, invalid UTF-8 queries, oversized query blobs,
unknown kinds and trailing bytes are terminal protocol errors as before.

Extracted TextPages have an independent LRU, keyed by document and page, at most
eight entries, with an accounting budget of SessionLimits::text_cache_bytes
(default 16 MiB; zero disables retention). It charges TextPage::retained_bytes()
plus 512 bytes per entry for its inline header and cache bookkeeping. The engine's
capacity estimate counts both page and logical text, vectors, strings and shared
source allocations once per page; Arc reference counts do not change it.
Allocation sizes differ by target: text_bytes and, near the byte budget,
eviction and text_pages can differ between native and WASI. Protocol layouts
and document results stay identical. Accounting counters are not cross-target
heap-size constants or document-output parity evidence.
No source-size multiplier is used. Hits refresh recency; over-budget pages are
served without retention. Close releases the text cache for that document.
Kind 15 reports these charges and entry counts; kind 9 stays unchanged.
Charges exclude font caches, document internals, temporaries and transport/JS
buffers and are not heap/RSS measurements. Text extraction may compile a page
independently of its cached painting list; no text is added to display lists.

## Painted content bounds (v1.3)

`hello(65539)` enables kinds 1–15 unchanged plus kind 16; protocol 1.4
preserves these operations and adds typed diagnostics (request 17). Older sessions reject
a valid kind-16 request with operational InvalidArgument and remain usable.

| Kind | Request payload | Success kind | Success payload |
|---|---|---|---|
| 16 content_bounds | `u32 document,page` | 144 | `u8 present; [f64 x0,y0,x1,y1]` |

Present is exactly 0 or 1. A zero has no following coordinates and means the
page has no painted bounds. A one has four finite coordinates, ordered as an
effective crop rectangle. Payloads are exactly 1 or 33 bytes, before kind/id.
They use default user space, before Rotate, UserUnit or zoom. These are the
renderer’s conservative painted-content bounds, clamped to the crop box
(12.3.2.2); curves, clips, strokes and groups can overestimate the painted area.
The viewer uses them for FitB/FitBH/FitBV, falling back to the crop box for an
empty page. This fallback is viewer policy, not fabricated engine geometry.

The request reuses or admits the page's compiled display list under its existing
LRU budget, so bounds and subsequent tiles share compilation. Unknown handles
and invalid page indexes return operational InvalidArgument at payload offsets
5 and 9 respectively; compilation errors and bounds traversal limits preserve
their structured category/context. A response-budget failure returns Limit
without partial geometry. Request truncation or trailing bytes remain terminal
protocol errors. The request is synchronous and never interrupts running work.
