# Unified renderer: independent accuracy and performance gates

Status: proposed validation additions, 2026-10-20. Existing ADR 0002 release
requirements remain binding. This document defines future evidence; none of
these new gates has been implemented or executed by the planning change.

See [ADR 0016](../adr/0016-unified-renderer.md) and the
[implementation sequence](unified-renderer.md). Baseline source:
`359e19dda5b78a23675678b6a153da7ab7fdc2c5`.

## Evidence rules

Pagura Exact is a diagnostic reference, not an independent oracle. Native/WASM
parity, identical cache outcomes, and comparisons between the two modes prove
consistency only. A new fast path needs a reference appropriate to its entire
admitted operation. Passing a sample set supports its proof obligations; it
does not establish a universal numerical bound by itself.

The legacy Competitive exceptions, including large channel differences in
some fixtures, remain explicit approximate-policy data. They cannot be used
as strict tolerances or as evidence that a kernel is more accurate than other
engines. Cross-engine distance to Exact is labelled agreement, not truth.

## A0: Freeze identity, policy, corpus, and reference

Before execution, bind source commit plus patch/tree hash, compiler and flags,
features, binary and harness hashes, independent oracle code/data hashes,
corpus/document hashes, and benchmark/accuracy policy version. Record actual
engine backend/wrapper/runtime versions, fonts/provider hashes, ICC profiles,
color intent, overprint/annotation/layer settings, recovery, entrypoint and
compilation-validation policy, page boxes, rotation, UserUnit, DPI, background, alpha representation,
filter and rounding policies, and requested output format/dimensions.

Assign pinned documents to development, validation, and holdout splits by
document and provenance family, grouping near-duplicates and producer templates.
Pages of one document cannot straddle splits. Treat already tuned benchmark
pages as development data. Freeze corpus selection, denominators, and intended
workload families before tuning. Do not shrink a required set after a failure.

Each reference declares one model and one evidence class:

| Reference model | Claim it can support | Existing foundation / required addition |
|---|---|---|
| Decoded samples and codec policy | Correct source reconstruction and image processing for the specified decoder, rounding and color contract | Independent codec vectors and `tools/fixturegen/image.py`; distinguish codec-allowed variation from renderer arithmetic. |
| Polygon filled set and hard clip intersection | Coverage against independent geometry, including nonzero/even-odd rules | Rational construction in `tools/fixturegen/raster.py`; extend adversarial joint intersections. |
| True curve coverage | Area of the original admitted curve rather than its polygon approximation | Existing independent `coverage/quadratic/oracle.rs` uses f64 adaptive Gauss–Legendre, tolerance 1e-14, depth 40. Label numerical evidence; add rigorous enclosure/high-precision adjudication for ambiguous thresholds. |
| Per-mark coverage-byte composition | PDF composition under the current declared coverage/color model | Rational fixtures in `tools/fixturegen/transparency.py` and content/paint generators; matches the scope of ADR 0004. |
| Continuous-scene integration | Integral of pointwise painted geometry, retaining inter-mark spatial correlation | Separate independent rational/local-scene research oracle. Not interchangeable with today's per-mark contract. |
| Real-document compatibility | Adjudicated content/geometry/color requirements for the identified page | Standards-based investigation with multiple implementations; unknown pages remain unverified, not oracle passes. |

Evidence classes are `exact_rational`, `rigorous_enclosure`,
`independent_numerical`, and `adjudicated_semantic`. Record precision and
limitations. An enclosure spanning unacceptable outputs is inconclusive.
An independent numerical oracle is not relabelled a formal proof.

Extend existing `Case.extra` in `tools/fixturegen/model.py` and schema-2
manifest generation in `tools/fixtures.py`; do not introduce a competing
fixture system. Validate reference model, code/hash identity, rounding/tie
policy, coverage quantization, source interpretation, channels, and final bound.
The generator must not run/import Pagura to manufacture expected outputs.

## A1: Semantics and structural parity

Structural changes preserve both modes separately: pixels, relevant structured
errors, source object/position, repairs, diagnostics and their discovery order,
explicit unsupported behavior, and documented validation scope. Do not compare
only successful images. Font validation depends on entrypoint/compilation as
well as Accuracy: `compile_page()` and retained host lists use full font loading,
while compile-and-render Competitive can use drawing-only loading. Test those
routes separately, including damaged unused tables and cached failures. Do not
reuse partially validated lists as fully validated ones. Changing the policy
needs an explicit decision before comparing equivalent work.

Required cases include source-order competing errors, `q/Q` and content-stream
boundaries, form resource scopes, operator validation before visible painting,
offscreen malformed input, groups with unbounded influence, and optional
preparation refusal. An index may omit a visit only if the omitted work is
proven effect-free under that policy. A geometry or semantic Limit error is
not an optional-cache refusal.

