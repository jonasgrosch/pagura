# ADR 0028: Colour management in `pagura-color`; one session per page compilation

Status: accepted, 2026-10-18. Refines the crate table of
`docs/ARCHITECTURE.md` and the rendering pipeline of ADR 0004.

## Context

ICC profile parsing and evaluation (about 1,300 lines) and the CIE
colorimetry of the calibrated spaces lived in `pagura-render`. The profile
code needed nothing from PDF beyond `pagura-core` and MD5, yet it was
reachable only through page rendering. Lab to XYZ, Bradford adaptation and
the XYZ to sRGB matrix existed once for ICC and once for `CalGray`, `CalRGB`
and `Lab`, with different knee tests. ARCHITECTURE already named colour
management as the next crate.

Inside the renderer, every content context was a copy of one interpreter.
Forms, annotation appearances, tiling cells, soft masks and Type 3 glyphs
were interpreted by a `detached()` copy, and each copy started with its own
budgets and caches. An annotation appearance therefore began with fresh
`decoded_bytes`, image-cache and operation budgets, so N annotations could
retain N budgets of images. Graphics state also carried the font cache, the
extraction output and the text object, so `q`/`Q` and child contexts copied
state that is not graphics state.

## Decision

1. **`pagura-color` owns colour management without PDF objects.** It holds
   ICC.1:2010 forward transforms (`Profile::parse(bytes, components,
   &Limits)`, `Profile::convert`, `Intent`) and the CIE colorimetry the
   calibrated spaces share (`cie::Calibrated` for `CalGray`, `CalRGB` and
   `Lab`; adaptation; XYZ to sRGB). It depends on `pagura-core` and
   `pagura-crypto` (profile IDs) only, reads profiles through
   `bytes::Cursor` and charges `decode_work` and `decode_bytes` (ADR 0016).
   It owns the `colorspace.` tag area. `pagura-render` resolves colour-space
   objects, caches profiles by object identity in a `ColorContext`, and
   reports evaluation errors in the profile's object.
2. **One `Session` per page compilation.** Everything that is
   compilation-wide lives in the session and is borrowed by every content
   context: the operation, mark and retained-byte meters (`operations`,
   `display_commands`, `render_bytes`), the profile and paint caches, the
   optional-content configuration and the extracted text. A content context
   is a `Compiler` created by `Compiler::child` with a `Child` (form, soft
   mask, tiling cell or Type 3 glyph) that states what the child inherits.
   Graphics state holds only what `q`/`Q` save; the text object, Type 3
   procedure and extraction scope are fields of the context.
3. **Content is tokenized and decoded once.** `operators::token` splits all
   content, appearance strings included, into operands and operator words;
   `Operator::decode` checks each operator's operands and returns a typed
   `Operation`. Handlers never index into the operand list.
4. **Document-bound resources are shared.** The provider, the font cache and
   the default optional-content configuration form the `Shared` resources a
   session borrows. `RenderContext` keeps them across pages; the free
   compile functions create them for one call.

## Consequences

ICC parsing can be fuzzed and tested from profile bytes alone, and another
crate can adopt it without depending on the renderer. `pagura-render` no
longer depends on `pagura-crypto`. A page's budgets cannot reset inside the
page: every child context charges the same meters, and cached images and
patterns stay charged once. Adding a context kind means adding a `Child`
variant, so what it inherits is reviewed in one place.

The compositor counts its own surfaces and caches (`Context::memory`), on
`decoded_bytes` until the rasterizer's retained buffers move to
`render_bytes`. Function, calculator and sampled
evaluation work charge `function_work` (ADR 0016).
