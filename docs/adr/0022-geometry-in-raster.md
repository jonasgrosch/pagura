# ADR 0022: Geometry stays in `pagura-raster`; raster owns byte quantization

Status: accepted, 2026-10-18. Confirms the `pagura-raster` responsibility in
`docs/ARCHITECTURE.md` and the outline type of ADR 0009.

## Context

A workspace review noted that geometry is split across crates: `Rect` lives
in `pagura-core`, while `Point`, `Transform`, `Path`, `PathBuilder` and
`Segment` live in `pagura-raster`. `pagura-font` and `pagura-text` depend on
the rasterizer only for those types. The review called this acceptable
while raster depends on nothing but core, and asked for a decision.

The same review found two rounding conventions for converting a float in
`0..=1` to a byte (`.round()` in the renderer's colour code, half-up with a
1e-9 guard in coverage and the compositor), and CONTRIBUTING.md now requires
one shared helper in `pagura-raster`.

The options for geometry were:

1. **Keep it in `pagura-raster`.** No call site changes.
2. **Move it to `pagura_core::geometry`,** next to `Rect`.
3. **A new `pagura-geometry` crate** below raster, font and text.

## Decision

1. **Point, vector, transform and path types stay in `pagura-raster`.**
   - Raster depends only on `pagura-core`, is PDF-agnostic and forbids
     unsafe code, so a dependent gains no PDF, I/O or decoder coupling;
     the cost is build order only.
   - The path types encode what the rasterizer consumes: every subpath
     starts with `MoveTo`, a line or curve needs a current point
     (`NoCurrentPoint`), and `Segment` is closed by the PDF path operators
     (8.5.2). They change together with flattening and stroking.
   - Moving them into core would add rendering vocabulary to the contract
     that every parser crate depends on (ADR 0017 makes core changes
     workspace-wide reviews), although most of those crates (codec, crypto,
     document, image, write, pki) never use a point or a path.
   - A geometry crate would add a crate for a few hundred lines and would
     still leave `Rect` in core, because `Rect::from_array` reads PDF
     values.
   - If `pagura-font`'s dependency on the rasterizer needs to go, the font
     crate can emit outlines through a sink trait of its own that a
     downstream crate implements for `PathBuilder`; that is the font
     crate's decision and needs no geometry move.

   Revisit when a crate that must not depend on the rasterizer (the
   document model, the writer) needs points or transforms. Then `Point`
   and `Transform` move to `pagura_core::geometry`, and `pagura-raster`
   re-exports them so that callers keep compiling.
2. **Byte quantization belongs to raster.** `pagura_raster::quantize_unit`
   is the one conversion from a unit-interval float to a byte: clamp to
   `0..=1`, scale by 255, round half up with a 1e-9 guard so analytic
   half-pixel ties survive cancellation, and map NaN to 0. Coverage uses it
   now; the renderer's colour and compositor code adopt it in their own
   change. Raster also keeps the one byte product `round(a × b / 255)`
   used for premultiplication and compositing.

## Consequences

The crate graph in `docs/ARCHITECTURE.md` stays as it is. Raster's public
API keeps the geometry vocabulary next to the rasterizer that consumes it;
callers that need a rectangle path use `Path::rect`. Colours, coverage and
composited pixels round the same way once the renderer adopts
`quantize_unit`, so a value can no longer quantize differently as a mask
and as a colour.