Fix an independently proven existing bug in a separate correctness change;
then rebaseline the structural comparison. Do not freeze known wrong behavior
as truth merely to keep byte parity.

## A2: Independent final-output accuracy

For current strict execution, enforce the applicable ADR 0004 reference and
final bound: exact expected output where required, otherwise at most one
channel level under the independently justified contract. Keep the separate
f64-reference rule for amplifying operations explicit; do not promote it to
a universal exact-real guarantee.

There is one final allowance, not one per stage. Include geometry/coverage,
sample rounding and clamps, Decode, nonlinear color conversion, mask/matte,
filtering, repeated composition and final quantization in the dependency
argument, starting from the inputs defined by the selected reference model.
Do not charge geometric error twice or claim that a coverage-byte composition
proof establishes continuous-scene integration. If a path differs from Exact by one and Exact differs
from the independent reference by one, the result has not proved this gate.

Every promotion records:

1. The admitted mathematical operation and exact preconditions.
2. Where those preconditions are checked before information is discarded.
3. The total error argument, including downstream amplification.
4. An independent oracle and adversarial cases near admission/rounding boundaries.
5. Refinement/fallback when the proof does not apply, including its work cap.

Retain per-channel maximum error and every affected fixture pixel; report
within-one share, MAE, tail quantiles, and locations as additional diagnostics.
An average cannot waive a strict localized failure. Annotate a changed sampling
model instead of raising tolerances to hide its differences.

The PPM CLI conformance comparison checks RGB. Add direct premultiplied-RGBA
checks for alpha-bearing kernels and independently expected composites over
black and white. Neither RGB equality over white nor an alpha-ignoring metric
is sufficient for a transparency claim.

## A3: Invariance and adversarial operation matrix

| Area | Required cases |
|---|---|
| Index and bounds | Tiny/empty pages; every command in every tile; far-offscreen and broadcast entries; degenerate paths, hairlines, miter bounds; non-finite bounds; ordered groups; duplicate/drop prevention. |
| Clip geometry | Coincident and complementary partial regions; slivers; nested/crossing clips; both fill rules; self-intersections; repeated clips; exact-boundary ties; large global origins; reflection. |
| Images | Admitted transformations and explicit rejected-path controls; quarter turns, reflection, fractional/negative origins, anisotropic scales, shear fallback; source/page/crop edges; all admitted chroma grids; Decode inversion; packed samples; masks with differing dimensions; zero/near-zero alpha and per-sample matte removal. |
| Precision and groups | Many tiny-alpha stores, alternating colors, values near rounding ties; narrow surfaces feeding later transfers/masks; shape distinct from opacity; isolated/nonisolated and knockout groups; mask backdrop and color-space transitions. |
| Caches and identity | Disabled, cold, hit, eviction and optional-budget refusal; two documents with reused object numbers; revision/form/provider changes; distinct DPI, transform phase, color and accuracy policy; retained owners versus recycled buffers. |
| Resource safety | Incidence count/byte overflow, deep clips, dense intersections, fallback storms, tiny budgets, simultaneous scratch+retained peak, concurrent publication where supported, source-position preservation on failure. |

Metamorphic checks: full-page output equals assembly from arbitrary tiles,
including one-pixel and irregular partitions; matching integer translation of
scene and viewport; equivalent content-stream splits; permitted state
save/restore transformations; cache history equivalence; native/WASM and
scalar/SIMD parity under identical options.

Do not assert that splitting one paint into several paints is universally
equivalent: antialiasing, transparency and repeated coverage can change it.
Changes to continuous-scene sampling require their own versioned metamorphic
relations and independent reference.

## A4: Work, storage, failure, and safety bounds

Charge conservative capacities and bookkeeping before publication; account
for live output/group surfaces, decode planes, index/clip scratch, retained
entries and host storage together when reporting peak memory. Keep each
owner's enforced budget separate from allocator/RSS measurement.

Cache hits cannot bypass mandatory semantic work accounting; retries cannot
reset work charges or hide required errors. Bound optional preparation in a
separate planning allowance and report its work alongside mandatory execution.
A failed optional attempt must not spend the legacy semantic budget needed by
the fallback; total actual work remains explicitly bounded by both allowances.
Any aggregate policy change is separately reviewed. Remember declined
preparation during the render so every band does not retry.
Optional admission refusal falls back before mutation and releases reservations.
It must be distinguishable from required Limit/Unsupported/Syntax failures.

Preserve no-unsafe, no implicit host capability, independent implementation,
and dependency policies. Include malformed-input and resource-regression
tests for new plans; do not claim memory-safe Rust alone proves bounded work.

