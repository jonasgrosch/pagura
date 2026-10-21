# ADR 0004: Rendering pipeline and accuracy contract

Status: accepted, 2026-10-17. Rewritten 2026-10-21 for ADR 0034: this text
replaces the original decision and its amendments of 2026-10-18 to
2026-10-21 (canonical glyph polygons, overprint preview, certified
accumulation, bounded storage, direct glyph curves, direct opaque images, the
competitive gate, signed-area glyph coverage and the competitive path). The
history of this file keeps them and their evidence.

## Decision

### 1. Pipeline

ADR 0034 sets the architecture: one interpreter records a scene per page, and
one renderer prepares it for a request and executes it in bands. This record
fixes what the stages compute and how exact the result is.

- **Interpretation** reads content streams as one token sequence across all
  `/Contents` streams (7.8.2), carrying operand and graphics state across
  stream boundaries. Every operator of Annex A is recognized; an
  unimplemented one fails with `Unsupported(operator.<name>)`, an unknown one
  is a syntax error except inside `BX`/`EX`. The scene is in PDF terms: paths
  in user space with their transform, fill rule, stroke parameters and clip
  state, and colours in their source colour space, converted at paint time
  by prepared converters (ADR 0028).
- **Rasterization** (`pagura-raster`) knows no PDF. Paths are flattened by
  the shared flattener, which stays within 1/256 device pixel of the curve,
  and filled by exact-area coverage under the nonzero and even-odd rules.
  Strokes are outlined in user space and then transformed, so a non-uniform
  transform distorts the pen as PDF requires; a zero-width line is one
  device pixel wide. Clips stay geometry: a clipped mark covers the exact
  area of mark and clip together, computed jointly only where both are
  partial in a pixel. `sh` paints its clip region, within the shading's
  `BBox` if it has one.
- **Composition** follows clause 11 on premultiplied values, onto the
  requested background premultiplied once. Each output pixel is quantized
  once with `pagura_raster::quantize_unit` (ADR 0022).
- **Regions.** A request names a rectangle of the page's device pixels.
  Bands lie on the global device grid, so the regions of a page assemble into
  the whole page byte for byte, and no result depends on band height, region
  partition or platform: native, WASI with SIMD128 and scalar WASI give the
  same bytes. Exact kernels store no colour in 8 bits before the output and
  use no fused multiply-add and no relaxed SIMD.

### 2. The accuracy contract

Every rendered pixel has an exact answer, and one contract covers every
stage: with every fast parameter off (section 9), each channel of each output
pixel is within one level (of 255) of its exact result.

- **Coverage:** the exact area of the pixel square covered by the mark and
  its clip. Curves enter as the shared flattener's polygon, except exactly
  quadratic glyph outlines, which enter as their true curve (section 4).
- **Image sampling:** for a minified image, the exact-area average of the
  source samples under the pixel's preimage; for a magnified one, the nearest
  sample or, with `/Interpolate`, bilinear interpolation of associated colour
  and alpha between sample centres (8.9.5.3, ADR 0010). Decoders return
  byte-exact samples, and a decode request for exact means (JPEG DC terms of
  blocks that do not clamp) returns what full decoding and averaging would.
- **Composition through non-amplifying operations:** the imaging model of
  clause 11 in exact arithmetic on the same coverage bytes and sources, for
  any number of overlapping marks. These operations are Normal compositing,
  alpha, shape and soft-mask application, and the separable blend modes
  Multiply, Screen, Darken, Lighten, Difference and Exclusion. Their results
  never change faster than their inputs, so rounding errors only add up.
- **Composition through amplifying operations:** ColorDodge, ColorBurn,
  Overlay, HardLight, SoftLight, the non-separable blend modes (11.3.5),
  steep transfer functions (10.5) and knockout's division by shape can change
  faster than their inputs, so no finite precision bounds them against exact
  arithmetic for every input. Every buffer that feeds one is evaluated in
  f64 from its first store, and the contract is one level of that documented
  f64 evaluation. Feeding is transitive: a group or soft mask whose result
  enters such a buffer feeds the operation too. The renderer's
  classification of each operation, with its amplification bound, decides
  the side (`BlendMode::amplifies` for blend modes).
