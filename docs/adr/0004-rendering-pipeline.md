# ADR 0004: Rendering pipeline and accuracy contract

Status: accepted, 2026-10-17. Rewritten 2026-10-21 for ADR 0034: this text
replaces the original decision and its amendments of 2026-10-18 to
2026-10-21 (canonical glyph polygons, overprint preview, certified
accumulation, bounded storage, direct glyph curves, direct opaque images and
signed-area glyph coverage). The history of this file keeps them and their
evidence. Section 4's coverage producers were brought in line with the
implementation on 2026-10-22, and the fast parameters and their accuracy gate
were retired the same day (section 8). On 2026-10-24 TrueType quadratics
became their own path segments, integrated without per-placement
certification, and round caps that their end segments hold became part of
the end pieces (section 4).

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
stage: each channel of each output pixel is within one level (of 255) of its
exact result.

- **Coverage:** the exact area of the pixel square covered by the mark and
  its clip. Curves enter as the shared flattener's polygon, except exactly
  quadratic glyph outlines, which enter as their true curve (section 4).
- **Image sampling:** for a minified image, the exact-area average of the
  source samples under the pixel's preimage; for a magnified one, the nearest
  sample or, with `/Interpolate`, bilinear interpolation of associated colour
  and alpha between sample centres (8.9.5.3, ADR 0010). Decoders return
  byte-exact samples, and the exact average is over those samples. A JPEG
  block's DC term is the mean of its unrounded inverse transform, not of its
  rounded, clamped and colour-converted samples (on a large real image they
  differ in about a quarter of block components, by up to 33 levels where
  blocks clamp), so block means from DC terms are an output-changing
  optimization under section 3: admissible only for blocks certified not to
  clamp in any stage, with the bound shown against this oracle.
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

The band rasterizer has one signed-area cell kernel. Prepared marks contain
straight edges and certified monotone glyph quadratics. Edges are listed
per strip of 16 rows; the kernel accumulates them in their own order into
reusable f64 cells, tracks touched cells, and emits partial cells and solid
spans. A rectangle is two vertical edges in that same kernel.

**Certification and row union.** Each contour splits into y-monotone chains.
The certificate proves that neighbouring chains keep their order between
successive chain ends and alternate in direction from one common sign, so
the winding is zero or that sign with magnitude one. For quadratics the
certificate bounds the curves themselves, including their tangent fans at
shared vertices. Doubt leaves the shape uncertified. Stroke pieces that are
proved to meet without overlap are certified by construction. An open
round-capped run is proved where its joints are and each end segment holds
its cap: the segment is longer than the pen radius plus what its other end
needs along it (the other cap's radius, or a turn's inner reach), so the
cap polygon's part behind the end line lies within the end piece. The end
piece then takes the chain of the canonical cap polygon beyond its end
line, entered and left where the polygon's sides cross that line, and the
cap polygon is not drawn. Cap vertices behind the end line must lie across
the piece within eight rounding units of their coordinates, so the piece
differs from the components' union only by slivers of that width. A lone
segment holds both caps as one convex piece where each cap polygon's part
beyond the other end line, which lies within sqrt(r^2 - L^2) of that end,
is inside the other polygon's inscribed circle of radius r cos(pi/n), less
eight rounding units; for segments at least as long as the pen radius that
part is empty. A point's two round caps are the same polygon, drawn once.
Cap admission also bounds crossing and side rounding by 64 rounding units at
the endpoints' construction-space magnitude. This must fit in 1/4096 of the
pen's flattening tolerance, which already accounts for device stretch;
otherwise the run keeps the canonical components and their row union.

Open curve caps retain the source Bézier's endpoint tangents, including the
first nonzero derivative when an endpoint control repeats. Using the first or
last flattened chord rotates the cap and introduces an error proportional to
pen width: the 144-dpi Ink Path circle exposed four seam pixels with errors of
two to five levels. Endpoint tangents travel with only dashes touching those
original open endpoints and transform with the pen. The first and last component
edges use the corresponding normals. Such runs use the component union because
the existing tiling certificate assumes chord-normal caps; closed joins and
interior dash boundaries retain their existing construction. This is a
correctness repair, not a speed claim. The independent disk-area oracle and cap
plane controls retain their existing one-level and exact geometric bounds.

