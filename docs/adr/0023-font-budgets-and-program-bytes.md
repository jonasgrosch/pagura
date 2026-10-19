# ADR 0023: Font budgets, format ceilings and borrowed program bytes

Status: accepted, 2026-10-18. Keeps the outline type of ADR 0009.

## Context

A review of `pagura-font` found:

- **Copied static data.** `FontProgram::parse(impl Into<Arc<[u8]>>)` copied
  `include_bytes!` data to the heap: about 6 MB for the bundled standard 14
  fonts and 8 MB per CJK font, each byte resident twice.
- **The raster dependency.** The crate depends on `pagura-raster` for
  `Path`, `PathBuilder`, `Point`, `Segment` and `Transform`. The review asked
  whether an `OutlineSink` trait should replace it.

## Decision

1. **Static program bytes are borrowed.** `parse` takes
   `impl Into<FontBytes>`, an enum of `Static(&'static [u8])` and
   `Shared(Arc<[u8]>)`. `&'static [u8]` converts without copying, so the
   bundled-data call sites stop copying with no change. `retained_bytes`
   counts shared input only: static data is not heap.
2. **Outlines stay `pagura_raster::Path`** (ADR 0009, ADR 0022). An
   `OutlineSink` was considered and declined. The font matrix and the
   composite and seac transforms still need `Point` and `Transform`, so the
   crate would keep the raster types or need its own copies; the renderer
   caches outlines as `Arc<Path>` and would build the same path through the
   sink; quadratics already convert to cubics exactly; and raster depends
   only on `pagura-core`, so the dependency adds build order, not coupling.

## Consequences

Bundled fonts are no longer resident twice.