- **Sources** enter composition as f64 values from prepared converters,
  functions and samplers. The storage bounds of sections 5 and 6 include
  encoding them and reserve a margin for their f64 evaluation.

Pixel-aligned opaque cases stay exact. Exact rounding ties are not part of
the contract, and exact rounding of arbitrary translucent stacks is not
promised.

**Oracles.** Fixture oracles compute the exact result independently, in
rational arithmetic, or by high-precision quadrature for true curves; a
fixture may carry a tolerance of 1 against its oracle. `pagura-reference`
(test only, ADR 0034) is the slow, obviously correct f64 raster and
compositor for differential tests. Earlier renderer output is a differential
reference, never an oracle.

### 3. Output-changing optimizations

A faster computation that changes output qualifies under the contract only
with:

1. an independent oracle for every affected case, never the code under test;
2. every changed pixel against the previous output reported with its oracle
   value;
3. the one-level bound holding on every conformance fixture, on every
   benchmark page at 72, 150 and 300 dpi and on the stress set;
4. the exact-rounding match rate, the share of changed pixels equal to the
   correctly rounded exact value, reported before and after.

Where new and previous output differ by more than one level, both are
compared with the oracle; the change is accepted only if the new result meets
the contract and is closer to the oracle at every such pixel. A result beyond
one level of its oracle is a correctness change and needs its own decision. A
mechanism admitted under this rule records its bound here.

### 4. Coverage and glyphs

The exact coverage producers are the band rasterizer's certified integer
accumulation, its exact local resolver for cells that fail certification, the
closed form for rectangles and the analytic integration of glyph quadratics.
For the same flattened geometry they yield the same coverage bytes (ADR
0034). A producer that covers a stroke by pieces meeting without overlapping
(section 9) replaces the union's resolution on the exact path only after
section 3 establishes it.

**Certified accumulation** snaps flattened device geometry to a
page-anchored grid of spacing u = 2^-20 pixels, with ties to even and
consistently rounded shared crossings. A cell's count n conservatively counts
the original flattened segments meeting it. A segment whose snapped image has
zero length, or turns horizontal while its original did not, stays as a
zero-cover, zero-area ghost that counts toward n and leaves its cells
uncertified; a snapped contour that degenerates leaves every cell of its
original bounding box uncertified. Cells are certified by one-chain,
closed-cell boundary-walk, constant-fill or group-intersection certificates;
cells with ghosts or n > 512 resolve the unsnapped geometry. For n <= 512,
snapping and crossing err by at most (2.5n + 7)u, so with byte quantization a
cell stays below 0.813 levels from the exact area. Subpaths degenerate before
snapping keep the floor-cell policy. Certification depends only on the
geometry, never on regions, bands or omitted redundant clips.

**Glyph placement** is exact (ADR 0013). Filled outline text (render modes 0
and 4) keeps its font outline, its linear glyph-to-device map L and its exact
device origin t. The relative polygon is built by the shared flattener at the
1/256-pixel tolerance, measured through L; its vertices are mapped by L and
then translated by t. Translation never takes part in subdivision and nothing
is snapped to a grid, so a glyph's edge setup depends only on the outline and
L and serves every placement (the `Store`, ADR 0012). Closure and point
classification come from the L-mapped endpoints and controls before
approximation and translation: coincident approximating vertices do not turn
a curve into a point, while a singular L can produce a genuine point, which
encloses its translated floor cell (8.5.3.3.1). A final open one-point
contour is omitted. Each glyph is a separate nonzero fill composited in
content order, including overlap and knockout (9.3.6, 9.3.8, 11.4.6). Glyphs
of a run share preparation and accumulate into one band row only where no
pixel receives coverage from two of them, never as a union. Strokes and text
clipping follow the path rules.