Uncertified rows, and rows crossed by both a mark and a non-convex path clip,
go through one exact row union during preparation. It cuts at edge ends and crossings,
keeps the active edges ordered across each slab, and emits the boundaries of
the area in which every group (mark and clipping paths, 8.5.4) is inside.
Edges are ordered by the exact two-product interpolation. A slab first
orders them by the kernel's plain interpolation and keeps that order where
neighbours lie apart by more than twice its bound, since every comparison
then agrees with the exact one.
These straight boundary pieces have winding zero or one and use the same
cell kernel. Dense rows are partitioned by column with carried windings;
each partition covers the same set and adds no boundary at its cut. The
work and scratch reservations are charged before each allocation or sweep.
Clips are prepared once per clip state into outside, inside and boundary
runs. Rectangular clips clip edges geometrically before accumulation.

**Convex clip paths.** A clipping path that is one contour certified simple
and convex (repeated points ignored; turns of one sign beyond eight rounding
units of their products, and one revolution, as for convex stroke outlines)
is also kept as its left and right chains, each of strictly increasing
height. When every path of a clip state is convex, a polygon mark never meets
their boundary runs. A mark whose hull box, or failing that every vertex,
lies strictly inside every path, by orientation signs certified against
their rounding, keeps its own edges and drops the paths. Any other mark is
clipped to each path in turn as to the rectangle: an edge keeps its part
between the chains, a part beside a chain is replaced by that chain's sides
at the same heights, and a part above or below the path becomes horizontal
and bounds no area. That map fixes the region and moves every other point
along a horizontal segment outside it, or vertically through heights outside
it, so the moved boundary has the mark's winding inside the region and none
outside: windings of zero or one sign of magnitude one stay so, and either
fill rule selects the region's part of the mark. Each piece lies on one
stored line, the edge or a chain side, and its ends are evaluated on that
line at event heights. Crossing heights use the row union's interpolation of
the two lines' differences; the pieces meeting there end on their own lines
and are joined by a horizontal step, so a crossing height off by a rounding
unit moves area only within that unit's slab, however flat the lines meet.
Parts moved along a nearly horizontal side can reach far beside the mark, so
the pieces are then clipped to the mark's hull box, as to the rectangle.
Curves meet such clips only as the polygon every path-clipped mark already
uses, and glyph placements are still inspected against their source curves
under the canonical clips, with the chain sides' interpolation uncertainty
added to the clip's. The rasterized bytes equal the row union's on the
benchmark pages at 72, 150 and 300 dpi, the catalogued pages and the
conformance fixtures, and the exact-oracle clip tests find no differing
pixel; marks along a chain side, through chain points and across nearly
horizontal sides are tested separately.

A subpath whose points all coincide covers the pixel under them
(8.5.3.3.1), and a fill whose region is empty with a boundary paints that
boundary's hairline outline (10.7.4).

