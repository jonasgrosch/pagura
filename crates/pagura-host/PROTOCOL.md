# Pagura session protocol v1 through v1.11

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
`hello(65540)` (0x00010004) for v1.4, through `hello(65548)`
(0x0001000c) for v1.12. Versions 1.5–1.7 add the form operations below.
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
`pagura_host::protocol` (ADR 0032). `web/protocol_constants.mjs` is generated
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
selected value, status 0 is returned **before page lookup, recording or
painting**, with no dimensions or pixels. Status 1 is followed by
`u32 width,height | straight RGBA8 bytes`, identically to a v1 tile. Error
responses still use kind 255 and retain the existing structured fields.

The browser increments its local generation as soon as the viewport changes.
Before transmitting a queued tile it checks that generation and skips obsolete
requests locally. It sends kind 7 before the first tile of the newest view.
This matters for a synchronous transport: a later generation frame cannot
overtake an earlier frame already being processed. A tile already executing
is allowed to finish; its stale pixels are discarded. There is no interruption
inside interpretation or painting from the browser. Generations do not wrap: reopen the session
if the u32 space is exhausted. Buffer error offsets refer to the actual v1.1
payload fields, including the additional generation word.

## Session accounting (v1.1)

Request kind 9 has no payload. Response kind 137 contains:
`u64 source_bytes | u64 cache_bytes | u32 documents | u32 lists`.
This additive request requires a v1.1 handshake; v1 rejects it with an
operational InvalidArgument error. No existing layouts change.

Source bytes count original input retained across open documents; cache bytes
are what the open documents' render stores retain, and lists counts the
recorded page scenes those stores keep, as described below. Close releases
all three counts for that handle. The counters exclude document-internal caches, temporary
allocations, allocator overhead, request/response buffers and JavaScript data.
They are not total live bytes. Document's internal ADR 0012 counter currently
has no public accessor. The shim separately reports linear-memory byte length
as a sideband message, outside the protocol; WebAssembly memory cannot shrink
when allocations are freed. Neither value is process RSS.

## Recorded page retention

Each open document has one render store (ADR 0012) with a capacity of
SessionLimits::store_bytes (default 64 MiB); zero disables retention. A page
is recorded once into a scene, which does not depend on zoom or tile, and the
store keeps it with the decoded images, fonts and glyph setups that rendering
reuses across tiles and zoom levels. When a new value does not fit, the store
evicts the least recently used values that no render still uses; a value
larger than the capacity is used without being retained. Close releases the
document's store.

The bytes are the engine's own **estimates** of what each value retains, not
measured heap sizes. The capacity bounds retention policy per document; it is
not a hard ceiling on allocator or process memory. Per-render Limits still
apply, and a render costs the same budgets whether its values came from the
store or were built for it.

Tiles render every pixel within one level of its exact result (ADR 0004).

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
receives, and SessionLimits for the session as a whole (ADR 0032). No session
budget borrows a Limits field. Maximum body length is
SessionLimits::max_frame_bytes, by default `min(u32::MAX, max(file_bytes +
string_bytes + 14, pixels*4 + 18, reading_response_bytes + 5, 1024))` with
saturating arithmetic. The 1024-byte minimum allows structured errors even with
zero operation budgets. Check it before reserving request memory; allocations
use fallible reservation. Each pdf/password blob is also checked against
file_bytes/string_bytes. Retained
source bytes **across all open documents** must fit total_source_bytes
(default 512 MiB), and at most max_documents (default 256) documents are open
at once; close releases both. Handles cannot overflow.
Tiles are checked against pixels and the response frame cap before painting.
Responses are bounded during encoding; an unencodable success is replaced by a
Limit error. The same limits apply on native and wasm. Limits do not constitute
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
Session::with_cancellation. Generated tiles observe it before recording,
between recording and painting, before every band of painting, and after
painting before publishing pixels. Cancellation produces the existing kind 136
skipped response. A scene recorded for a cancelled tile stays in the store,
since the next generation usually renders the same page. Reset the flag only
after acknowledgement, before the next generation. v1 requests retain their
original behavior. Recording a page is not interrupted. The current WASI command cannot observe an
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
buffers and are not heap/RSS measurements. Text is extracted by interpreting
the page into the text device, which records no scene and decodes no image;
extraction and rendering do not change each other's results.

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

