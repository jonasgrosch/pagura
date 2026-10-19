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

**Known defect** (recorded 2026-10-18, corrected the same day). Pages whose
content is all opaque Normal fills take an 8-bit path that stores every mark
immediately. Its kernel works on a source colour already quantized to 8 bits,
and rounds the source and backdrop contributions separately before adding
them. So a partially covering mark can add up to about one and a half levels of
error, and the error carries into later marks. A fully covering mark leaves at
most the half level of its own colour's quantization.

Two partially covering marks after a full one can already give 217 where the
exact result is 219. A thousand slivers on one pixel give 127 instead of 5.
The first recorded version of this note claimed half a level per mark; that
bound was wrong.

The fix under study first rounds once per store from the unquantized source
colour, then bounds and removes what remains, so that the 8-bit path is
retired only once its replacement is cheap. Fixture oracles compute the exact result independently in
Python, and a fixture may carry a tolerance of 1 against it.

## Consequences

The 8-bit surface is not the final imaging model: transparency groups, soft
masks and blend modes need group surfaces with higher precision. That change
is local to composition and does not affect the display list or rasterizer
interfaces. GPU backends are out of scope until the CPU reference is complete.

## Amendment: canonical glyph polygons (2026-10-18)

Filled outline text in modes 0 and 4 retains its font outline, linear
glyph-to-device map L and exact device origin t. Construct its relative
polygon with the shared flattener at the unchanged 1/256-device-pixel
tolerance, using L for the device-space error bound. Map emitted outline
vertices by L, then add t. Instance translation never participates in
subdivision, and neither geometry nor placement is snapped to a grid. This
decision does not prescribe the subdivision algorithm. Allocation and culling
bounds combine relative geometry with device-mapped origins; they must not
erase glyph extents by adding user origins before scaling.

Preserve closure and geometric-point classification from the L-mapped source
endpoints and controls before approximation or translation. Coincident
approximating vertices do not turn a nonpoint curve into a point. A singular L
can produce a genuine point, which encloses its translated floor cell under
8.5.3.3.1. Retain the existing omission rule for a final open one-point
contour. Raster entry points and retained masks consume this metadata
directly.

Each glyph remains a separate nonzero fill, geometrically clipped and
composited in content order, including overlap and knockout (9.3.6, 9.3.8,
11.4.6). Runs share preparation without unioning coverage. Eligible glyph
rectangles use the closed-form rectangle mechanism; other contours use the
shared exact-area sweep and span interfaces. Strokes and accumulated text
clipping retain their existing path rules.

A bounded per-render cache may retain canonical geometry by retained outline
identity and actual device linear-map bits. It is separate from
path-flattening retention and never stores glyph coverage. Charge capacity and
index estimates, release optional retention before rejecting required
transient storage, and preserve semantic work charges and error positions
across hits, misses and eviction (ADR 0012, ADR 0013).

The raster oracle is the placed canonical polygon with its preserved point
rule, except for glyphs admitted to the curve sweep: their reference is the
true curve (amendment of 2026-10-19). The polygon departs from that curve by
up to its flattening tolerance, so a correctly rounded curve result can differ
from it by more than one level; for those glyphs it is a differential
reference, not the oracle. ADR 0004's composition contract remains unchanged.
No fixture tolerance is raised by this amendment. The translated-tiny-outline
correction from RGB 0 to the oracle's 255 is an explicit correctness
correction.

## Amendment: overprint preview (2026-10-18)

Overprint preview is controlled by `OverprintPreview`. Auto is the default:
simulate only when PDF/X print intent is declared by an OutputIntents
dictionary with `S /GTS_PDFX`, or a `GTS_PDFXVersion` string in Info. Catalog
and page output intents are recognized, including indirect entries. This
detects declarations, not PDF/X conformance; malformed advisory metadata is
treated as absent. Always simulates wherever overprinting affects the result.
Never ignores overprint and paints ordinary composite content, including
within groups and soft masks.

When page simulation is needed, a CMYK default process device is used while
explicit group blending spaces are retained. Named plates are preserved
through groups; soft masks use alternates. At output, each name's first
painted alternate definition is used and CMYK ink equivalents are added with
saturation before RGB conversion. At most 32 spot plates are supported, with
`Limit` above that. This is a display approximation: physical ink interaction
and destination ICC output transforms are not modelled.

An ICC-based transparency group composites in its N-component device base
without inverse ICC transforms; its sources keep their forward transforms.

