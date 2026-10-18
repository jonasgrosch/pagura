# ADR 0004: Rendering pipeline — display list, exact-area rasterizer, tiles

Status: accepted, 2026-10-17.

## Decision

Rendering has three stages with one-directional dependencies:

1. **Interpretation** (`pagura-render`): content streams are read as one token
   sequence across all `/Contents` streams (clause 7.8.2), carrying operand
   and graphics state across stream boundaries. The interpreter produces a
   **display list in PDF terms**: fills and strokes of paths in user space
   with their CTM, fill rule, stroke parameters, clip stack and colour *in
   its source colour space*. Device colour conversion happens at paint time.
   Every operator in ISO 32000-2 Annex A is recognized; unimplemented ones fail
   with `Unsupported(operator.<name>)`, unknown ones are syntax errors except
   inside `BX`/`EX`.
2. **Rasterization** (`pagura-raster`): PDF-agnostic. Paths are flattened to
   lines with a device-space tolerance, then filled by **exact-area coverage
   accumulation** (signed area per pixel cell, accumulated along scanlines),
   for nonzero and even-odd rules. Strokes are converted to fill outlines in
   user space and then transformed, so non-uniform CTMs distort the pen as PDF
   requires; zero-width lines render one device pixel wide. Clipping uses
   coverage masks intersected along the clip stack.
3. **Composition** into a premultiplied RGBA8 `Pixmap`, source-over, onto
   the requested background.

The public API is `render_page` and `render_tile(document, page, options,
tile)`. A tile is a device-pixel rectangle; the device transform includes the
tile offset, so tiles of one page assemble exactly into the full page. Tiles
are the unit of caching, progressive display and future parallelism
(`std::thread::scope` natively, `wasm32-wasip1-threads` in the browser).

## Colour

DeviceGray, DeviceRGB and DeviceCMYK use the conversions of clause 10.4
until ICC-based colour management exists. A page using `ICCBased`,
`Separation`, `DeviceN`, `Lab`, `Indexed` or default colour spaces fails with
the corresponding `colorspace.*` tag until implemented.

## Accuracy and oracles

Every rendered pixel has an exact answer, and one contract covers every
stage (amended 2026-10-18): each pixel is within one level of 255 of its
exact result.

- **Rasterization:** the exact covered area of each pixel (the path clipped
  to the pixel square), with curves within a documented flattening
  tolerance.
- **Image sampling:** the exact-area average of the source samples under the
  image transform, or the nearest sample where 8.9.5.3 asks for no
  interpolation.
- **Composition through non-amplifying operations:** the imaging model of
  clause 11 applied in exact arithmetic to the same coverage bytes, for any
  number of overlapping marks. The non-amplifying operations are Normal
  compositing, alpha, shape and soft-mask application, and the separable
  blend modes Multiply, Screen, Darken, Lighten, Difference and Exclusion.
  Their results never change faster than their inputs, so rounding errors
  only add up. Per-mark 8-bit accumulation does not hold this bound.
- **Composition through amplifying operations** (amended 2026-10-18): some
  operations can change faster than their inputs, so no finite precision
  bounds them against exact arithmetic for every input. These are
  ColorDodge, ColorBurn, Overlay, HardLight, SoftLight, the non-separable
  blend modes (11.3.5), steep transfer functions, and knockout's division by
  shape. Every buffer that feeds one is evaluated in f64 from its first
  store, and the contract is one level of that documented f64 evaluation.
  The renderer's classification of each operation, with its amplification
  bound, is the reference for which side an operation is on.

Pixel-aligned opaque cases stay exact, and exact rounding ties are not part
of the contract.

**Known defect** (recorded 2026-10-18). Pages whose content is all opaque
Normal fills take an 8-bit path that stores every mark immediately. A fully
covering opaque mark makes a pixel exact again. Each partially covering mark
then rounds once and adds at most half a level. After `n` partially covering
stores since a pixel's last full coverage, its output is within `n/2 + 1/2`
levels of the exact rounded result:

- two such stores keep the contract;
- from the third onward, a pixel can drift by up to half a level per store.

Real pages rarely put more than two partially covering marks on one pixel.
A crafted page with a thousand slivers on one pixel gives 127 where the exact
result is 5. The fix under study counts partial stores per pixel and promotes
before the third, so that the 8-bit path is retired only once its replacement
is cheap. Fixture oracles compute the exact result independently in
Python, and a fixture may carry a tolerance of 1 against it.

## Consequences

The 8-bit surface is not the final imaging model: transparency groups, soft
masks and blend modes need group surfaces with higher precision. That change
is local to composition and does not affect the display list or rasterizer
interfaces. GPU backends are out of scope until the CPU reference is complete.
