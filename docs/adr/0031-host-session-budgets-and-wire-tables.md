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

## Consequences

- A session can bound how many documents it keeps open and how much source
  it retains, independently of the per-document budgets.
- A caller that lowered `Limits::file_bytes` to bound session memory must
  now set `SessionLimits::total_source_bytes` as well.
- `Session::new` takes a `SessionConfig` (`From<Limits>` keeps the old call
  shape), and `with_cache_bytes`/`with_text_cache_bytes` are replaced by
  the corresponding `SessionLimits` fields.