**Glyph reference.** An exactly quadratic outline is referenced against its
true curve at its exact device position (8.5.2.2, 9.3.6): TrueType
quadratics, and cubics whose third difference -P0 + 3P1 - 3P2 + P3 is at
most 2^-24 device pixels per coordinate, as degree elevation leaves it after
rounding. Their midpoint quadratics move the curve by less than 2^-27
pixels, so by less than 2^-26 of a pixel's area per monotone piece crossing
it, and the rasterizer integrates monotone quadratic pieces analytically in
f64. A general cubic's midpoint quadratic is no area certificate:
displacements below 1/1024 pixel on several edges sharing a pixel exceed one
level. Cubic outlines therefore keep their placed canonical polygon, with its
point rule, as the reference. The flattening tolerance alone does not bound
area against the true curve, so independent high-precision quadrature of the
outline checks curve results, and for those glyphs the polygon is a
differential reference only. Which reference a glyph has depends only on its
outline, L and its clip state, never on regions, bands or cache state.

### 5. Storage formats

Each buffer (the page, a group, a soft mask) gets its format before its first
store, from its group's whole content and never from the requested region
(11.3.3, 11.4.4, 11.4.6). Geometry, coverage, colour conversion and image
reconstruction keep their own precision. Buffers are band-local, so a wider
format costs bandwidth, not page-sized memory.

- **F64** for every buffer that feeds an amplifying operation; every buffer
  that receives a store with a blend mode other than Normal or with a
  transfer function, and every soft mask with a transfer function; buffers
  with spot channels and every buffer under overprint simulation; buffers
  whose blending space is not RGB; knockout groups, except those whose
  members' opacity equals their shape (opaque members), which composite as
  source-over; non-isolated groups whose content is not all Normal, and the
  buffers that provide their backdrop; tiling-pattern cells; every child of
  an F64 buffer; and every buffer that no narrow bound admits.
- **N16** for the page buffer when its bound in section 6 holds, otherwise
  **F32** for a page, group or soft-mask buffer when its bound holds.
- **U8** only under the `byte_storage` fast parameter.

A tiling pattern's cell colours and alpha enter the marks painted with it as
a per-pixel source, whose f64 evaluation error includes the cell buffer's
bound (section 6).

N16 stores four u16 words at 257 words per level, 8 bytes per pixel. An
opaque source level L = 255 clamp(S) is encoded by rounding 257 L, taking the
adjacent word where needed to keep round(L) at the final byte boundary; the
encoding errs by at most σ16 = 1 + 2^-36 words. A store of coverage byte c
writes floor((c s + (255 - c) u + 127) / 255) for source and backdrop words s
and u: one integer rounding of at most 127/255 words. The initial value
(background levels b as 257 b, transparent as zero) is exact for an opaque
background and within half a word otherwise. The output is the nearest byte
of u/257, which is never a tie, so it adds at most 128/257 levels.

F32 stores four f32 channels of levels on 0..255, 16 bytes per pixel.
Inexact source levels are encoded with round-to-odd, which keeps their side
of a following nearest-byte boundary; exact levels stay exact. Full-opacity
stores overwrite; other stores evaluate x += a (t - x) with separate
roundings. Let ρ = 2^-17 levels, one rounding below 256, and
π = 2^-16 + 255 · 6 · 2^-24 levels, the source encoding and six roundings
relative to the opacity.

The source encodings and full-opacity overwrites keep nearest output at half
levels, which final rounding alone would not. Store counts treat each glyph
instance as a store, not each glyph run, and N bounds the stores at any pixel
of the buffer; a count over the buffer's whole content is such a bound.

### 6. Bounds of narrow storage

**Store lemma.** Take one channel of one pixel of a buffer, in levels. A store
of opacity a in (0, 1] and target t changes the reference value y to
(1 - a) y + a t and the stored value x to (1 - a) x + a t + ζ; a store of
opacity zero does nothing. The store is *contracting* when a ≥ θ and
|ζ| ≤ a σ + ρ, and *dynamic* otherwise, with |ζ| ≤ a σ + ρ_k + η_k for its own
rounding ρ_k and an injected error η_k. If x starts within σ of y, then after
N stores, K of them dynamic,

    |x - y| ≤ σ + min(Σ ρ_k, ρ/θ + Σ_dyn ρ_k) + Σ_dyn η_k.