**Exactness of accumulation.** Accumulation and resolution are f64 throughout.
For the geometry preparation admits, a cell's integral errs by about 1e-12 of
a pixel against a level step of 1/255, so each coverage byte is the correctly
rounded exact area, except where the exact area lies within that margin of a
rounding boundary, and is always within one level of it. `quantize_unit`'s
tie guard of 1e-9 levels exceeds that error, so exact half-level ties round
up consistently. The order of every operation is fixed (edges in their own
order, prefix sums left to right from column 0, cells indexed by global device
column) and no kernel uses fused multiply-add, so a cell is a function of the
geometry, the cell and the rule alone: band height, region and platform
select which cells are visited, never their arithmetic. Certification and
row union depend only on the geometry and the clip state. An edge whose
coordinates are at most 2^16 in magnitude and whose height is at least
2^-30 interpolates its crossings in plain f64, within the straight-edge
interpolation bound below; other edges use the exact two-product form.
Execution windows of at most 256 columns with at least one item per two
columns keep no touched bitmap: they accumulate densely and emit every
cell, with the same values. A thin stroke across a wider window touches
few of its cells and keeps the bitmap.
A glyph
placement's last window in a band also takes its rows in the next band, as
far as one window reaches, into storage reserved with the rasterizer; the
next band copies those bytes instead of walking the placement again. They
are the same cells, so the bytes do not change, and a band rendered without
the band before walks its placements as usual.

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
contour is omitted. A curved outline whose only point is one explicit
closed move keeps its other contours' quadratics and adds the point's floor
cell to them; a placement that a clip cuts takes the outline's polygon.
Each glyph is a separate nonzero fill composited in
content order, including overlap and knockout (9.3.6, 9.3.8, 11.4.6). Glyphs
of a run share preparation and accumulate into one band row only where no
pixel receives coverage from two of them, never as a union. Strokes and text
clipping follow the path rules.

**Glyph reference.** A quadratic outline is referenced against its original
stored curve at its exact device position (8.5.2.2, 9.3.6), independently of
the renderer's certificate, its clip state and its selected representation.
TrueType quadratics arrive as quadratic path segments and are integrated as
themselves. Every polygon flattens a quadratic as its degree-elevated cubic,
whose controls round once more than the source's; the flattener's hull
allowance includes that rounding. A cubic outline may also be a quadratic
one: in device space the sum of absolute third differences
-P0 + 3P1 - 3P2 + P3 over the entire outline must be at most 2^-24 pixels per
coordinate. This defines its source-curve reference independently of
certification. Analytic reduction of such cubics additionally
includes a conservative 64-rounding-unit allowance for the absolute products
of the original controls and linear map, third-difference evaluation and
midpoint-control construction. Absolute products retain their scale when a
large offset or cancelling map hides it in the third difference; uncertain
cases use the area-bounded original-curve polygon below.

The bound is on area inside each pixel, including any number of near-parallel
pieces. Between an original cubic C and its midpoint quadratic Q, the
homotopy F(s,t) = Q(t) + s(C(t) - Q(t)) remains cubic. At each s each coordinate
has at most three monotone intervals, and its total variation inside a unit
pixel is at most three. Integrating the absolute Jacobian of F bounds the
area swept inside that pixel by 3(delta_x + delta_y), where delta is the
coordinate-wise maximum displacement. For exact midpoint reduction,
C(t) - Q(t) = D t(t - 1/2)(t - 1), whose maximum factor is sqrt(3)/36.
The cumulative third-difference and floating-error allowances bound the sum
of displacements by 2^-24 per coordinate, hence area by 6 * 2^-24 square
pixel (under 0.0001 levels), independent of the number of curves. Clipping
cannot increase this symmetric difference. Certified outlines use
closed-form integration of monotone quadratics with no piece-count cap.

Uncertified quadratic outlines and quadratic placements crossed by a path
clip use de Casteljau subdivision of the original stored curves, each
quadratic as its elevated cubic. Each
accepted chord and its curve lie in their shared control hull. Each curve's
hulls have at most its share of 1/4096 square pixel in area, divided among
the outline's curves; subdivision divides each curve's share between its
two children. Where every placement certifies its bytes (below), the
allowance is divided among at most eight curves, so the sum over the whole
outline can exceed 1/4096 square pixel but not in a pixel that meets at
most eight curves. The
hull determinants include outward error allowances for multiplication,
cancellation, mapping and accumulated subdivision rounding; an uncertain
zero determinant cannot certify zero area. Thus, in a pixel whose curves
hold at most 1/4096 square pixel together, the symmetric difference under
any clip or fill rule is less than 0.063 levels. This gives the fallback the
same true-curve reference without depending on a distance tolerance or on
the number of boundaries in a cell. Segment and subdivision limits still apply; an
unattainable bound returns a limit error.

