# Pagura session protocol v1

ADR 0005 defines a synchronous command over stdin/stdout, identically on native
and wasm32-wasip1. This document freezes the v1 byte layout, including the refused-repair field.
The earlier round-1 layout was a draft: rebuild host and clients together.
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

The first request must be `hello(1)`. Repeated hello, unsupported versions,
non-increasing IDs, unknown kinds, invalid recovery bytes, trailing payload,
truncation and oversized frames emit one error and **end the session**. Clean
EOF between frames is successful. EOF inside a frame is malformed. Native exit
status is 0 at clean EOF and 1 after protocol/transport failure. I/O failures
cannot always produce an error frame. PDF errors, missing handles/pages, bad
render arguments and resource-limit failures during operations return error
responses and keep the session alive. A failed open consumes no handle.

`blob` is `u32 byte_length | bytes`; `text` is a UTF-8 blob.

| Request kind | Request payload | Success kind | Success payload |
|---|---|---|---|
| 1 hello | `u32 version` | 129 | `u32 version` (1) |
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
to the document API (ADR 0003); diagnostics snapshots contain repairs discovered
so far, in discovery order, including repairs made during lazy rendering.

## Structured failures and diagnostics

Error kind 255 has payload `u8 category | text feature_tag | text repair_tag |
text message | context`. The two tags are distinct fields, including their
u32 lengths even when empty. Categories are 1 Syntax, 2 Limit, 3 Unsupported,
4 MissingObject, 5 Cycle, 6 Password, 7 InvalidArgument.

- `feature_tag` is nonempty only for Unsupported, carrying Error::feature().
- `repair_tag` is optional for Syntax: it carries Error::repair when strict
  mode rejects a repair (ADR 0003). It is empty on ordinary Syntax errors and
  the other categories. The repair **was refused**, not applied. For example,
  strict open of classic/trailing-bytes.pdf has category 1, an empty feature
  tag and repair tag `file.trailing-bytes`.
- Message, object and position retain exactly the facade Error context. Tags
  are never inferred from message text. The JS HostError exposes `tag` for
  the feature and `repair` for the refused repair (empty strings when absent).

Diagnostic records describe repairs that **were applied**:
`text repair_tag | text message | context`. Their existing layout is unchanged.

`context` is:

- `u8 has_object` (0 or 1), then if 1: `u32 object_number | u16 generation`;
- `u8 position_kind`: 0 Unknown (no further bytes), 1 File (`u64 offset`),
  2 Stream (`u64 offset | u32 stream_object_number | u16 generation`),
  3 Buffer (`u64 offset`). Buffer offsets in protocol errors refer to the body;
  framing errors use Buffer(0). File/Stream positions preserve engine semantics.

## Budgets and capabilities

Session::new accepts facade Limits. Maximum body length is
`min(u32::MAX, max(file_bytes + string_bytes + 14, pixels*4 + 13,
decoded_bytes, 1024))` with saturating arithmetic. The 1024-byte minimum allows
structured errors even with zero operation budgets. Check it before reserving
request memory; allocations use fallible reservation. Each pdf/password blob is
also checked against file_bytes/string_bytes. Retained source bytes **across
all open documents** must fit file_bytes; live document count is capped by
objects. Each document also receives all Limits. Handles cannot overflow.
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