*Proof.* The error e = x - y becomes (1 - a) e + ζ. Each side of the minimum,
with Σ_dyn η_k added, is an invariant, and both hold at the start. For the
first, (1 - a)(σ + R) + a σ ≤ σ + R, so a store adds at most ρ_k + η_k. For
the second, if |e| ≤ σ + ρ/θ + D, a contracting store leaves
(1 - a)(σ + ρ/θ + D) + a σ + ρ ≤ σ + ρ/θ + D, because a ρ/θ ≥ ρ, and a
dynamic store adds at most ρ_k + η_k to D. ∎

**N16.** Contracting stores take opaque sources (σ = σ16 for a solid colour,
plus the f64 evaluation error of a source evaluated per pixel), opacity c/255
for a coverage byte c ≥ 1 (θ = 1/255) and ρ = 127/255 words, so ρ/θ = 127
words. With the output's 128/257 levels, a pixel stays within one level while

    σ + 127 + Σ_dyn (ρ_k + η_k) ≤ 129 words.

Opaque solid colours alone give at most (256 + 2^-36)/257 < 0.99612 levels,
independently of overlap count, and leave 1 - 2^-36 words for dynamic stores.
A dynamic N16 store is evaluated in f64 and rounded once to the nearest word,
ρ_k = 1/2 word, with its f64 error counted in η_k. So an N16 buffer admits at
most one dynamic store at any pixel, with η_k ≤ 1/2 - 2^-36 words.

**F32.** Every store has σ = π and ρ_k = ρ, and θ conservatively lower-bounds
the positive opacity of the contracting stores. The buffer is admitted when

    π + ρ min(N, 1/θ + K) + Σ_dyn η_k ≤ 1/2 - 2^-12 levels;

the 2^-12 margin covers f64 source evaluation, and the final nearest byte
adds at most half a level. A stack of 270,000 stores of opacity 3/2^27
therefore stays F64.

**Children.** A child buffer B, a group or a soft mask, reaches its parent as
one dynamic store at each pixel it touches, evaluated in f64 from B's stored
values and rounded once to the parent's format. A narrow child is an isolated
group, a non-isolated group of Normal content (source-over is associative, so
it composites as an isolated one) or a soft mask, in an RGB blending space,
without knockout other than the opaque-member identity, composited with
Normal into a narrow parent. Every other child is F64, since no inequality
bounds what narrow children inject into an F64 parent. Let B's colour and
alpha channels be within E_c(B) and E_α(B) levels of their reference, and m be
the composite's opacity factor (constant alpha, coverage and soft-mask
value). The composite x' = (1 - m α/255) x + m c then injects

    η ≤ m (E_c(B) + (256/255) E_α(B)) + 2^-40 levels,

because stored values stay within 256 levels and the composite's f64
evaluation errs by at most 2^-40 levels. A store whose opacity includes a
soft-mask value within E(M) levels of its reference injects (256/255) E(M)
more. A luminosity mask's value is within the largest bound of its colour
channels, since the luminosity weights sum to one, and an alpha mask's within
its alpha bound, each plus the f64 evaluation of its luminosity and transfer
function.

A child's own bound E(B) is:

- zero for an F64 buffer that feeds an amplifying operation, whose
  documented f64 evaluation is the reference;