The request reuses or records the page's scene in the document's render
store, so bounds and subsequent tiles share one recording. Unknown handles
and invalid page indexes return operational InvalidArgument at payload offsets
5 and 9 respectively; recording errors and the group nesting limit preserve
their structured category/context. A response-budget failure returns Limit
without partial geometry. Request truncation or trailing bytes remain terminal
protocol errors. The request is synchronous and never interrupts running work.

## Form editing (v1.5)

`hello(65541)` preserves older operations and adds requests 18 and 19. Older
sessions reject these requests with operational InvalidArgument. Document
handles remain immutable; saving returns complete PDF bytes for a separate open.

| Kind | Request payload | Success kind | Success payload |
|---|---|---|---|
| 18 form_fields | `u32 document` | 146 | `u32 count; field[count]` |
| 19 save_form | `u32 document; blob changes` | 147 | `blob complete_pdf` |

A blob is a u32 byte count followed by that many bytes; text is a UTF-8 blob.
Each field is `u32 index; u8 kind,flags; text name,value; u32 options; option[]`.
Kinds are text=0, choice=1, button=2, unsupported=3. Flag bits are read-only=0,
multi-select=1, multiline=2, editable-combo=3. Unsupported field values are blank.
An option is `u32 key; text label; u8 selected; blob state`. Choice keys index
options and have empty states; button keys index widgets globally and carry raw
PDF name bytes for the on state. Selected is exactly 0 or 1. Responses are
bounded to 1024 fields, 4096 total options and min(document_bytes,32 MiB).

Changes begin with a u32 count (at most 1024), then strictly increasing unique
`u32 field_index; u8 kind; payload` records. Kind 0 text and kind 4 editable
choice have a text payload; kind 1 has `u32 count; u32 option_indices[count]`
(at most 4096); kind 2 switches a button off with no payload; kind 3 selects
`u32 widget_index; blob raw_state_name`. The whole batch is bounded to
min(document_bytes,32 MiB). Invalid types, values, indexes, names or trailing
batch bytes fail atomically with an operational error. Source bytes and handles
remain usable. The ordinary framing and document budgets also apply.

Saving uses the default library policy: strict source, no encrypted input,
reject signatures or uncertain signature history, and explicit unsupported
errors for unavailable appearance generation. No script/action is executed.

## Inline widget geometry (v1.6)

`hello(65542)` adds request 20: `u32 document,page`. Response 148 contains
`u32 count` and records `u32 widget_index,field_index; u8 flags; f64 x0,y0,x1,y1`.
Rectangles are normalized default user-space rectangles. Flags encode radio
(bit 0), NoToggleToOff (1), RadiosInUnison (2), and list-box choice (3).
At most 4096 widgets and 1024 fields are accepted. Membership comes from the
page's Annots array; the optional P backlink never assigns a widget to a page.
Hidden/NoView, ToggleNoView, NoZoom/NoRotate, optional-content and rotated-MK
widgets are excluded until the browser implements their view policies. Direct
widget identities and read-only/unsupported field controls are not editable.
Earlier versions receive operational InvalidArgument; other layouts are unchanged.

Exact duplicate widget dictionaries may map to their unique canonical form-tree
widget even when Annots uses another object number. Ambiguous candidates are an
operational error; differing dictionaries are not aliases. Changed browser saves
append a reference-reconciliation revision before the normal strict form save.
No-op saves retain original bytes. Comparisons are capped at 64 MiB of estimated
dictionary bytes and page Annots arrays at 4096 entries.

## Native editor presentation (v1.7)

`hello(65543)` adds request 21: `u32 document,page`. Response 149 is `u32 count`
followed by records: `u32 widgetIndex`, UTF-8 font name, three little-endian f64
values (font size, left inset, baseline measured down from widget top), and u8
quadding (0 left, 1 center, 2 right). Glyph positions come from isolated stored
appearance interpretation, including its matrix and fitting; DA supplies a
fallback for empty/unavailable text. The source handle's rendering policy also
applies here. Source decoding is capped at 32 MiB per request; at most 4096
widgets are described. Versions 1.0–1.6 retain their existing layouts.

## Document services (v1.8)

`hello(65544)` adds the following requests without changing earlier layouts.
Older negotiated versions reject these operations with InvalidArgument and stay
usable. These requests require protocol 1.8 or newer.

| Request kind | Payload | Success kind | Payload |
|---|---|---|---|
| 22 metadata | `u32 handle` | 150 | Metadata record below |
| 23 attachments | `u32 handle` | 151 | `u32 count`, then catalog attachment records |
| 24 metadata_bytes | `u32 handle` | 152 | Optional `blob` |
| 25 attachment_bytes | `u32 handle,index`, `u8 filename_key` | 153 | `blob` |