Neither area allowance alone certifies a coverage byte. A curve and a fine
inscribed polygon can fall on opposite sides of a half-level; repeating
that one-byte difference in separate glyph placements can exceed the scene's
one-level contract. Preparation therefore inspects the common cell kernel's
unquantized areas and quantizes both ends of their source-curve intervals,
including reduction, mapping and translation error. An unambiguous interval
certifies the byte that ordinary execution emits. A placement that
integrates its outline's own quadratic segments in closed form, crossed by
no clip path, has no reduction error, but its mapping, translation and kernel
arithmetic can still be ill-conditioned. A cached whole-outline allowance
includes the absolute mapping products and a quadratic coefficient-scale
bound on root residuals, integrals and cell accumulation. At each placement,
translation is included too. Only an allowance below 1/1024 of one coverage
level skips inspection; larger or non-finite allowances use the measured
certificate. The admission work is cached with the linear glyph map, so
ordinary repeated placements perform only a constant-size check. Polygon
placements and reduced cubics are always inspected. A polygon pixel that the
whole-outline allowance leaves ambiguous is checked again against the shares
of only the curves whose boxes meet it: subdivision keeps a curve's controls
within its control box, so the box's mapped corners, widened by the mapping
and hull rounding of the deepest subdivision, contain every hull that spends
its share. Outlines with cubics keep the whole allowance, since their reduced
quadratics may leave their control boxes. Ambiguous pixels are
resolved from the original stored curves in a pixel-local frame using the
shared bounded subdivision and row-union arithmetic. Accepted control hulls
bound the area uncertainty under the canonical clips. Integer localization
retains its two-sum tails; full outward hulls must miss outward clip bounds
before their error is omitted.

When the proven byte differs from the prepared representation, only curve
pieces whose full hulls meet those pixels are subdivided more finely. The
replacement remains an ordinary polygon with resolved rows. Every cell of
that replacement is inspected again, so refining one boundary cannot silently
change a neighboring byte. Inspection, local proofs and all replacement
resolutions share one bounded work meter; scratch and overlapping old/new
geometry are charged before allocation. Unresolved precision or exhausted
work, segment, subdivision or memory limits refuse preparation. Execution
has no coverage overrides, allocation or fallible refinement. A synthetic
coincident-outline composition checks coverage byte one directly and its
three-placement composed result against the analytic byte reference.

The interval includes canonical straight-edge arithmetic as well as curve
approximation. An interior interpolation parameter lies in [0,1]; rounding
its two differences, division, product and addition is bounded by sixteen
rounding units times the endpoint magnitudes in each coordinate. A linear
boundary's swept uncertainty inside a unit pixel is bounded by those radii
times its coordinate variations there. Axis-aligned boundaries require no
such interpolation; localization tails are still retained. Large opposite
endpoints can otherwise hide a low bit in the denominator and move an area
from below a half-level onto it. Independent analytic source-line and
clip-line regressions require that case to remain inside the interval and
refuse unresolved precision.

Preparation measures signed accumulation in the common kernel for both
initial ambiguity selection and local proof. Error-free TwoSum retains the
actual roundoff of each area/cover addition without changing the stored
result or operation order. Let A be the outward sum of absolute inputs and
E the outward sum of primitive errors and addition tails. Stored cell
magnitudes total at most A+E. Only emission's prefix remains unmeasured: at
most window width plus four operations per pixel, bounded by gamma_n(A+E).
The positive A/E sums include their own count-dependent rounding allowance.
Thus thousands of exactly added fragments do not themselves impose a
precision floor, while lost low terms under cancellation remain bounded.
This includes the primitives before they cancel: H's coefficient products
and endpoint evaluations, coordinate evaluations, area products and the
multi-column kernel's internal prefix of dy values. Their absolute operand
bounds remain charged even for columns left of the requested window.
Measuring only the final area additions would miss cancellation between
large antiderivative values.