## A5: Coverage and adjudication

Publish open, enumeration, strict/repaired success, pages rendered, independently
verified pages, unsupported tags, crashes, timeouts and resource failures
separately. Do not remove failed pages from timing denominators or report
partial pages as successful output.

Classify discrepancies as implementation defect, oracle defect, declared policy
difference, unsupported feature, or unresolved. Record reviewer, supporting
specification/proof, and reference version. Majority agreement and screenshots
are investigation aids, not normative truth. An unresolved case blocks the
affected correctness/performance claim. Oracle corrections require separate
review and reruns of all affected engines, not unilateral golden replacement.

When a holdout failure becomes an optimization target, retain it as a regression
case and explicitly version a new untouched holdout. Preserve the original
failure and denominator in the report.

## T0: Repository checks and independent execution control

`python3 tools/check.py` remains the repository contract for implementation
changes. It includes formatting, safety/architecture policy, Clippy, Rust and
Python tests, fixture regeneration verification, and CLI integration.

Do not run it during this planning change. It is not currently a guaranteed
single-worker command: conformance defaults to eight workers, and Cargo/Rust
tests have their own concurrency. A reserved serial validation mode must
explicitly forward and verify limits for each subprocess before a restricted
runner can be described as serial. Do not invent an already-supported flag.

Builds, tests, profiling, fuzzing and timing require a positive exclusive lease
or an explicitly reserved idle window, as defined in the implementation plan.
Each phase runs alone. Shared CI may run functional checks; it supplies no
performance evidence. Remote execution is not authorized by merely finding a
host name; establish runner availability and scope first.

## P0: Benchmark eligibility and tracks

Reuse existing `tools/bench.py`, `tools/benchkit/`, policy and release matrix.
Keep the six required baselines: PDFium, MuPDF, PDF.js, Poppler, Hayro and Apple
CoreGraphics where declared applicable. Adobe remains excluded under ADR 0011;
a licensed automation baseline would require a new decision.

| Track | Complete timed operation | Required distinction |
|---|---|---|
| Cold process/document | New process, open/enumeration and selected complete document rendering; report startup, emit and total elapsed too | OS page caches may be warm after warmups. This is not cold storage. |
| First render in a resident engine | Open context followed by the first render with preparation and allocation charged | Do not substitute a retained rerender. |
| Warm retained page | Reuse the declared document/list state and render again | A persistent runner is implementation work, not an existing round5 capability. |
| Viewer region sequence | First tile, panning, zoom, eviction and full tiled traversal, including host transport/output extraction | Keep first-tile latency separate from total document throughput. |
| Native portable CPU | One thread, then separately fixed equal thread counts where supported | Record ISA/backend dispatch; tuned native is a separate track. |
| WASM | Scalar/SIMD and each runtime/compiler policy separately | Record module compilation/instantiation, stage timing and full elapsed. Node WASI does not establish browser experience. |
| GPU, later | Screen output and CPU pixel output separately | Include compilation, upload, synchronization and required readback; do not mix resource classes. |

Existing stage boundaries remain open/render/emit. A competitor with only
process elapsed data cannot be compared to Pagura render-only time. Every
required output is checked outside timing; normalization cannot silently crop
mismatched dimensions to a common rectangle. Pixel policy differences require
adjudication before a cell can support a correctness-qualified speed claim.

The existing release matrix uses 72/144/300 DPI; the round5 72/150/300 campaign
is exploratory. The 150-DPI row cannot fill a 144-DPI release cell. Existing
round5 sample counts and stored competitor medians are not a replacement for
fresh randomized paired release samples.

## P1: Proposed per-change decision policy

This is a proposal for admitting implementation changes, not a claim that the
repository already implements it and not a relaxation of P2. Freeze the target
and protected cells, reference, paired sampling plan and stopping rule before
measurement. Use at least five warmups and 100 randomized measured pairs for
an acceptance decision. One hundred pairs is a minimum, not evidence that the
p95 is stable; predeclare sample size/replication from a separate pilot, with
time blocking or another validated treatment of temporal dependence as needed.
A pilot is exploratory and can never admit the candidate.

- Every designated target cell of a nontrivial optimization must have a
  one-sided 95% upper confidence bound on median candidate/baseline latency
  <= 0.95, and p95 <= 1.00. No post-hoc aggregate substitutes for a failing cell.
  This proposed five-percent floor prevents retaining substantial machinery
  for an indistinguishable microbenchmark gain.
- Protected families include tiny cold pages, dense/all-overlap scenes,
  fallback-heavy pages, non-target content, and complete tiled traversal.
  Their median and p95 upper bounds must be <= 1.02. This explicit proposed
  two-percent overhead guard is not called zero regression and must be shown
  in every decision report. A stricter policy can be adopted before runs.