An optional value is `u8 present` (exactly 0 or 1), followed by its value only
when present. An object identity is `u32 number | u16 generation`. Missing and
empty text/blob values remain distinct. Filename keys are F=0, UF=1, DOS=2,
Mac=3 and Unix=4 (`AttachmentKey`); other codes are terminal syntax errors.
A valid but absent key or out-of-range attachment index is an operational
InvalidArgument error, with the request field position. There is no key fallback.

A metadata record contains:

1. Optional Info record: optional object identity; eight optional UTF-8 texts in
   the order Title, Author, Subject, Keywords, Creator, Producer, CreationDate,
   ModDate; and a text trapping declaration (`True`, `False` or `Unknown`).
2. Optional catalog language text.
3. Optional catalog metadata-stream object identity.

Info, language and packet identity use the public typed document views. Packet
bytes are read only by request 24. Info and XMP are separate, unreconciled sources;
no XML parsing, entities, schemas, calendars or profile validation are implied.

Catalog attachment records appear in EmbeddedFiles name-tree byte-key order;
their zero-based ordinal is the index for request 25. Each record contains:

1. Raw name-tree key `blob`, then optional file-specification object identity.
2. Optional UF text; four optional raw filename blobs in F, DOS, Mac, Unix order.
3. Optional description text, optional file-system-name blob, optional
   relationship-name blob, then `u8 volatile` (0 or 1).
4. `u32 payload_count`, then each payload: `u8 filename_key`, object identity,
   optional media-type-name blob, optional `u64 declared_size`.

Bytes are preserved for names without a defined text encoding. The browser
represents declared sizes as exact BigInts. Size and relationship are producer
declarations, not verified payload lengths or security conclusions. Listings
contain at most 1024 files and five EF entries per file. Unknown future library
relationship variants are represented as absent, never guessed. This version
lists catalog EmbeddedFiles only; AF/RF, annotation discovery and portfolios
remain separate work.

All four responses use `SessionLimits::reading_response_bytes` (32 MiB by
default), checked before each fallible writer append. Source views and payload
decoding separately obey the document view/stream limits. Payloads are fully
decoded before the response is framed; this is not streaming output or a cap on
total transient memory. Exceeding either limit returns one error, no partial
response, and leaves the session usable. Listings do not decode embedded bytes;
request 25 applies attachment crypt filters and EFOpen authentication even when
an embedded stream omits Type. External filenames and URLs are never followed.

The viewer loads these records only when Properties and attachments is opened.
Text is placed with `textContent`. Downloads require an explicit button click,
use an octet-stream Blob and fixed local filenames (`metadata.xmp` or
`attachment-N-KEY.bin`), and revoke their object URLs. Closing the dialog,
replacing the document or restarting the host invalidates pending results and
downloads. Payload content is never previewed or executed. Errors in one listing
remain visible without hiding the other service or the document's pages.


## Version 1.9: complete PNG page export

Request 26 `page_png` contains `u32 handle,page | f64 scale | u8 background`.
Scale is positive and finite, relative to 72 DPI. Background 0 composites onto
opaque white; 1 retains transparent unpainted pixels. Other background codes,
truncation and trailing bytes are terminal protocol errors. Missing page/handle,
invalid scale, rendering/encoding failures and budget limits are operational
errors; the session remains usable. Earlier negotiated versions refuse the new
request without ending the session.

Response 154 contains `u32 byte_count | PNG bytes`. The complete page uses its
PDF crop, rotation and UserUnit, screen annotation policy and Auto overprint.
It shares the document render store and ordinary preparation/rasterization path.
PNG is straight RGBA8, non-interlaced and without ancillary color/profile chunks.
No partial image is returned. View-only rotation and viewport generations do not
change this source-page operation.

Surface pixel and decoded-byte budgets are checked before page recording. The
encoder retains the document's input/scratch limits; its output stream-byte cap
is additionally lowered to `max_frame_bytes - 9` for the frame header and byte
count. PNG does not borrow the reading-response budget. The operation retains a
whole surface plus encoder scratch, then complete encoded bytes and the outgoing
frame. It is synchronous: incremental output, active-request cancellation and
host backpressure remain separate work.

