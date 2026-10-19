# ADR 0031: Host session budgets and generated wire tables

Status: accepted, 2026-10-18. Refines ADR 0005 and applies ADR 0016 to the
host session.

## Context

`pagura-host` serves the session protocol (ADR 0005, `PROTOCOL.md`) over
stdio, natively and as a WASI module that the web viewer runs. Its session
policy borrowed engine budgets. The open-document count was "capped by"
`Limits::objects` (4,000,000, so effectively unbounded); the source bytes of
all open documents were capped by the per-file `file_bytes`; `decoded_bytes`
sized the compiled-page cache, the reading response cap and a floor of the
frame cap, as well as each document's own decoding; `document_bytes` sized
the text cache. Tuning one engine budget silently retuned the session, which
is exactly what ADR 0016 rules out for `Limits`.

Its wire codes were bare integers in four places: the Rust encoder and
decoder, the session's gating, the fuzz driver (which re-declared two error
categories) and the browser codec in `web/`, which also hard-coded the frame
cap computed from `Limits::default()`. Nothing checked that the copies
agreed.

Tile pixels were copied into the frame: the encoder appended the straight
RGBA pixels to the frame buffer, so a 64 MiB tile briefly needed 128 MiB,
and WebAssembly linear memory never shrinks again. Request bodies were
zero-filled to their declared length before any byte arrived.

## Decision

1. **`SessionLimits` holds the session's budgets,** next to `Limits`, each
   with one meaning and a named default: `max_documents` (256 open at once),
   `total_source_bytes` (512 MiB across open documents), `list_cache_bytes`
   (64 MiB), `text_cache_bytes` (16 MiB), `reading_response_bytes` (32 MiB)
   and `max_frame_bytes`. A `SessionConfig { engine, session }` configures a
   `Session`; every open document still receives `engine` unchanged. Like
   `Limits`, `SessionLimits` is built by functional record update.
2. **The frame cap is derived, not borrowed.** `SessionLimits::for_engine`
   sets `max_frame_bytes` to the largest frame the engine budgets allow: an
   open of a maximum file and password (`file_bytes + string_bytes + 14`), a
   maximum generated tile (`pixels * 4 + 18`) and a full reading response
   (`reading_response_bytes + 5`), at least 1,024 bytes for a structured
   error and at most `u32::MAX`. Under the defaults this is the value the
   viewer hard-codes, 544 MiB + 14 bytes; `decoded_bytes` no longer takes
   part.
3. **One registry owns what a document retains.** Open documents, their
   viewport generations and both page caches live in one place, so closing
   a handle releases all of them together.
4. **Every code space is an enum** in `pagura_host::protocol`: versions,
   request and response kinds, error categories, diagnostic kinds, position
   kinds, tile status, destination fits and action types, plus the open,
   search and outline bit fields (an open request's flags byte has room for
   bits that later versions assign). Each has `TryFrom` for decoding
   and its variant names. Requests decode exhaustively by kind into fields
   that remember their body offsets, and each kind names the version that
   added it, so gating is one comparison.
5. **`web/protocol_constants.mjs` is generated from those enums** and the
   default budgets. A pagura-host test fails when the checked-in file
   differs, and the browser codec imports the file instead of repeating
   numbers. The generator is Rust, not a script under `tools/gen/`, because
   the tables it emits are the Rust enums themselves; it is compiled only
   for that test and for the example behind the `codegen` feature
   (`cargo run -p pagura-host --features codegen --example
   protocol_constants`), so it is neither product code nor public API.
6. **Responses stream their bulk.** Encoding produces the length prefix and
   fields, then borrows the tile pixels or reading payload as a tail that is
   written without copying. Request bodies grow with the bytes received:
   the first allocation is 64 KiB, and each later one at most doubles what
   has arrived, never past the declared length. Both are held until the
   open and tile workloads in `web/host_benchmark.mjs` and
   `web/tile_benchmark.mjs` show no regression in time or peak memory,
   natively and on WASI.

## Consequences

- A session can bound how many documents it keeps open and how much source
  it retains, independently of the per-document budgets.
- A caller that lowered `Limits::file_bytes` to bound session memory must
  now set `SessionLimits::total_source_bytes` as well.
- Changing a wire code or a default budget in Rust fails the drift test
  until the web constants are regenerated, so the viewer cannot silently
  disagree with the host it ships with.
- Peak memory for a tile response is one copy of its pixels, and a request
  that declares a large body but sends little reserves little. A large open
  makes about log2(size / 64 KiB) reservations instead of one.
- `Session::new` takes a `SessionConfig` (`From<Limits>` keeps the old call
  shape), and `with_cache_bytes`/`with_text_cache_bytes` are replaced by
  the corresponding `SessionLimits` fields.
