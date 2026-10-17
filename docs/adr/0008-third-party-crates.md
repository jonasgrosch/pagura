# ADR 0008: Third-party crates only by evaluated exception

Status: accepted, 2026-10-17. Refines ADR 0001.

## Context

Pagura's product crates have used only `core`, `alloc`, `std` and each other.
Some areas — JPEG 2000, JBIG2, font programs, colour management — are large,
intricate formats where an independent rewrite costs a great deal and carries
its own risk of defects. Reusing a mature, safe crate can be the better
engineering choice there.

## Decision

`core`, `alloc` and `std` are always available. A third-party crate may enter a
product crate only when all of the following hold, recorded in an evaluation
in `docs/dependencies/<crate>.md` and approved for that crate and version:

1. **Need.** An independent implementation would exceed roughly 20,000
   lines or carry unusually high risk; around 10,000 lines is ordinary work
   and is implemented in Pagura (font programs, JBIG2 and JPEG 2000 decoders
   fall in that range). The crate covers a self-contained format or
   algorithm with no connection to PDF parsing: Pagura remains an independent
   PDF engine (ADR 0001), so no crate from a PDF engine (hayro and its
   components, pdf.js ports, PDFium or MuPDF bindings) is used.
2. **No unsafe code.** Neither the crate nor any transitive dependency
   contains `unsafe` code, FFI or build scripts; each crate root forbids
   unsafe code or is verified free of it. `tools/policy.py` enforces this for
   every locked dependency.
3. **Licence.** MIT, Apache-2.0, BSD, ISC, Zlib or another licence compatible
   with Pagura's `MIT OR Apache-2.0`; never copyleft.
4. **Portability and bounds.** It builds for `wasm32-wasip1`, needs no host
   capabilities, and lets Pagura bound its work and memory (or is wrapped so
   that hostile input cannot exceed Pagura's `Limits`). It is covered by
   Pagura's fuzz targets.
5. **Maintenance.** Actively maintained, versions pinned exactly in
   `Cargo.lock`, updated deliberately to the latest release.

The default remains an independent implementation; the evaluation must show
why the exception is worth its supply-chain cost.

## Consequences

`tools/policy.py` checks product dependencies against an allowlist instead of
rejecting all of them. Each accepted crate is part of the trusted base and is
reviewed again on every version update.