The browser client defaults to 1.11 and exposes `pagePng(handle, page, scale,
background)`. More → Export page PNG lets the user choose a page, 1–2400 DPI and
a white or transparent background. Downloads use fixed `page-NNNN.png` names and
`image/png`; object URLs are revoked. Current form values are copied at the click
and, when present, saved to a disposable document handle. That handle is closed
on completion, failure or a stale result. The open source and drafts are not
replaced. Closing the dialog or changing the document discards pending results;
it does not interrupt a running host command. Failed form snapshot/render/encode
operations produce an error instead of a download.

Evidence: public Rust wire/version/budget tests, Python PNG/CRC/zlib pixel decoding
against independent page oracles, native/WASI/client byte parity, and viewer
option/snapshot/race/cleanup tests. These checks are authored; hosted runtime
validation and real-browser interaction verification are pending.

## Version 1.10: attachment parameter checks

`hello(65546)` adds request 27 `attachment_check`: `u32 handle,index` followed
by `u8 filename_key`, using the same catalog order and exact EF keys as request
25. Older negotiated versions return operational InvalidArgument and remain
usable. Malformed keys, truncation or trailing bytes are terminal syntax errors.
Absent handles, indexes or EF keys are operational errors, without fallback.

Success 155 contains `object` (`u32 number | u16 generation`), `u64 decoded_size`,
`u8 size_status`, `u8 checksum_status`. `AttachmentCheckStatus` values are 0
not-declared, 1 matches and 2 mismatch; other values are invalid. The JavaScript
client returns `{ object, decodedSize, size, checksum }` with an exact BigInt
size. The payload is exactly 16 bytes, the body 21 and framed response 25.
A completed mismatch is a successful observation, not a protocol error.

Checks use `EmbeddedFile::check_parameters`: complete authorized decoding under
the document's stream limits, then Size and Table 45 MD5 comparisons. Missing
declarations still require successful decoding. The reading-response cap must
allow 16 bytes before decoding starts; decoding/scratch budgets are independent.
No payload bytes are returned or retained in the result. Password, unsupported
filter, malformed content and resource failures produce errors, not observations.
The source is unchanged. Checking is synchronous and whole-buffer; declarations
are not evidence of authenticity, MIME correctness or content safety.

`Client.attachmentCheck(handle,index,key)` exposes the command. Properties and
attachments offers an explicit Check size and checksum button for each EF
payload. Listing or downloading does not implicitly check. The result shows the
actual size, both observations and any operation failure. It never downloads a
payload. One dialog operation is pending at a time; close, document/epoch/client
changes and recovery invalidate results and callbacks. This discards a stale
response; it does not interrupt the synchronous host command.

Evidence: literal request/response and malformed-code tests, old-version checks,
exact response/decode budgets, attachment authorization and selection failures,
independent compressed fixture observations, native/WASI/client parity, exact
BigInt values and viewer missing/mismatch/error/duplicate/stale-state tests.
Hosted verification is pending; real-browser interaction remains to be checked.


## Version 1.11: page attachment annotations

`hello(65547)` adds three requests, distinct from catalog attachment commands:

| Kind | Request | Payload | Success |
|---|---|---|---|
| 28 | `page_attachments` | `u32 handle,page` | 156 |
| 29 | `page_attachment_bytes` | `u32 handle,page,index; u8 filename_key` | 157 |
| 30 | `page_attachment_check` | `u32 handle,page,index; u8 filename_key` | 158 |

`index` is the original zero-based Annots slot, including other annotation
subtypes. Selecting a link or an absent slot is an operational InvalidArgument;
there is no catalog fallback or fallback to a different EF key. Malformed wire
fields are terminal syntax errors. Versions 1–1.10 refuse these operations but
continue accepting their own commands. The page is zero-based.

Response 156 begins with `u32 count` (at most 1024). Each record contains:

1. `u32 index`, increasing in original Annots order; `object annotation`.
2. Four finite `f64` values: normalized `x0,y0,x1,y1` in default user space.
3. Optional UTF-8 Contents and optional byte-string icon name, each using the
   existing canonical presence flag followed by a length-prefixed value.
4. The file-specification record from response 151, starting at its optional
   object identity. There is no catalog name-tree key in this record.

An absent icon means the Table 187 PushPin default; unknown declared names stay
as bytes. Contents and the file specification's description remain separate.
Repeated annotation identities have separate indexes. Hidden annotations are
included in discovery. No icon is synthesized or file activated by listing.
The complete library view is validated before any response is sent. Its own
traversal/result limits precede encoding; the shared reading-response cap also
bounds the encoded result. A list failure produces no partial success response.