## Amendment: certified accumulation (2026-10-18)

Rasterization may select certified integer accumulation or the unsnapped strip
sweep. Admission and the sweep's shape-anchored integer frame must be
deterministic and independent of tile boundaries and redundant rectangle-clip
omission. Both backends feed the common coverage-span compositor path. Revisit
the admission rule once single-outline strokes land.

Accumulation snaps flattened device geometry to a page-anchored grid of
spacing u = 2^-20 pixels, with ties-to-even and consistently rounded shared
crossings. The count n conservatively counts original flattened segments
meeting the cell, including ghosts, not merely surviving snapped pieces.

A segment whose snapped image has zero length, or becomes horizontal while its
original was nonhorizontal, remains as a zero-cover, zero-area ghost. Ghosts
count toward n and make affected cells uncertified. A snapped contour that
degenerates makes every cell touched by its closed original bounding box
uncertified. Ghost cells and cells with n > 512 use unsnapped original
geometry.

Eligible cells may use one-chain, closed-cell boundary-walk, constant-fill and
group-intersection certificates. Failed certificates use the local resolver;
snapped resolution requires passing the ghost and density guards. For
n <= 512, snapping and crossing error is at most (2.5n + 7)u; including byte
quantization, error remains below 0.813 levels. Curve flattening retains its
separate tolerance. Originally degenerate subpaths retain the existing
floor-cell policy before snapping. Tiles must assemble byte-identically.

## Amendment: bounded storage for general composition (2026-10-19)

Status: accepted, 2026-10-19.

The existing opaque renderer and its eligibility test remain unchanged.
Preserve the sole opaque RGB page-image copy ahead of general composition;
when it declines, use the selected general buffer and its actual-byte
admission. Its documented per-mark 8-bit accumulation defect is a separate
decision. These rules apply only where the general compositor runs (ISO
32000-2, 11.3.3, 11.4.4 and 11.4.6).

Choose the format before the buffer's first store, independently of the
requested device tile. Keep geometry, coverage construction, source colour
conversion, image reconstruction and prepared sampling in their existing
precision. Admit narrow storage only for RGB Normal source-over with
associative groups, including the existing opaque-member knockout identity. A
retained group, root knockout, non-RGB group, soft mask, pattern, transfer
feed, other blend mode or failed numerical bound conservatively fixes the
entire general page on f64. Active CMYK simulation devices and every spot pass
also remain f64, including RGB groups within them. Amplifying feeds use f64
from their first store. Independent narrow child buffers, promotion and buffer
pools remain deferred.

N16 stores four u16 words at 257 words per output level, using 8 bytes per
pixel. Background bytes embed as b*257. Encode a source level L=255*clamp(S)
by rounding 257*L; choose the adjacent word when necessary to preserve
round(L) at the final byte boundary. Source error is bounded by 1+2^-36 words.
For coverage byte c and source/backdrop words s,u, store
floor((c*s+(255-c)*u+127)/255), with one integer rounding. Division error is
at most 127/255 words and positive opacity is at least 1/255. Intermediate
error is consequently at most (128+2^-36)/257 levels, independently of overlap
count. Final nearest output floor((u+128)/257) adds at most half a level:
total error remains below 0.998055 levels.

F32 stores four level lanes on 0..255, using 16 bytes per pixel. Encode
inexact source levels with round-to-odd, preserving their side of a following
nearest-byte boundary; exact source levels remain exact. Source rounding costs
at most 2^-16 levels. Full-opacity stores overwrite; other stores evaluate x
+= a*(t-x) with separate roundings. Let rho=2^-17, pi=2^-16+255*6*2^-24, N
bound stores at any pixel, K count stores with varying opacity, and theta
conservatively lower-bound positive opacity of the other stores. Admit only
when pi+rho*min(N,1/theta+K) <= 1/2-2^-12. The reserved margin covers retained
f64 source-evaluation roundoff. Final nearest-byte rounding adds at most half
a level. The 270,000-store opacity-3/2^27 counterexample therefore remains
f64. Exact rounding of arbitrary translucent stacks is not promised.

Count individual glyph instances, rather than glyph runs, when bounding
stores. Preserve glyph order, canonical geometry, sparse coverage spans,
clamped rows, prepared texels and mask lookup tables. Charge planning, raster
work and actual allocated storage to existing limits. Use chunks up to 256
pixels per side, or 128 for a single-image f32 scene. Narrow storage belongs
to the page surface; wide group surfaces retain their existing representation.
Keep the existing CMYK preservation metadata, spot replay, transfer regions
and transfer_run ordering. F64 Pixel::paint and Surface arithmetic remain
unchanged. At final output, opaque RGB pixels without pending transfer or spot
processing may quantize premultiplied colour directly, skipping unit-alpha
division and background blending without changing bytes.