Computed quadratic roots need no inverse-derivative assumption. Moving a row
cut to a computed parameter changes coverage only in the horizontal strip
between the target height and the exact stored polynomial at that parameter;
a column transition similarly changes coverage only in a vertical residual
strip. Its area inside a unit cell is bounded by the residual times one plus
the opposite-coordinate residual. Coordinate-evaluation error, both ends,
endpoint floor/ceil decisions and clamped column roots are included. A
quadratic has at most two monotone intervals per coordinate, so an eightfold
sum including residual cross products conservatively covers their overlaps.
These bounds remain valid at a zero derivative and for a carried row root.
An independent long-quadratic half-level regression checks the primitive
interval and requires the original-curve byte or a bounded precision refusal.
The execution specialization omits that measurement. Canonical clip proof
bounds and their broad arithmetic allowance are computed once with charged
work. Strip input scans are reserved before traversal, mask filtering uses
one monotone row cursor, and replacement-builder scratch is returned before
the next inspection starts.

Clip classification uses those unquantized cells too. Touched cells keep
their boundary support even when they round to zero; an untouched zero run
is omitted only if its full arithmetic interval still quantizes to zero.
Only an untouched run with raw area one, whose resolved boundaries span the
whole pixel row, is inside. A fractional-height run that merely rounds to
255 remains a boundary and participates in joint mark/clip resolution.
These are ordinary clip segments, charged by the existing row meters. They
prevent classification from hiding an ambiguous pixel before glyph
certification, and preserve the same geometry in sub-region requests.

A general cubic's displacement tolerance alone does not bound area when
several edges share a pixel, so its outline retains the canonical 1/256-pixel
polygon as its reference. Singular linear maps retain the polygon's
geometric-point rule. The independent test oracle keeps the stored quadratic
and cubic segments and integrates their covered lengths by adaptive Gauss-Legendre
quadrature. The slow differential reference bounds its own original-curve
polygon's total control-hull area by 1/65536 square pixel; it never adopts
the renderer's certificate or a path-clip exception. That initial area
allowance alone cannot decide a coverage byte near a half-level. The
reference therefore retains each accepted hull's outward bounds and area
(including placed-coordinate mapping and translation rounding),
sums the errors of hulls that touch each pixel, and quantizes both ends of
the resulting area interval. When those bytes differ it refines the
original controls with one sixteenth of the area allowance and repeats.
Clipping cannot increase the interval's error. An unresolved interval,
segment limit or subdivision limit is an error, never permission to select
the byte that agrees with the renderer. A synthetic overlapping-quadratic
case checks this distinction against an analytic area.

Ambiguous pixels are evaluated in a local frame: subtract the integer pixel
from the stored glyph origin before mapping the original controls. An
error-free two-sum subtraction retains its tail as coordinate uncertainty;
canonical clip endpoints retain their own subtraction tails. Thus a large
absolute position cannot impose an artificial precision floor. A curve hull
is omitted only when its outward rectangle misses the intersection of the
canonical clips' outward bounds, including those endpoint tails. The full
hull, rather than its chord, must miss that intersection. Translated
half-level and clipped half-level cases exercise both decisions.

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

### 8. Fast parameters

Retired on 2026-10-22 by owner decision, with their accuracy gate: exact
rendering is the only rendering.

## Consequences

- One pipeline replaces the six execution routes, the page-wide storage plan,
  the 8-bit opaque route and the exact re-render of the former fast mode
  (ADR 0034). The
  8-bit route's known defect, which quantized source colours and rounded
  twice per partial store (up to about 1.5 levels each, accumulating), goes
  with it: opaque pages store in N16 band buffers within the bound of
  section 6.
- Formats are chosen per buffer, so a soft mask, a pattern or a blend inside
  a group no longer puts a whole page on F64, and translucent groups and
  soft masks can run in F32 under their parents' bounds.
- The retired producers (ADR 0034) took their admission rules and bounds
  with them; a producer that returns re-establishes its bound under section
  3.
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
