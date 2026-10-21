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
   requires; zero-width lines render one device pixel wide. The paths of the
   clip stack stay geometry: each mark integrates them together with its own
   outline, and a clip region's own coverage is integrated only for a shading
   without a bounding box (amended 2026-10-20).
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

Exact uses the stage-specific reference models below (amended 2026-10-18).
Its one-level bound is relative to the applicable model. In particular,
composition uses already quantized coverage bytes; this does not establish an
end-to-end one-level bound against unquantized coverage or a filtered continuous
scene. Those are different references, as the
[coverage-correlation research cases](../plans/correlated-coverage-research.md)
illustrate. This clarification does not change a gate or reference model.

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

## Amendment: competitive accuracy gate (accepted, 2026-10-20)

Status: accepted, 2026-10-20.

The one-level contract stays the reference and the default. An approximate
path, one that can leave a pixel more than one level from its exact result,
may become the default only when it meets the absolute goals below on the
40-page benchmark set at 72, 150 and 300 dpi. The goals are fixed numbers.
They were derived on 2026-10-19 from the best competitor in each cell, not
from any single engine, measured on main 99bfb290 against the then-latest
releases (hayro 0.8.0, PDFium 156.0.8076 via pypdfium2 5.14.0, MuPDF 1.28.5 via
PyMuPDF 2.0, pdf.js 6.4.299, Poppler, Quartz on macOS 15.8), and do not move
when competitors change.

**Accuracy goals.** Measured against Pagura's exact rendering of the same
page, pixel-weighted per category and DPI. Each value is at least 10% better
than the best competitor's value for that measure, rounded to a round number
on the ambitious side:
- MAE: mean absolute RGB error;
- within ±1: share of pixels whose largest channel error is at most one;
- P99.9: 99.9th percentile of the largest channel error.

No page may be less accurate than the best competitor on that page. The
maximum error is not gated, because every engine reaches 231–255 on some
pages.

| Category | DPI | MAE ≤ | Within ±1 ≥ | P99.9 ≤ |
|---|---:|---:|---:|---:|
| Text | 72 / 150 / 300 | 1.0 / 0.5 / 0.3 | 93 / 96 / 98% | 60 / 40 / 30 |
| Vector | 72 / 150 / 300 | 0.6 / 0.15 / 0.05 | 95 / 99.2 / 99.6% | 50 / 30 / 20 |
| Mixed | 72 / 150 / 300 | 1.5 / 0.75 / 0.25 | 92 / 94 / 98% | 110 / 90 / 40 |
| Images | 72 / 150 / 300 | 3.0 / 0.75 / 0.4 | 85 / 94 / 98.5% | 80 / 60 / 90 |

Each value is the competitors' best figure for that measure, improved by at
least 10% and then rounded towards the more ambitious side.

The best competitor differs by cell. It is hayro for most text and vector
cells, MuPDF for vector at 72 dpi, and pdf.js for images at 72 and 150 dpi,
mixed at 150 dpi and some percentiles. The engines considered were PDFium,
MuPDF, hayro, pdf.js, Poppler and Quartz.

**Speed goals.** For each category and DPI, the geometric mean over the
category's documents of Pagura's engine time (open plus every page render)
must be at most the value below, in milliseconds, on each benchmark runner.
Each value is 0.9 times the fastest competitor on that runner, rounded down
in 0.5 ms steps below 10 ms, 1 ms steps up to 30 ms and 5 ms steps above. The
fastest competitor differs by runner. On M4 it is hayro in every cell. On x86
it is PDFium for text at 72 and 150 dpi and for all vector, MuPDF for images
at 72 dpi, and hayro otherwise. On Graviton it is PDFium for text, vector and
mixed, MuPDF for images at 72 dpi, and hayro for images at 150 and 300 dpi.

| Category | M4 (ms ≤) | x86 (ms ≤) | Graviton (ms ≤) |
|---|---|---|---|
| Text | 5.5 / 8 / 7.5 | 11 / 20 / 26 | 9.5 / 15 / 19 |
| Vector | 3 / 4.5 / 9 | 6 / 11 / 29 | 5.5 / 8.5 / 19 |
| Mixed | 4 / 6.5 / 7 | 9.5 / 17 / 23 | 8.5 / 14 / 17 |
| Images | 13 / 19 / 16 | 25 / 45 / 50 | 27 / 40 / 40 |

The values are for 72 / 150 / 300 dpi, from the 2026-10-19 runs on main
99bfb290.

**Ratchet.** Both margins tighten over time from 10% towards the original
20%. Each tightening adds a new dated table to this section. A path that
falls behind a tightened table returns to the exact default.

