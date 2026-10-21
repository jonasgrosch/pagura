# Sub-byte coverage: independent contract review

2026-10-21. Review of the frozen renderer at `f98ad836` plus the u16 converter
candidate, followed by a separately bounded twelve-output diagnostic. The
analytical references below were authored before execution. Both modes reproduced
every prediction; the raw outputs and exact arithmetic were independently audited.
This diagnostic is separate from the completed u16 performance admission.

## Judgment

The proposed repeated-subpixel example exposes a real limit of the current
accuracy claim; it does not violate the explicit coverage-byte composition
contract. [ADR 0004](../adr/0004-rendering-pipeline.md) separates accurate per-mark area rasterization
from exact composition on **the same coverage bytes**. Each width-1/1024 mark
may correctly round to zero; N16 then correctly does nothing. The public `Accuracy::Exact` summary requires this operational definition.
ADR 0004 now states the reference scope explicitly; its gates are unchanged.

An end-to-end one-level bound against unquantized per-mark coverages does not
follow from one-level raster accuracy plus one-level composition accuracy.
Nor is unquantized per-mark source-over the same oracle as filtering the final
continuous scene: coverage correlation is lost even with infinite precision.
[ADR 0016](../adr/0016-unified-renderer.md) already treats continuous scene filtering as a contract change.

## Three distinct references

Use white background, black opaque Normal fills, no groups, masks, transfers,
curves, images or color conversion, at 72 dpi. Let q=1/1024. A one-pixel page
has MediaBox [0 0 1 1]. The width is the exact terminating decimal
0.0009765625, which is exactly representable in binary64.

| Case | Current declared coverage-byte reference | Unquantized per-mark source-over | Box average of final continuous opaque scene |
|---|---|---|---|
| One width-q strip | 255 | 255*(1-q)=254.7509765625 | 254.7509765625 |
| Same width-q strip painted 1024 times | 255 | 255*(1023/1024)^1024, between 93 and 94 | 254.7509765625 |
| 1024 disjoint width-q strips that tile the pixel | 255 | Same value between 93 and 94 | 0 |
| Same tiled strips as subpaths of one fill | 0 | 0 | 0 |
| One full-pixel rectangle | 0 | 0 | 0 |

The same strip does not become larger when repainted opaquely. Therefore the
repeated coincident case cannot alone support a claim that the physically
continuous result is approximately 94. The disjoint/compound pair is a stronger
model discriminator: final geometry is identical, but separate scalar coverage
composition and integrating the final visible scene give different answers.

For a simpler correlation-only control, paint two disjoint half-pixel strips:
the byte model gives 255*(127/255)^2=16129/255; unquantized per-mark composition
gives 63.75; final-scene area integration gives 0. This has no sub-byte dropout.

## Static path establishing the dropout mechanism

Paths in an admitted opaque RGB page take N16 (compositor/plan.rs:93 and
143–182). Context::path_in at compositor.rs:783–802 requests clipped_coverage.
The rectangular path in raster/reuse.rs:269–282 returns closed-form rows.
coverage/rectangle.rs:225–231 quantizes horizontal area times row height;
coverage.rs:1163 rounds area*255 with a small half-tie guard. Here 255/1024 is
below one half by a large margin, so the resulting byte is exactly zero.
Rows::new splits that byte through Runs::new (rectangle.rs:309–315), whose
iterator excludes zero bytes (raster/spans.rs:23–30). Thus narrow_path receives
no paint spans; its source evaluation and area work charge still happen at
compositor.rs:825–826. N16 output widening cannot restore this lost coverage.

The general mask also stores coverage as bytes. Selecting F64 after coverage
construction is therefore not a remedy. Passing finer coverage into today's
N16 recurrence would need a new proof: ADR 0004:221–226 explicitly relies on
positive coverage being at least 1/255.

## Reproduced fixture recipe

Content begins `1 g 0 0 1 1 re f 0 g`.

1. Coincident: append `0 0 0.0009765625 1 re f` 1024 times.
2. Disjoint: for integer i=0..1023 append `x 0 0.0009765625 1 re f`, where
   x=i/1024. Encode x exactly as `(i*9765625)/10^10`, padding the fractional
   decimal to ten digits; do not use a binary floating-point text formatter.
3. Compound: use the same disjoint rectangles, omit every intermediate `f`,
   then append one final `f` (nonzero fill rule).
4. Single/full controls: one width-q rectangle and one full-pixel rectangle.

Use ordinary uncompressed PDF content and four minimal objects: Catalog,
Pages, Page, Contents. Or construct corresponding PathBuilder rectangles
directly in a renderer unit test to exclude PDF parser/number issues first.
Keep root fills; knockout forms would introduce a separate shape-composition
interpretation. All outputs have opaque alpha 255. Later tests may repeat the
same geometry at integer translations and compare full versus offset tiles.

## Independent gates and rejection criteria

Keep this diagnostic outside current performance admission. It must not change
or retrospectively invalidate the frozen u16 output-conversion gates, whose
contract and inputs are unchanged.

* Current composition regression: compute c_i=round(255*a_i) independently,
  then the rational source-over recurrence on c_i/255. The first three cases
  must remain white, with no suppressed source errors or reset work budgets.
* A proposed **unquantized per-mark** accuracy policy must pass the rational
  recurrence using a_i directly. Compare `abs(actual_integer - exact_rational)
  <= 1`, not tolerance one against an already-rounded oracle. For the repeated
  and disjoint tiny-strip cases the only admitted bytes are 93 and 94. Reject
  the policy if it silently falls back to the current byte-input route and
  emits 255 while claiming the stronger bound.