- Structural refactors make no speed claim; they must satisfy the protected
  guard, all correctness gates, and a concrete simplification/reuse objective.
- Preserve the checked-in no-peak-memory-regression rule, plus explicit retained
  and transient byte limits. Missing memory evidence cannot pass. A desired
  memory tradeoff needs its own reviewed policy decision before acceptance.
- Compare complete requested operations including preparation. Report actual
  counters: visited commands, index incidences/build bytes/time, joint boundary
  cells, fallbacks, decoded/converted rows, output pixels, cache hits and retained
  bytes as relevant. Profiling counters run separately if they perturb timing.

Use one fresh, fixed-size confirmation run as the sole acceptance analysis;
neither pilot data nor interim looks can admit a candidate. Freeze the candidate,
cells, sample count and analysis before confirmation. An inconclusive result
remains inconclusive: another opportunity to pass requires a separately reviewed
sequential-inference protocol, not an automatic second attempt. Preserve all
runs and cells. Candidate tuning ends before heldout measurement.

Intervals crossing admission boundaries are inconclusive. A validated lower
bound beyond a prohibited-regression threshold supports rejection; a lower
bound above the useful-gain floor can reject unjustified complexity. U1 must
implement these lower bounds or an equivalent preregistered test: today's
`ratio_bounds` returns upper bounds only. Missing an upper-bound pass threshold
alone proves neither regression nor insufficient benefit. Reports distinguish
the all-cells acceptance decision from any stronger simultaneous-confidence
claim about every individual cell.

## P2: Existing release comparison gate and missing machinery

ADR 0002 remains the release requirement: median AND p95 one-sided 95% upper
confidence ratios <= 0.80 against each required competitor in every declared
applicable matrix cell, at least 100 paired measurements/five warmups, correct
equivalent output, and no peak-memory regression. Category geometric means
and the Competitive development thresholds do not replace it.

Missing/failed/timed-out/unsupported/incorrect/incomparable required cells block
the gate. Applicability is declared before runs; it is not decided by which
engine wins. No speed claim is made for an output still awaiting adjudication.

Two current harness gaps are explicit implementation work:

1. `tools/benchkit/statistics.py` expects exact reference pixel hashes. Add a
   reference-bound verifier for reviewed nonzero tolerances that validates the
   actual artifact, model, settings and bound. Keep exact-hash verification
   for exact cases. Add negative tests against fabricated status, wrong oracle,
   wrong alpha/dimensions, stale settings, and unauthorized tolerance changes.
2. `tools/benchkit/gate.py` deliberately blocks release until complete keyed
   cross-platform/resolution/concurrency aggregation exists. Implement and test
   that aggregation; do not remove the blocker while cells remain unbound.
   The matrix also leaves operations such as `open-cold`, `render-cold`,
   `text-search`, `save-noop`, `edit-save`, and workload families unfinished.
   B1 tracks those separate product/harness prerequisites; B0 alone cannot
   produce complete release evidence.

An engine leadership claim is limited to the declared matrix and independent
accuracy evidence. It does not establish superiority on every possible PDF.

## Machine-readable evidence and dispositions

Extend existing records rather than inventing a parallel report pipeline.
Validate fields at ingestion; preserve underlying artifacts and raw samples.

| Group | Required additions/bindings |
|---|---|
| Identity | Gate/case ID, split version, source/tree/binary/harness/policy hashes, all output settings and environment/backend identities. |
| Reference | Model/class, oracle data and code hashes, precision/enclosure, final bound/channels, proof/adjudication ID. |
| Correctness | Semantic, independent accuracy, internal parity, resource and coverage statuses; actual output hash, dimensions, per-channel errors and failure locations. |
| Execution | Runner identity/lease receipt, CPU/thread/resource class, cold/warm definition, randomized pair order, timing boundaries, raw measurements and confidence method/bounds. |
| Cost | Peak RSS, retained/transient accounting, preparation cost, relevant work counters and fallback counts/reasons. |

Allowed final dispositions:

- **Accept:** all applicable gates pass and added complexity earns its declared benefit.
- **Reject:** demonstrated correctness/resource failure, prohibited regression, or confidently inadequate benefit; remove abandoned machinery.
- **Inconclusive:** insufficient/contaminated evidence, uncertain reference, ambiguous identity, or confidence intervals crossing the threshold. No promotion or claim.
- **Deferred:** necessary runner, oracle, backend or aggregation is unavailable. Keep the missing requirement visible; it blocks the corresponding completion claim.

Correctness failures are never traded against speed. Plans, source inspection,
successful tool invocation, old reports, and green unrelated CI are not run evidence.