**Unchanged.** Rendering stays deterministic and byte-identical across
platforms and tile layouts. Work and storage stay bounded and charged to
`Limits`. No `unsafe` code or dependency exceptions are introduced (ADR 0008).
Conformance fixtures keep their independent oracles. Where an approved
approximate path changes a fixture, the fixture records that path's measured
bound, listed with this amendment. Every approximate path is named, can be
switched to the exact path through `RenderOptions`, and its per-category
accuracy table is kept with the change that introduces it.

## Amendment: signed-area glyph coverage (2026-10-19)

Status: accepted, 2026-10-20.

A glyph whose canonical polygon has a certified winding may be covered by
signed-area accumulation in f64, in place of certified integer accumulation
and the strip sweep. Its reference and oracle stay the placed canonical
polygon (amendment of 2026-10-18). Glyphs admitted to the curve sweep keep it
and its true-curve reference. Rectangles keep closed-form coverage. Point
contours, uncertified glyphs and glyphs under nonrectangular clips keep the
existing paths. Selection depends only on the canonical geometry and the
retained clip stack.

**Certificate.** It is computed once per canonical geometry and cached with
it. Each contour splits into strictly y-monotone chains; horizontal edges
enclose no area. Between consecutive chain-end heights, the chains crossing the
slab must alternate in direction from the left, starting with one sign s
common to the glyph. Chains adjacent at the slab's middle must not meet
inside the slab, since the first meeting in a slab is between such neighbours
(Shamos and Hoey). Neighbours are compared at every vertex height of either
one, or skipped when their x ranges within the slab are separated. An
interpolated comparison must exceed 2^-45 M, where M >= 1 bounds the
coordinates; that is more than 2^8 times its rounding error. Only identical
vertices may touch, and only at slab ends. The winding is then 0 or s almost
everywhere, so each cell's |signed area| is its nonzero area. Any doubt fails
the certificate. The sweep stops and leaves the glyph uncertified once the
chains crossing its slabs, counted per slab, plus its segment steps exceed
32E + 1024 for E edges, so its work stays linear in E even for geometries
the cache does not keep.

**Kernel.** With t the exact device origin and f = t - floor(t), a vertex
x maps to X = (x + f_x) - c, where c = floor(x_min + f_x); y maps likewise.
Rounding is monotone, so every placed vertex stays within the frame. Edges
are clipped to integer rows. Each row piece adds the exact trapezoid areas of
the cells it crosses as prefix differences. One prefix sum per row, from the
glyph's first column, gives each cell's signed area, and |area| is quantized
once with the existing quantizer. A cell depends only on the geometry, t and
the cell. Tiles, chunks and integer clip rectangles therefore cut the result
exactly. Cells cut by a fractional rectangle-clip edge take the polygon path's
geometric clip. Every other cell keeps the kernel's value, whichever tile or
redundant-rectangle omission produced it. Rows are processed in bands of at
most 2^16 accumulators. Work charges count vertex visits, row-piece columns
and band cells, independently of the geometry cache.

**Error bound.** Let u = 2^-53, E the polygon's vertex count, C its chain
count, B >= 1 its largest relative coordinate, W = floor(width) + 4 and
H = floor(height) + 2. To first order in u:

- Placing a vertex errs by at most u(B + W + 1) in x and u(B + H + 1) in y. A
  row crossing interpolates within 6uW more, and a single-column piece's
  midpoint within u(W + 1).
- Moving a piece's ends by dx and dy changes its contribution to any prefix
  by at most |d| dx + 2 dy, where d is its signed height and |d| <= 1. A row
  has at most E pieces.
- The computed terms of one piece err by at most 32u|d| in total. Each chain
  crosses a row band once, so the |d| of one row sum to at most C.
- A row's prefix sums add at most E W terms of total magnitude at most C, so
  summation errs by at most E C W u.

So a cell's area errs by at most u(E(3B + 8W + 2H + 4) + 32C + E C W).
Admission requires twice this to be at most 2^-20 pixel area, and W <= 2^16.
Each byte is then within 0.5 + 255 * 2^-20 + 10^-9 < 0.50025 levels of the
exact area of the placed polygon. Underflow adds at most 2^-1074 per
operation. On the 40 benchmark pages the bound admits every certified glyph.

Fixtures and unit tests compare against independent per-cell polygon
clipping. Changed cells against the previous backends must be reported with
their oracle values; on the 40 benchmark pages at 72, 150 and 300 dpi, every
non-tie change moved a cell to the correctly rounded exact area.