- for any other F64 buffer, N_B · 2^-40 levels for its N_B stores at a
  pixel, plus its sources' f64 evaluation error and its children's
  injections (the lemma's first form with σ = 0): every f64 kernel of a
  non-amplifying operation departs from its exact evaluation by at most
  2^-40 levels per store (a few roundings on magnitudes below 2^9 levels)
  and passes earlier errors on with a factor of at most one, which its tests
  check against rational arithmetic;
- for an F32 buffer, the left side of its admission inequality, its own
  children included, plus its sources' f64 evaluation error, which is at
  most the 2^-12 margin.

Admission therefore computes bounds from the innermost buffer outwards,
counting each child's composite as a dynamic store of its parent; a buffer
whose parent ends up F64 is then F64 too, which only lowers its bound.

N16 children are never admitted. An N16 buffer's own bound is about half a
level per channel, so its composite can inject m (0.498 + 0.496) levels, more
than an N16 parent's room for any useful m and more than an F32 parent's once
m exceeds about one half; such children are not worth a rule. Group and
soft-mask buffers are therefore F32 or F64, and only the page buffer may be
N16.

**Not proven.** These stay F64 until a bound is recorded here: a second
dynamic store at an N16 pixel, because two nearest-word roundings exceed its
room; N16 children; narrow children of F64 parents; blend modes other than
Normal and transfer functions in a buffer, whose results depend on the
backdrop in a way the lemma does not model; non-isolated groups whose content
is not all Normal, because removing the backdrop divides by the group's
alpha; knockout groups other than the opaque-member identity; non-RGB
blending spaces, spot channels and overprint simulation, whose conversions
and spot accumulation the lemma does not cover; and promotion of byte-stored
chunks to N16 (ADR 0034), which needs a bound for a start already rounded to
bytes and work aligned to a global chunk grid.

A change to admission or kernel arithmetic keeps independent rational
fixtures, half-level source tests, glyph-instance counts, tests of every F64
case above, nested-buffer fixtures at the edges of these inequalities,
actual-byte budget tests, region and band-height assembly, changed-channel
exact oracles, native and WebAssembly parity, and paired timing and memory
measurements.

### 7. Overprint preview

`OverprintPreview` controls simulation (8.6.7, 11.7.4). Auto, the default,
simulates only when a PDF/X print intent is declared: an OutputIntents
dictionary with `S /GTS_PDFX` in the catalog or a page, including indirect
entries, or a `GTS_PDFXVersion` string in Info. This detects declarations,
not conformance; malformed advisory metadata counts as absent. Always
simulates wherever overprinting affects the result. Never ignores overprint
and paints ordinary composite content, also within groups and soft masks.

Simulation uses a CMYK default process device and keeps explicit group
blending spaces. Spot colourants are extra channels of the buffers, preserved
through groups; soft masks use the alternates. At output each name's first
painted alternate definition is used, and CMYK ink equivalents are added
with saturation before RGB conversion. At most 32 spot plates are supported,
with `Limit` above that. This is a display approximation: physical ink
interaction and destination ICC output transforms are not modelled. An
ICC-based transparency group composites in its N-component device base
without inverse ICC transforms; its sources keep their forward transforms.

### 8. The competitive gate

The one-level contract stays the reference and the default. An approximate
configuration, one that can leave a pixel more than one level from its exact
result, may become the default only when it meets the absolute goals below on
the 40-page benchmark set at 72, 150 and 300 dpi. The goals are fixed
numbers. They were derived on 2026-10-19 from the best competitor in each
cell, not from any single engine, measured on main 99bfb290 against the
then-latest releases (hayro 0.8.0, PDFium 156.0.8076 via pypdfium2 5.14.0,
MuPDF 1.28.5 via PyMuPDF 2.0, pdf.js 6.4.299, Poppler, Quartz on macOS 15.8),
and do not move when competitors change.

**Accuracy goals**, measured against Pagura's exact rendering of the same
page, pixel-weighted per category and DPI: MAE, the mean absolute RGB error;
within ±1, the share of pixels whose largest channel error is at most one;
P99.9, the 99.9th percentile of the largest channel error. Each value is the
competitors' best figure for that measure, improved by at least 10% and
rounded towards the more ambitious side. No page may be less accurate than
the best competitor on that page. The maximum error is not gated, because
every engine reaches 231–255 on some pages.

| Category | DPI | MAE ≤ | Within ±1 ≥ | P99.9 ≤ |
|---|---:|---:|---:|---:|
| Text | 72 / 150 / 300 | 1.0 / 0.5 / 0.3 | 93 / 96 / 98% | 60 / 40 / 30 |
| Vector | 72 / 150 / 300 | 0.6 / 0.15 / 0.05 | 95 / 99.2 / 99.6% | 50 / 30 / 20 |
| Mixed | 72 / 150 / 300 | 1.5 / 0.75 / 0.25 | 92 / 94 / 98% | 110 / 90 / 40 |
| Images | 72 / 150 / 300 | 3.0 / 0.75 / 0.4 | 85 / 94 / 98.5% | 80 / 60 / 90 |

The best competitor differs by cell among PDFium, MuPDF, hayro, pdf.js,
Poppler and Quartz: hayro for most text and vector cells, MuPDF for vector at
72 dpi, and pdf.js for images at 72 and 150 dpi, mixed at 150 dpi and some
percentiles.

**Speed goals.** For each category and DPI, the geometric mean over the
category's documents of Pagura's engine time (open plus every page render)
must be at most the value below, in milliseconds, on each benchmark runner.
Each value is 0.9 times the fastest competitor on that runner, rounded down in
0.5 ms steps below 10 ms, 1 ms steps up to 30 ms and 5 ms steps above. On M4
the fastest competitor is hayro in every cell. On x86 it is PDFium for text
at 72 and 150 dpi and for all vector, MuPDF for images at 72 dpi, and hayro
otherwise. On Graviton it is PDFium for text, vector and mixed, MuPDF for
images at 72 dpi, and hayro for images at 150 and 300 dpi.

| Category | M4 (ms ≤) | x86 (ms ≤) | Graviton (ms ≤) |
|---|---|---|---|
| Text | 5.5 / 8 / 7.5 | 11 / 20 / 26 | 9.5 / 15 / 19 |
| Vector | 3 / 4.5 / 9 | 6 / 11 / 29 | 5.5 / 8.5 / 19 |
| Mixed | 4 / 6.5 / 7 | 9.5 / 17 / 23 | 8.5 / 14 / 17 |
| Images | 13 / 19 / 16 | 25 / 45 / 50 | 27 / 40 / 40 |

The values are for 72 / 150 / 300 dpi, from the 2026-10-19 runs on main
99bfb290.

**Ratchet.** Both margins tighten over time from 10% towards the original
20%. Each tightening adds a new dated table to this section, and a
configuration that falls behind a tightened table stops being a default.

**Unchanged by any configuration.** Rendering stays deterministic and
byte-identical across platforms, regions and band heights; work and storage
stay reserved against `Limits` (ADR 0016); no `unsafe` code or dependency
exception is introduced (ADR 0008); conformance fixtures keep their
independent oracles. Every approximation is a named fast parameter (section
9), and its per-category accuracy table is kept here with the change that
introduces it.

### 9. Fast parameters

`RenderOptions::fast` is a `FastParams` of four switches, each a kernel or
producer variant inside the one pipeline (ADR 0034). `FastParams::EXACT`, all
off, is the default and is held to the contract above;
`FastParams::COMPETITIVE`, all on, is the configuration the gate measures.
Each parameter can be switched alone and is tested alone and in combination;
none routes a page to other code, and fonts load the same way under every
set. Preparation reserves what the selected kernels use, so a parameter can
change whether a render reaches a limit; a failed render is not retried
exactly.

- **`glyph_origin_bins`:** glyph coverage is reused per origin bin. Bins lie
  on the global device grid (ADR 0013): 1/128 pixel horizontally and,
  vertically, 1/128 pixel below 108 dpi, 1/16 pixel from 108 dpi and 1/64
  pixel from 216 dpi. Curves become 0.15-pixel chords below 108 dpi and two
  area-balanced chords per 1/4-pixel piece from 108 dpi. Coverage is a pure
  function of outline, linear map and bin, so reuse never depends on
  regions, bands or cache state.
- **`approximate_coverage`:** fills and strokes accumulate signed areas in
  f32 cells after flattening curves to 1/64-pixel chords, instead of
  certified accumulation. Cells are indexed in global device columns, so any
  partition assembles into the page. Rectangles, degenerate contours and
  compound paths, other than a stroke's pieces, keep the exact producers. A
  stroke becomes pieces that meet without overlapping: each segment's
  quadrilateral ends at the inner miter point on the inner side of a turn and
  at its normal on the outer side, the outer wedge takes the join, and a
  round cap is the half disc beyond its endpoint. A stroke keeps overlapping
  components when a run reverses, turns tighter than the pen around a short
  segment, has an open centreline whose directions span a half turn or a
  closed one that is not convex, or when the bounding boxes of its runs' pens
  meet. A clip whose path contains a mark's control hull is dropped for that
  mark, and a quadrilateral mark within quadrilateral clips is integrated
  with them geometrically. Otherwise the clip's coverage, a column factor
  times a row factor for a rectangle and its own cells once per band for any
  other region, is multiplied into the mark, which approximates the
  intersection where both boundaries cross one pixel.
- **`byte_storage`:** a buffer that exact rendering stores in N16 or F32 is
  U8, rounding each store once; the error accumulates per mark.
- **`reduced_image_decode`:** an opaque DeviceGray or DeviceRGB DCTDecode
  image without `Mask`, `SMask` or `Decode` is reconstructed near its drawn
  size, with 1, 2, 3, 4 or 6 cells per eight-sample block axis, the fewest
  that leave two and a half cells per device pixel along its less minified
  axis. A cell is the mean of its block's unclamped T.81 A.3.3 samples from an
  integrated cosine basis, rounded once; with six cells, a block that may
  leave the sample range takes the means of its exact samples. The samplers
  take the cells, the image's unit square spanning its declared extent in
  them. JPEG 2000 images decode at a reduced resolution level near their
  drawn size; the selection rule is recorded here with its accuracy table.

The route stack these parameters replace met every accuracy goal on main
752b52e7 (table in this file's history). The combination's own table is
recorded here before it can become a default.

**Fixtures.** Every render fixture stays within its exact tolerance under
each parameter alone and under the combination, except these under the
parameter named. The manifests record the bounds as `competitive_tolerance`
(`tools/fixturegen/competitive.py`), which the conformance check applies under
`FastParams::COMPETITIVE`; each parameter alone stays within the same bound.

| Fixture | Bound | Parameter: cause |
|---|---:|---|
| classic/alpha-unsupported, content/alpha, paint/image-direct-rgb-fractional, paint/text-knockout-false, zero_area/half-alpha | 1 | `byte_storage`: one byte rounding per store |
| content/alpha-accumulation | 66 | `byte_storage`: byte rounding accumulated per mark |
| content/precision-slivers-254, content/precision-slivers-255 | 33 | `byte_storage`: byte rounding accumulated per mark |
| content/precision-slivers-1000 | 122 | `byte_storage`: byte rounding accumulated per mark |
| content/precision-slivers-270000, content/precision-translucent-stagnation | 127 | `byte_storage`: byte rounding accumulated per mark |
| paint/bundled-symbol | 25 | `glyph_origin_bins`: origins binned |
| paint/bundled-symbol-differences | 27 | `glyph_origin_bins`: origins binned |
| paint/bundled-zapfdingbats | 14 | `glyph_origin_bins`: origins binned |
| paint/bundled-zapfdingbats-differences | 19 | `glyph_origin_bins`: origins binned |
| zero_area/zero-width-shading-bbox | 64 | `approximate_coverage`: the empty `BBox`'s boundary outline as a clip in cells |

A bound changes only with the evidence of section 3 recorded here.

## Consequences

- One pipeline replaces the six execution routes, the page-wide storage plan,
  the 8-bit opaque route and the Competitive exact re-render (ADR 0034). The
  8-bit route's known defect, which quantized source colours and rounded
  twice per partial store (up to about 1.5 levels each, accumulating), goes
  with it: opaque pages store in N16 band buffers within the bound of
  section 6.
- Formats are chosen per buffer, so a soft mask, a pattern or a blend inside
  a group no longer puts a whole page on F64, and translucent groups and
  soft masks can run in F32 under their parents' bounds.
- Producers that ADR 0034 retires (the strip sweep, signed-area glyph cells,
  the fragment rasterizer, the direct image routes) take their admission
  rules and bounds with them. A producer that returns re-establishes its
  bound under section 3.
- GPU backends stay out of scope until the CPU reference is complete.

## Alternatives considered

- **All-F64 composition:** simplest, but forgoes the measured allocation and
  bandwidth savings of narrow formats.
- **Formats per page:** what main did; one mask in a corner put every pixel
  on 56-byte f64 values (ADR 0034).
- **N16 child buffers:** an N16 buffer leaves no room for a child that
  already carries half a level (section 6).
- **Byte storage promoted to N16 per 16×16 chunk:** deferred to the
  performance phase as an experiment with the proof obligations of section
  6.
