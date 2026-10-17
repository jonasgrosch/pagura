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

Exact-area antialiasing has an analytic answer: the covered area of each
pixel. Fixture oracles are computed independently in Python by clipping the
polygon to each pixel square. Rasterizer output must match within ±1 level
of 255 for polygons, and within a documented flattening tolerance for curves.
Pixel-aligned opaque cases stay exact.

## Consequences

The 8-bit surface is not the final imaging model: transparency groups, soft
masks and blend modes need group surfaces with higher precision. That change
is local to composition and does not affect the display list or rasterizer
interfaces. GPU backends are out of scope until the CPU reference is complete.