## Amendment: the competitive path (2026-10-20)

Status: accepted on main, 2026-10-21. This records the original Competitive
policy. The later shared-composition, background and F32-preservation amendments
below supersede its storage choices on this experimental branch; main's
acceptance does not admit those experiments or their performance gates.

`RenderOptions::accuracy` selects the path: `Accuracy::Exact`, the default,
keeps the one-level contract; `Accuracy::Competitive` (CLI `--accuracy
competitive`) is the approximate path measured against the gate above. It
takes, per primitive, the faster approximation that keeps the gate:

- **Glyphs** of a solid colour in byte storage are masks from row fragments.
  An outline is flattened once per linear map, its edges are clipped to rows
  once per vertical origin, and those fragments are reused across horizontal
  origins. Origins snap to 1/128 pixel horizontally and, vertically, to
  1/128 pixel below 108 dpi, 1/16 pixel from 108 dpi and 1/64 pixel from
  216 dpi. Curves become 0.15-pixel chords below 108 dpi and two
  area-balanced chords per 1/4-pixel piece from 108 dpi. Masks are a pure
  function of their key and are retained per rendering context, at most
  16 MiB within `Limits::document_bytes` (ADR 0012).
- **Other fills and strokes** accumulate signed areas in f32 cells after
  flattening curves to 1/64-pixel chords. Cells are indexed in global device
  columns, so any tile partition assembles into the page. Rectangles,
  degenerate contours and compound paths without a retained stroke boundary
  keep geometric integration.
- **Strokes** become pieces that meet without overlapping, retained as the
  stroke boundary: each segment's quadrilateral ends at the inner miter
  point on the inner side of a turn and at its normal on the outer side, the
  outer wedge takes the join, and a round cap is the half disc beyond its
  endpoint. The pieces cover the components' set once, so signed cells
  integrate them without a union. A stroke keeps its overlapping components,
  and so geometric integration, when a run reverses, turns tighter than the
  pen around a short segment, has an open centreline whose directions span a
  half turn or a closed one that is not convex, or when the bounding boxes
  of its runs' pens meet.
- **Clips.** A clip path that contains a mark's global control hull is
  dropped for that mark. A quadrilateral mark within quadrilateral clips is
  integrated with them geometrically. A rectangular clip region's coverage
  is a column factor times a row factor; any other clip region's own cell
  coverage is integrated once per band. Either is multiplied into each mark,
  which approximates the intersection where both boundaries cross one pixel.
- **Composition.** Where the exact path would admit 16-bit or f32 storage,
  Normal composition of an opaque page rounds each store once into one byte
  page, painted in full-width bands of 256 rows. Retained groups, blend
  modes, soft masks and other compositing keep the general compositor.
- **Images.** A page whose only mark is one opaque DeviceGray or DeviceRGB
  image, upright or turned by a quarter, is decoded, converted and filtered
  in one pass straight into the output, within one level of the exact
  result. On byte pages, the fully covered interior of an opaque image, or
  one with a soft mask, takes the exact integer area sampler of the direct
  opaque images amendment, for its colour and for its mask's levels, rounded
  to bytes and composed once; a matte is first removed from each sample, as
  the image sampling reference does. A one-sample stencil mask is painted as
  a fill of its unit square. Boundary pixels and every other image keep the
  image sampling reference. An opaque DeviceGray or DeviceRGB DCTDecode
  image without `Mask`, `SMask` or `Decode`, drawn from page content and not
  as the page's sole mark, is reconstructed near its drawn size: with 1, 2,
  3, 4 or 6 cells per eight-sample block axis, the fewest that leave two and
  a half cells per device pixel along its less minified axis. A cell is the
  mean of its block's unclamped T.81 A.3.3 samples from an integrated cosine
  basis, rounded once; with six cells, a block that may leave the sample
  range takes the means of its exact samples. The samplers then take the
  cells, the image's unit square spanning its declared extent in them.
- **Fonts.** Embedded TrueType programs are decoded only as far as drawing
  reads them; damage in later tables is neither examined nor reported.

Rendering stays deterministic and byte-identical between native and
WebAssembly builds (with and without SIMD128) and across tile layouts. Work
and storage are charged to `Limits`; a page that exceeds this path's own
working budgets renders exactly.

**Accuracy** against the exact path on main a48e5ecc, pixel-weighted per
category and DPI over the gate's pages (the predeclared 20-page subset at
300 dpi). MAE is over RGB; within ±1, P99.9 and the maximum are over each
pixel's largest channel error.