Response 157 is a length-prefixed payload, exactly like response 153. Response
158 has the same 16-byte observation payload as response 155, including object
identity, exact u64 decoded size and two independent declaration statuses.
Authorization, stream budgets and the 16-byte check-response preflight remain
identical to catalog operations. Discovery does not decode embedded bytes.

The client exposes `pageAttachments(handle,page)`,
`pageAttachmentBytes(handle,page,index,key)` and
`pageAttachmentCheck(handle,page,index,key)`. Listing records are
`{index,object,rectangle,contents,icon,file}`; `file` contains the existing
file-specification fields, and all byte strings remain Uint8Array values.
Client decoding rejects noncanonical flags, invalid rectangles, non-increasing
indexes, duplicate EF keys, oversized counts, unknown observation statuses and
truncated/trailing data. Viewer selection is separate follow-up work.

Evidence includes literal wire values, old-version gates, exact original slots,
page-only/catalog separation, missing/malformed/duplicate entries, declaration
checks, password failures/Identity and independent response/decode limits. Hosted
native/WASI/client tests are authored; current-head runtime validation is pending.

## Version 1.12: scoped document source services

Every request below begins with `u32 document`; every response is encoded under
the reading-response cap. Request and response kinds remain offset by 128.
Earlier versions refuse these operations without changing their own records.

| Request / response | Remaining request payload | Successful response |
|---|---|---|
| 31 / 159 Files | file scope | counted records: index, optional key bytes, 1.8 specification, extended details |
| 32 / 160 FileBytes | file scope, u32 index, u8 EF key, payload selection | byte blob |
| 33 / 161 FileCheck | same as FileBytes | existing 1.10 parameter-check record |
| 34 / 162 ObjectMetadata | owner | optional metadata object identity |
| 35 / 163 ObjectMetadataBytes | owner | optional byte blob |
| 36 / 164 Collection | none | optional collection declaration |
| 37 / 165 Images | page/object owner | counted optional resource name and image declaration |
| 38 / 166 ImageBytes | page/object owner, u8 image kind, u8 transfer mode | mode text, optional codec text, byte blob |
| 39 / 167 Reauthenticate | password byte blob | new u32 handle, u32 page count |
| 40 / 168 Thumbnail | page/object owner | optional image declaration |

File scope is a code: 0 catalog EmbeddedFiles, 1 catalog AF, 2 page AF followed
by u32 page, 3 object AF followed by object identity, or 4 page annotations
followed by u32 page. Annotation indexes retain original Annots slots. Payload
selection is 0 EF or 1 RF followed by u32 pair ordinal within that exact EF key.
There is no fallback or deduplication. Owner is 0 document, 1 page followed by
u32 page, or 2 object followed by identity. Images/Thumbnail require page/object;
ordinary ImageBytes requires an image object. Image kind 0 means image, 1 means
thumbnail of the selected owner. Transfer modes are 0 stored and 1 encoded.

Extended file details contain optional ID byte pairs; a parameter record per EF
entry; RF records (key/name byte blobs, object, optional MIME, parameter record);
CI records (key bytes, optional prefix, type name and value); and optional EP
subtype/version byte names. Parameters retain optional exact u64 size, decoded
date strings and checksum bytes. CI integer values use decimal text, decoded by
the client into BigInt; real values use finite f64. CI value type names are
absent, text, integer and real. Sort/presentation declarations remain inert.
Collection responses contain optional identity/initial key, view name, fields
(key, label, kind, optional decimal order, visible/editable flags) and sort keys.
Collection views are Details, Tiles, Hidden or Navigator; field kinds are the
public CollectionFieldKind variant names. Counts are capped at 1024.

An image declaration contains object identity, u32 width, u32 height and a
thumbnail flag. It validates source shape, not image samples or complete PDF
colour/mask semantics. Source transfers are binary assets: stored bytes may be
encrypted, encoded bytes retain codec/PDF interpretation dependencies. Codec
names are DCTDecode, JPXDecode, JBIG2Decode or CCITTFaxDecode; absent codec in
encoded mode means packed PDF samples. No standalone appearance is asserted.

Reauthentication opens a separate immutable snapshot with the original recovery
and engine limits. Wrong EFOpen-only credentials fail immediately; failed
attempts consume no handle. Source/handle budgets are checked before opening.
The existing document is unchanged, and the new handle must be closed. The
viewer uses it only for the explicitly requested operation and clears supplied
password bytes after each attempt. Stale dialogs cannot download its output.