All-f64 composition remains the simpler alternative but forgoes measured
allocation and bandwidth savings. Final nearest rounding alone does not
prevent double rounding at source boundaries; the source encodings and
full-opacity overwrite retain nearest output for nearly all measured changed
channels. Keep independent rational fixtures, half-level source tests,
glyph-instance bounds, device/transfer exclusions, actual-byte budget tests,
tile assembly, changed-channel exact oracles, native/WASM parity and paired
per-cell timing/RSS measurements whenever admission or kernel arithmetic
changes. New optimizations must retain these bounds; CMYK and spot narrowing
require a separate proof.

## Amendment: direct glyph curves (2026-10-19)

Status: accepted, 2026-10-19.

For admitted glyphs, the coverage reference is the original outline's true
curve at its exact device position, rather than the polygon produced by
flattening it (8.5.2.2, 9.3.6; ADR 0013). All other glyphs keep the placed
canonical polygon as their reference and oracle (amendment of 2026-10-18).
The geometric flattening tolerance alone does not establish a one-level area
bound against the true curve, so independent high-precision quadrature of the
true outline must check admitted pixel areas. When a new result differs from
the polygon renderer by more than one level, compare both with that oracle;
accept an accuracy improvement only if the new result satisfies the one-level
contract and is closer to the oracle at every such pixel. The old renderer is
a differential reference, not the accuracy oracle.

Admitted glyphs are covered by analytically integrating monotone quadratic
pieces in f64. Only exactly quadratic outlines qualify: TrueType quadratics,
or cubics whose third difference -P0 + 3P1 - 3P2 + P3 is at most 2^-24 device
pixels per coordinate, as degree elevation leaves it after rounding. Their
midpoint quadratics move the curve by less than 2^-27 pixels, so 32 monotone
pieces change no pixel by more than 2^-21 of its area. A general cubic's
midpoint quadratic is not an area certificate: displacements below 1/1024
pixel on several edges sharing a pixel exceed one level. Cubic outlines
therefore keep certified polygons.

Canonical pieces are cached by outline and linear device map beside the
fallback glyph polygon. Coverage is evaluated afresh at each exact placement.
Admission uses the complete glyph and retained clip stack, independently of
tiles and cache hits: at most 32 source segments and monotone pieces, more
than four polygon vertices per piece, a curved piece, only quadratic curves,
no point contour, glyph paint and rectangular clips only. Other fills,
strokes, cubic or complex glyphs and nonrectangular clips keep the existing
rasterization path. The curve sweep is separate from the line sweep, so fills
without curves run the line sweep unchanged.

## Amendment: direct opaque images (2026-10-19)

Status: accepted, 2026-10-19.

A page whose only mark is one opaque DeviceGray or DeviceRGB image, with an
RGB page model, no separations, clip, transfer, mask, matte, overprint or
non-Normal composite, may be painted straight into the 8-bit output instead
of through a page buffer (11.3.3). Its Decode ranges must be [0 1] or [1 0],
so every sample code has an integer level: 255 / (2^bits - 1) times the code,
or 255 minus that (8.9.5.2). The background must be opaque.

The reference is unchanged: the image sampling reference above, with the
general compositor's footprints, then Normal composition of the pixel's
coverage byte over the background in exact arithmetic. Minified placements
that are upright or quarter turns take integer sums of interior codes and
f64 products for the two end cells of each footprint, then scale the mean
code to a level and mix a partial pixel with the background in f64, rounding
once. Magnification takes the nearest sample, or bilinear weights with
/Interpolate, from the same pixel centres as the sampler. Like the general
compositor's f64 evaluation, only the f64 geometry, end weights, row accumulation and normalization depart
from exact arithmetic, by far less than a level: a byte can differ from the
reference rounding only when the exact value lies that close to a half
level, and stays within one level. Decoders keep their byte-exact samples.
Other placements, images and pages use the general compositor.

Fixtures and unit tests compare against independent rational oracles,
including two-to-one half-level ties, quarter turns, inverted one- and
two-bit samples, partial coverage and bilinear magnification. Changed pixels
against the general compositor must be reported with their oracle values.