| Category | DPI | MAE (goal) | Within ±1 (goal) | P99.9 (goal) | Max |
|---|---:|---|---|---|---:|
| Text | 72 | 0.408 (≤ 1.0) | 94.09% (≥ 93%) | 19 (≤ 60) | 255 |
| Text | 150 | 0.116 (≤ 0.5) | 97.37% (≥ 96%) | 7 (≤ 40) | 255 |
| Text | 300 | 0.040 (≤ 0.3) | 99.08% (≥ 98%) | 5 (≤ 30) | 255 |
| Vector | 72 | 0.032 (≤ 0.6) | 99.59% (≥ 95%) | 9 (≤ 50) | 213 |
| Vector | 150 | 0.009 (≤ 0.15) | 99.84% (≥ 99.2%) | 3 (≤ 30) | 255 |
| Vector | 300 | 0.006 (≤ 0.05) | 99.89% (≥ 99.6%) | 2 (≤ 20) | 255 |
| Mixed | 72 | 0.140 (≤ 1.5) | 97.99% (≥ 92%) | 15 (≤ 110) | 255 |
| Mixed | 150 | 0.051 (≤ 0.75) | 98.85% (≥ 94%) | 6 (≤ 90) | 255 |
| Mixed | 300 | 0.016 (≤ 0.25) | 99.65% (≥ 98%) | 3 (≤ 40) | 255 |
| Images | 72 | 0.004 (≤ 3.0) | 100.00% (≥ 85%) | 1 (≤ 80) | 1 |
| Images | 150 | 0.011 (≤ 0.75) | 100.00% (≥ 94%) | 1 (≤ 60) | 1 |
| Images | 300 | 0.000 (≤ 0.4) | 100.00% (≥ 98.5%) | 0 (≤ 90) | 1 |

No page is worse than the best of PDFium, MuPDF, hayro, pdf.js, Poppler and
Quartz on that page in MAE, within ±1 or P99.9, at any of the three
resolutions, over all 40 pages (39 at 300 dpi: one page has no competitor
image there).

**Fixtures.** Every render fixture stays within its exact tolerance under
this path, except these, whose measured bounds the manifests record as
`competitive_tolerance` (`tools/fixturegen/competitive.py`) and conformance
checks:

| Fixture | Bound | Cause |
|---|---:|---|
| classic/alpha-unsupported, content/alpha, paint/image-direct-rgb-fractional, paint/text-knockout-false, zero_area/half-alpha | 1 | one byte rounding per store |
| content/alpha-accumulation | 66 | byte rounding accumulated per mark |
| content/precision-translucent-stagnation | 127 | byte rounding accumulated per mark |
| paint/bundled-symbol | 25 | glyph origins snapped to the lattice |
| paint/bundled-symbol-differences | 27 | glyph origins snapped to the lattice |
| paint/bundled-zapfdingbats | 14 | glyph origins snapped to the lattice |
| paint/bundled-zapfdingbats-differences | 19 | glyph origins snapped to the lattice |
| zero_area/zero-width-shading-bbox | 64 | the empty BBox's boundary outline as a clip in cells |

## Amendment: shared opaque composition (2026-10-21)

Status: independent composition correction, source/resource regression tests,
full repository validation and default Exact native/WASI probe passed for the
bound candidate. The [incremental speed/RSS pilot](../../benchmarks/results/opaque-composition-experiment.md)
failed the frozen gates. Retain only as experimental accuracy work; revise the
material costs before a fresh confirmation. No release admission is implied.

The independent direct-root baseline returns 217 where the one-level interval
is 218–219, and 127 where it is 5–6 after 1,000 partial opaque stores. The
per-mark byte route cannot meet the existing composition contract. Reuse the
general compositor instead of adding another precision implementation to the
legacy traversal. This supersedes the earlier decision to leave the opaque
renderer unchanged: Exact has one shared composition route, with the existing
N16/F32/F64 admission and bounded chunks. No fallback returns to opaque byte
accumulation after a precision or resource failure.

Competitive retains N16 for the planner's admitted opaque Normal scenes. It
continues to use its own coverage producers, including glyph fragments, but
the opaque sink shares N16 source encoding and rounded integer blending.
Arbitrary fragment rows handle full, zero and partial coverage individually;
they do not rely on the homogeneous spans produced by the Exact mask iterator.
N16 uses bounded square chunks, not full-width byte bands. F32 plans still use
Competitive's existing U8 policy; F64 exclusions remain unchanged. This fixes
composition error on the declared coverage bytes, not every geometric
approximation or translucent Competitive stack. The four opaque-sliver
Competitive allowances (33–127 levels) are removed; those controls must meet
the ordinary one-level independent oracle tolerance.

