# ADR 0017: Shared parsing primitives in `pagura-core`; workspace conventions

Status: accepted, 2026-10-18. Amends ADR 0003 (where tags are defined).

## Context

A workspace review found the low-level code rigorous but the layer between
`pagura-core` and the feature crates duplicated, and several copies had
drifted into real bugs:

- about twenty budget counters (`charge(work, n, max)` free functions and
  five private `Budget`/`Work` structs) with different overflow handling;
- three big-endian byte cursors that disagree on whether offset overflow is
  a `Limit` or a `Syntax` error, and six MSB-first bit readers;
- two PDF text-string decoders with different semantics plus a third
  PDFDocEncoding table, so the same bytes read differently as an outline
  title and as `/ActualText`;
- seven repair helpers, only two of which coalesce repeated repairs, and
  five copies of "if the position is in a stream, record that object";
- `Diagnostic::at` overwriting context that `Error::at` keeps, and
  `Diagnostics::extend` bypassing strict mode.

The same review found choices that each crate made differently: module
layout, test placement, tag placement and spelling, error positions,
`Limits` passing, public-API evolvability and clause citations.

## Decision

1. **The shared primitives live in `pagura-core`,** next to the vocabulary
   they serve, rather than in a new utility crate:
   - `Meter`, the incrementally charged budget (ADR 0016);
   - `bytes::Cursor` and `bytes::BitCursor<S: Stuffing>` with `Plain`,
     `JpegFf00` and `J2kFf7` stuffing;
   - `text_string::{decode, encode, encode_pdf_doc}`, deciding strict or
     reported repair through the caller's `Diagnostics`;
   - `Diagnostics::{repair_once, absorb}`, `Error::{within, in_object_opt,
     with_repair}`, the same builders on `Diagnostic`, and `ResultExt`;
   - `convert`, lossless integer widening;
   - `repairs::ALL`, the shape every crate's tag modules take.

   Every crate already depends on `pagura-core`, the primitives use only
   `std`, and none knows about files, documents or rendering, so they fit
   the crate's responsibility without changing the dependency graph. A
   separate `pagura-util` crate would add a layer with the same dependents
   and the same direction, and would split the error vocabulary from the
   readers that produce it.
2. **Context builders keep the innermost context** on both `Error` and
   `Diagnostic`: a position or object recorded close to the fault is the
   most precise one, and outer callers add context only where none is
   known. `at` sets only the position, because inside an object stream the
   position names the stream while the object is the compressed object
   being parsed. `within`, which places a codec's buffer offset in the
   stream it decoded, also records that stream as the object.
3. **Public types of `pagura-core` are evolvable.** `ErrorKind`, `Position`,
   `Origin`, `Recovery`, `DiagnosticKind`, `Error`, `Diagnostic` and
   `DirectStream` are `#[non_exhaustive]`; `Rect`'s fields are private. `Value` and
   `PathComponent` stay exhaustive because their variant sets are closed by
   the standard, and `Limits` stays a plain record (ADR 0016).
4. **Tags** are defined in each crate's `pub mod features` and
   `pub mod repairs`, each with `ALL`, instead of a `tags` module as ADR
   0003 said. Each tag area has one owning crate.
5. **The conventions in `CONTRIBUTING.md`** (modules, tests, tags,
   recovery, budgets, binary input, text strings, types, errors,
   signatures, conversions, citations, CI, generated tables, benchmarks)
   are the rules review applies.

## Migration

`pagura-core` and `pagura-testkit` change first. Each other crate then
replaces its copies with the shared primitives, moves its borrowed `Limits`
uses to the ADR 0016 fields and adopts the conventions in its own change.
`unreachable_pub` is enabled per crate as each is cleaned up and moves to the
workspace lint table once every crate passes it.

## Consequences

A fix to a reader, a budget or the text-string codec reaches every format at
once, and the drift bugs found by the review disappear as crates migrate.
`pagura-core` grows by the primitives (about 1,400 lines including their
tests)
and its public API becomes a longer-lived contract: changes to it affect
every crate, so they need the same review as other architecture changes.
Adding a variant to a core enum no longer breaks other crates at compile
time, so code that maps core enums to external codes (the host protocol)
must handle unknown variants explicitly.