* A proposed **continuous-scene box-filter** policy must distinguish coincident
  from disjoint geometry: coincident output 254 or 255, disjoint/compound/full
  output 0 or 1 under a one-level allowance (prefer exact 0 for full coverage).
  The two half-strips control must also be black. Reject a scalar-coverage-only
  solution that reports approximately 94 for both tiny-strip arrangements.

These are two different possible policy changes; neither may borrow the
other's oracle only on favorable cases. Freeze the chosen reference first.
Require bounded geometry/work/memory and tile-independent decisions before
testing performance. The smallest research scope is opaque axis-aligned
rectangles; do not generalize to masks, groups or non-Normal blending here.

## Bounded future architecture, separate from current N16 work

Start with an explicitly admitted policy for opaque Normal axis-aligned
rectangles and an opaque background, initially DeviceGray/RGB constants. The
existing ordered traversal must still evaluate required source errors and
charge its work. Canonical global pixel coordinates identify each cell; request
tile boundaries must not change its coverage reference or admission decision.

* Certified full interiors keep a direct overwrite path. A later fully covering
  opaque mark also supersedes earlier boundary geometry in that cell, after
  preserving required command validation and accounting. Retain raw prepared
  color or an independently bounded representation sufficient for a later
  boundary mark; an already rounded output byte is not an exact backdrop.
* Boundary cells retain a small bounded set of unquantized clipped rectangles,
  resolved source colors and paint ordinals. Do not replace geometry with one
  marginal coverage value. Resolve only after all later influencing commands
  have been considered, or when an opaque overwrite provably erases the prefix.
* For the first prototype, subdivide a cell at its retained x/y rectangle edges.
  Each resulting face has a single topmost opaque paint. Sum face area times
  that source color, then quantize output once. This is a local exact oracle
  for the admitted dyadic fixtures; arbitrary transforms/color conversion need
  separate certified arithmetic bounds. A face arrangement is bounded by the
  retained edge count; charge construction and face evaluation, not just output.
* Cap live records, edges, faces and cumulative work before allocating or
  modifying committed output. Prefer a small per-cell scratch reused within a
  bounded tile. A naive K-rectangle partition can have quadratic faces and
  cubic evaluation work; low caps are essential, not optional optimization.
* If a cap is exceeded, choose a bounded reference that preserves the same
  final-scene oracle, or return a structured resource/unsupported result under
  that research policy. Replaying the current scalar-coverage compositor may
  preserve today's policy but does not preserve the stronger claim. Any such
  opt-in fallback must be explicitly reported and excluded from strong-policy
  accuracy admission; it cannot be called certified final-scene output.

Do not partially commit a boundary cell and then replay its prefix over itself.
Keep deferred state until success, or retain enough bounded input to restart
from the cell's original opaque background. Shared group, mask and transparency
semantics are deliberately excluded from the first admission; adding them needs
dependency-aware deferred composition, not multiplying independent coverages.

The difference between the oracles is mathematical: an area average of a
product of spatial coverage indicators generally differs from the product of
their separate averages. Repeated coincident regions are perfectly correlated;
disjoint strips are mutually exclusive. A scalar alpha per mark loses precisely
the information needed to distinguish them. Widening that scalar or the N16
output quantizer cannot recover it. This architecture changes the reference
model and must remain a separate experiment, with no current implementation or
benchmark changes.

## PDF authority and limits of this conclusion

The local ISO 32000-2:2020 EC3 reference says shape is conceptually pointwise;
11.3.7.2 note 1 permits fractional antialiased boundary shape. Its 10.7.4 also
requires intersecting device pixels to be painted, even for very small
intersections. Those clauses do not specify Pagura's 8-bit coverage precision,
a one-output-level error budget, or final-scene box filtering. The interaction
of antialiasing and finite output quantization needs a separate conformance
interpretation; this review does not assert that every unchanged output byte
violates the scan-conversion rule or that PDF mandates our box-filter oracle.

Primary reference reviewed locally: ISO 32000-2:2020 EC3, clauses 10.7.4
and 11.3.7.2. The licensed reference remains in the private reference cache;
no standard text is copied into this repository.

Recommendation: keep today's Exact composition guarantee explicitly scoped to
declared coverage bytes, and reproduce this fixture family as a separate
accuracy-research diagnostic, now reproduced, before proposing any stronger
policy. It establishes what a future stronger claim must prove without misclassifying the current N16 quantizer or choosing an unsupported PDF
oracle.

## Executed diagnostic and evidence

Four pure tests passed. The fixed six-case matrix rendered once per mode, with
no retries or timing comparison. Both Exact and Competitive returned 255 for
single, coincident and disjoint tiny strips; 0 for compound and full fills; and
63 for the two disjoint halves. Every alpha was 255. All twelve outputs pass
the strict unrounded rational comparison under the declared byte model.

The independent auditor rehashed the retained outputs, source/binary bindings,
completed runner receipts and exact arithmetic for all three references. Three
observer scans recorded no known competing jobs. Neither observed coverage
bytes nor a normative PDF verdict follows from these results: the declared
reference predicts coverage from exact mark area, rather than instrumenting it.

[Results and evidence hashes](../../benchmarks/results/subbyte-coverage-diagnostic.json)
and [executed source archive](../../benchmarks/results/sources/subbyte-coverage/README.md)
retain the experiment. The current u16 gates and retention decision are unchanged.