Preserve required source errors before culling the scenes formerly evaluated
by the opaque traversal. A bounded source-order pass visits their nested
groups, evaluates painting solid colors in the former RGB conversion context,
and checks path stroke inverses and finite pen-space points before culling.
The segment scan is charged and does not allocate an outline. Non-painting
colors skip conversion; glyph
paint metadata does not create a stroke. Errors keep feature, object and
source context. The pass shares raster and function-work counters with
execution; it deliberately repeats conversion for visible marks until a
bounded prepared-scene record can reuse it. There is no unbounded color cache.
This pass does not claim complete eager validation of every general scene.

Transfers follow the existing general device-transfer semantics and stay F64.
Do not apply them to source colors during validation. Fractional nondefault
transfer regions may now return the documented Unsupported error; unused
off-region transfer functions need not execute. This migration must be tested
explicitly, along with source errors that must remain observable off-region.

The migrated route charges general planning, validation, raster work and
actual transient storage to the existing limits. It may reject budgets at
which the legacy route succeeded. No limits are increased, and no new retry
resets a counter. The existing Competitive-to-Exact Limit fallback remains a
bounded policy fallback into the same compositor; it never reaches the removed
byte route. Final output remains pixel-bounded; the added scratch is not an
invented decoded-byte cap on final output. Error selection among multiple
failures may change when validation precedes geometry. Acceptance requires
explicit tests of these contracts rather than claims of identical ordering or
completion thresholds.

Keep strict rational intervals, source half-levels, actual fragment execution,
mixed rows/clips, source diagnostics, function/raster/storage bounds,
multi-tile assembly and native/WASM parity. Measure ordinary opaque paths/text,
disabled controls and the retained clip/memo targets against the same stack.
Accuracy-changing outputs are evaluated independently; incorrect baseline
output is not equivalent work for a speed claim. Report the accuracy gain and
any time/memory cost separately, retaining useful experimental gains without
claiming that the entire matrix has passed.

## Amendment: narrow output conversion and background admission (2026-10-21)

N16 output channels may be converted in a flat loop with the exact identity
`n = word + 128; (n - (n >> 8)) >> 8`, replacing `(word + 128) / 257`.
For `n = 256q + r`, both produce `q` if `r >= q`, otherwise `q - 1`;
`n <= 65663` bounds the result to a byte. Exhaustively check all u16 values,
channel order, source offsets and incomplete destination pixels. Storage,
compositing arithmetic, work counters and pixel output remain unchanged.

The [first pilot](../../benchmarks/results/n16-output-pilot.md) found an
incremental target gain, but its confirmation was stopped for a separately
discovered accuracy failure: narrow initialization used straight background
channels where premultiplied output was required. Empty colored backgrounds
with alpha zero returned nonzero RGB. Keep that reproducer and strict expected
pixels for alpha 0/128/254/255, partial opaque coverage, translucent paint,
both modes and offset tiles. Nonopaque backgrounds now use F64 before context
creation and Competitive byte selection, compositing onto the existing
premultiplied output. Their 56-byte working pixels cost more than N16's eight;
low resource limits may reject pages the incorrect narrow route accepted.
No limits or counters are reset. Opaque-background admission is unchanged.
A revised candidate needs fresh bindings and samples; the canceled run is
not a confirmation and its records cannot be reused.

## Amendment: admitted F32 composition and main reconciliation (2026-10-21)

The validated [F32 correction](../../benchmarks/results/f32-preservation-cost.md)
supersedes the earlier experimental statement that F32 plans use U8 in
Competitive mode. Both modes retain the planner's F32 precision; Competitive
fragment coverage feeds its F32 sink. The independent translucent-text case
now yields 64 rather than 63. This is a scoped composition correction, not an
end-to-end geometry or general translucent-scene guarantee.

A Competitive compositor Limit retries the same compiled list once with exact
compositing. Reduced image samples or drawing-only font validation may remain;
the retry may fail and preprocessing can add attempts. This is not an Exact
policy recompilation guarantee. Nonopaque backgrounds retain the F64 guard.

Reconciliation with main restores its source-line image accumulation order
from `c19c5374`; the earlier reorder changed historical Exact rounding ties.
This is compatibility/resource reconciliation, not an independent accuracy or
speed claim. Keep source-line footprint evaluation before vertical accumulation,
its bounded scratch/work accounting, strict independent references and both
mode/tile/platform checks. Historical measurements of the reordered stack keep
their original source and binary identities.
