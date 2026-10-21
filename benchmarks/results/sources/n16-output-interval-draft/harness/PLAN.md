# N16 painted interval: bounded incremental experiment

Draft for coordinator review, not a frozen launch plan. No harness, fixture,
product code, imports, tests, builds, renders or measurements were executed to
prepare this document. Freeze the concrete baseline commit, source and binary
pins, expanded cell list, seeds and every helper before the pilot. The baseline
is the retained reconciled stack at `a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`
when this source-only reconciliation was prepared. It includes the glyph memo,
clip cleanup, shared opaque composition, retained u16 converter, nonopaque-
background F64 guard and preserved F32 admission in both accuracy modes. Freeze
the actual preceding commit again if another product change lands first.

## Decision and hypothesis

Proceed with painted intervals before prepared source spans. The coordinator's
earlier retained-product profile attributes 171 of 540 render-subtree samples to
the N16 output vector loop after examining the saved disassembly; Buffer::path
has 21 of those 540 samples. Bind that profile's record, source, binary,
disassembly and interpretation when freezing. These are whole-process sampling
observations filtered to a render subtree, not elapsed-time fractions or a
predicted speedup. Full-image checksum, printing and other process work must
remain separately reported. Prepared source words are a later experiment.

The product candidate is only the current source-only patch in
`/private/tmp/pagura-n16-interval-current/painted-interval.patch`, SHA256
`c71d0c06c2f7c621d8c4a67b875e91070b780caa9dc600a71da5d9df7d676ecb`: one
conservative flat half-open interval in each N16 buffer, widened by both existing
validated writers, and intersected with rows only at Context::write_output.
Buffer::output retains its dense behavior for arbitrary destination bytes.
Keep all pixel initialization, RGBA channels, quantizer arithmetic, source
evaluation, traversal, work charges, allocation admission and error ordering.
No per-row metadata, coverage scans, alpha removal, scratch reuse, source cache,
new public abstraction or prepared-span change belongs in this experiment.
The reconciled base includes the integrated checked legacy U8 charge/accounting
changes, glyph payload budget follow-up and F32 glyph-fragment support; freeze
both arms on the same current integrated baseline. The retained profile above must identify its own
source state; do not silently attribute it to a newer baseline.

For the existing opaque64 geometry, each 256x256 chunk contains one 256x6 strip.
Its ideal flat interval covers 6/256 = 2.34375% of the chunk, versus the current
full output conversion. This is a geometric reach prediction, not an observed
skip count or speed forecast. Output Pixmap initialization and N16 initialization
still touch the full surfaces; interval holes are still converted. An untimed
structural test must verify actual N16 admission and actual writer/output reach
in both modes before timing. Do not infer reach solely from a PDF filename.

## Fixed native matrix: 52 cells, 2 targets and 50 protected

Take all 46 cells, exact roles, modes and operations from
`benchmarks/results/n16-output-u16-plan.json` without deleting or retargeting any
cell. Only opaque64/exact/full and opaque64/competitive/full are targets.
Add these six protected cells, all at 72 dpi/page 0 with an opaque white backdrop:

| Case | Operation and modes | Exact recipe and independent expected output | Purpose |
| --- | --- | --- | --- |
| opaque-empty | full, Exact and Competitive | 512x512 PDF with an empty content stream and no painting commands; every RGBA byte is 255 | All N16 intervals remain empty; protect fixed per-chunk/row overhead and establish skip reach |
| opaque-distant | full, Exact and Competitive | 512x512 PDF, opaque black integer 1x1 rectangles at `(256*cx,256*cy+255)` and `(256*cx+255,256*cy)` for each `cx,cy` in `{0,1}`; eight black pixels, every other pixel white, alpha 255 | PDF-to-device y inversion puts the marks at each 256x256 chunk's first and last linear pixel, so its interval spans the whole chunk despite sparse paint; expose bookkeeping overhead |
| opaque-tiny (existing fixture) | full, Exact and Competitive | Reuse unchanged 64x64 PDF and independent oracle with its black 8x8 mark at `(24,24)` | Isolate tiny N16 render overhead already diluted into the existing cold cells |

Generate the new two PDFs using existing fixturegen.pdf mechanisms. Derive their
RGB occupancy directly from the integer coordinates with the established
top-down row inversion; derive opaque RGBA independently, without decoding the
renderer output to construct expectations. Content construction trusts a pinned
generator; the expected pixels do not. A 512x512 empty page must still be shown
to traverse N16 output in the tested production route. An optimized-away or
bypassed case cannot serve as evidence for this mechanism.

The existing matrix supplies opaque64 cold/full/first-tile/complete-tiled in both
modes; dense64; opaque-fill; opaque-clip; tiny cold/full; many short orthogonal
glyph spans; nested and default-knockout groups; image-only; and memo-capacity
controls. Keep the existing public scene_index_profile example unchanged.
Its `tile` operation is specifically the top-left at most 256x256 tile, while
`tiled` uses a complete 256-grid partition and includes assembly. Neither one
alone is an arbitrary-offset seam test. Existing `cold` includes open, compile
and render, excludes file reads/output writes, and is not cold filesystem I/O.
The unchanged profile uses default Repair opening and full `compile_page`, and
does not emit diagnostics. Strict public correctness probes remain separate.

Prove storage routing separately: opaque64/fill/tiny/empty/distant must select
N16; ordinary admitted translucent solid/image controls select F32 in both
Exact and Competitive; default-knockout64 and
outline2-knockout retain F64. A nonopaque page background always forces F64.
Confirm these using untimed structural tests or a diagnostic-only build whose
instrumentation is absent from both measured binaries. A direct image route
that bypasses the buffer is a useful protected control, not proof of F32 output
reach; retain a direct translucent path/glyph test for that branch.

## Accuracy, errors and resource gates before timing

All 52 benchmark cells must match their independent RGB8 oracle with tolerance
zero, alpha 255, and byte-for-byte baseline/candidate equality within each mode.
Verify and retain every warmup and measured output. Same-mode baseline equality
is supplementary; neither baseline nor Pagura Exact is the correctness oracle.
New integer-edge cases have independent exact pixel expectations. Existing
fractional precision cases instead use their explicitly declared coverage-byte
composition model, not continuous-scene coverage or a general PDF perfection
claim. Keep the recent subbyte three-reference diagnostic separate from this
experiment's admission criteria.

Before a pilot, require the following named gate groups in both modes where
the public interface exposes modes:

Supplied but unexecuted: the current patch contains private writer, dense-output,
colored-background, same-chunk multirow-hole, public irregular/offset-tile and
painted-to-blank context reuse tests. Follow the current
`/private/tmp/pagura-n16-interval-current/VALIDATION-CHECKLIST.md`; the draft is
not validation evidence until the coordinator applies and runs it. Actual N16
admission/output reach, current budget/error comparisons, native/WASI layouts
and the complete validation gates remain outstanding.

1. **PI-Q1 — interval invariants and writers.** Apply/review the prototype's
   narrow tests: initial empty interval; empty/zero/full coverage; repainting the
   background; first/last indices; failed bounds leave the same earlier state;
   negative/nonzero fragment origins; fractional clip products; valid first row
   followed by a failing clip row preserves the first row's interval and error.
   Exhaustively preserve all 65,536 u16 quantizer outputs and dense
   Buffer::output offsets, tails and arbitrary destination contents even when
   its interval is empty. No untracked N16 writer is permitted.
2. **PI-Q2 — cross-row output and reused storage.** Run the current case whose
   interval begins mid-row, ends in a later row, and contains untouched holes,
   with a nonzero tile/chunk origin and output stride different from chunk
   width. It crosses x=256; the separate public tests cover x/y=256 and 512.
   Verify opaque colored background bytes, not just white. Independently
   compare full renders with
   offset tiles and an irregular complete partition including widths/heights
   1, 255, 256 and 257 where valid. Include empty chunks and the new distant
   marks. Exercise the public context/storage reuse path across painted then
   blank outputs with a changed opaque background so stale interval/output
   contents cannot escape. Assert actual output-allocation reuse in Competitive;
   Exact currently does not borrow context spare storage but must pass the
   same independent pixel checks. Keep legacy U8 and admitted F32 dense-output
   controls. These are correctness tests, not added timing cells.
3. **PI-Q3 — background and precision.** Keep
   display::direct_root_backgrounds_remain_premultiplied_in_both_modes with
   straight RGB(200,100,50), alpha 0/128/254/255, blank/partial/half-opacity marks
   and offset tiles. Alpha below 255 must remain on F64; transparent blank output
   must be premultiplied zero. Keep direct-root rational fixtures in full/tile
   under both modes: three-mark reference 44617/204 permits output 218..219;
   1000 coverage-1 black marks give 255*(254/255)^1000 and permit 5..6; alpha is
   exactly 255. No Competitive-specific tolerance increase.
4. **PI-Q4 — failure/resource equivalence.** Preserve visible, off-page,
   requested-tile-miss, nested-off-page and nested-tile-miss ICC/Type4 fixtures:
   exact error kind, feature, object, source position, diagnostics and full/tile
   parity. Keep singular off-page stroke errors and glyph-metadata exclusions.
   Preserve function-work, command-work, raster-area, decoded-byte, pixel and
   depth boundary behavior, including equality at the successful limit and the
   immediately failing next request. Reuse existing source validation and
   narrow rectangle budget tests; compare baseline and candidate error values
   and consumed counters, not only whether both fail. Work must not disappear
   because output is blank or skipped. N16 pixel payload remains count*8.
5. **PI-Q5 — integrated validation.** Run the repository's complete authoritative
   check serially against the exact candidate, plus the precision/native-WASI
   host probes with strict diagnostics and response/pixel parity. The default
   host interface establishes Exact parity only; Competitive is covered through
   the native and WASI example/test interfaces, not an invented host mode.

Relevant existing anchors: compositor/plan.rs:51-115 and :276-321;
compositor.rs:1645-1771 and :2572-2655; display.rs:584-617, :768-846,
:848-911 and :916-1007; compositor/sources_tests.rs; compositor/narrow_tests.rs;
compositor/output.rs. The line numbers refer to the source read while drafting
and should be refreshed after the prototype is integrated.

## PI-M1 — actual native and WASI layout observations

Before timing, run a tiny internal size/alignment probe on BOTH baseline and
candidate for the actual native target and wasm32-wasip1, with pinned toolchain,
flags, source, executable/module and runtime. Report raw `size_of` and
`align_of` values for Buffer, Option<Buffer>, Surface and MaskSink; also report
the N16 payload/Range<usize> representation and target pointer width to explain
the delta. Use private module test access or a diagnostic-only probe; no public
API or benchmark-product instrumentation is needed. If a cfg(test) probe is
used, verify that these types have the same definitions/layout-affecting cfgs
as the measured production code. Execute the WASI probe under the pinned
runtime; a host calculation assuming 32-bit usize is not an actual WASI result.

This first slice requires no size or alignment growth of the four enclosing
types in either target, no new heap allocation, and unchanged count*8 N16 pixel
charges. The candidate's Words payload may visibly add its two usize fields;
that fact alone neither proves nor disproves enclosing enum growth. If the
enclosing layouts grow, stop and revise or explicitly re-plan before timing;
do not quietly redefine the memory gate. If a layout target cannot be observed,
mark it unavailable and do not claim portable layout neutrality. Retain native
per-process RSS gates for every cell regardless of static layout results.

## Fixed phases, measurements and execution ownership

Proposed fresh seeds to freeze with the new experiment `n16-painted-interval`:

| Phase | Pairs/cell | Warmup pairs/cell | Order seed | Expected verified outputs |
| --- | ---: | ---: | ---: | ---: |
| exploratory pilot | 5 | 5 | 20261105 | 1,040 |
| sole fresh confirmation | 200 | 5 | 20261106 | 21,320 |

Reuse the existing paired runner's predeclared randomized cell/order algorithm,
fresh-process samples and metric definitions. Pin its source rather than
changing scheduling during this slice. Report raw paired values, order and
timestamps; full/cold stage counters, median/p95 ratios and all memory values.
Use the existing fixed paired bootstrap calculation: 5,000 resamples,
one-sided 95%, seed 42017. This describes analysis of the one fixed sample set,
not permission for further measurements or repeated confirmation attempts.
Bounds are marginal; neither 200 pairs nor a green subset establishes calibrated
p95 precision or simultaneous coverage. Do not trim outliers or pool historical,
pilot, canceled, A/A or profiling observations into confirmation.

The pilot can reject an unhelpful candidate or justify spending the fixed
confirmation budget; it cannot admit the change. Freeze a decision to launch
the full 200 pairs before confirmation starts. One launched confirmation only.
No optional stopping, extra pairs, changed seeds/cells/roles/thresholds, automatic
retry or retuning against confirmation. Any revision requires a separately
named and explicitly predeclared experiment with all old evidence retained.

One coordinator owns builds, checks, diagnostic probes, profiling, audit and
measurement; agents perform read-only review or off-tree drafting. Use the
existing cooperative runner plus known-job observer, three quiet preflight scans
and 5-second observations. Wait for scheduling conflicts before a measured
child launches; a zero-child preflight refusal is not a statistical trial.
No overlapping owned compute or attachment to another task's renderer. Preserve
the existing 5,400-second launched-run ceiling, per-child limits, cancellation
cleanup and no automatic postlaunch retry. Observed postlaunch contention makes
admission unavailable without erasing the run; interruption/missing evidence
also cannot pass. Record ordinary desktop/background limitations honestly.

Keep the existing 64MiB per-output and 512MiB retained payload budgets with
content-addressed deduplication and all warmups. Report/checkpoint/receipt/observer
files are additional storage, as in the existing plan. Capacity exhaustion is
incomplete evidence, not permission to drop samples. Preflight identity hashing
warms filesystem caches, so there is no cold-I/O claim.

## Bounded WASI portability diagnostic before native confirmation

After native preflight/pilot quality and layout checks, reuse the existing
Node-WASI scene_index_profile mechanism, separately from native admission.
Freeze these 12 full-render cells: opaque64, opaque-fill, opaque-empty,
opaque-distant and opaque-tiny in both modes (10), plus image-only Exact and
default-knockout64 Exact (2). The complete native matrix and correctness gates
still protect both-mode fallbacks. Use two warmup pairs and 12 measured pairs
per cell, balanced six AB/six BA measured orders, fresh seed 20261107, exactly
336 verified outputs; no retry. Build both WASM modules serially from the same
source states as native and pin them and their completed build receipts.

Retain independent raw RGBA/oracle verification, pairing, arguments, all stages,
Node executable/version/options, module hashes, and exported linear-memory byte
length before/after execution. Linear memory is not Node RSS. Each fresh Node
instance uses default tiering; internal render_ns excludes process startup and
explicit module instantiation, but lazy compilation/tiering may occur in the
render interval. Do not compare native and WASI ratios as one common gate.

Any independent pixel/alpha mismatch blocks retaining this portable candidate.
Predeclare the existing diagnostic triage threshold for the 10 N16 cells:
median candidate/baseline render_ns >1.05 with at least 10/12 individual paired
ratios >1.02 suspends portable promotion and native confirmation pending revision
or investigation. Report the two fallback cells completely; a similar material
signal also needs explicit review before proceeding. This is a small-sample
triage rule, not calibrated speed inference. Absence of that signal is neither
a WASI speed claim nor release acceptance.

## Unchanged quantitative gates and explicit retention decision

For candidate/baseline ratios:

- Each of the two designated target cells: median upper bound <=0.95 and p95
  upper bound <=1.00.
- Every protected cell: median and p95 upper bounds <=1.02. This is the existing
  proposed 2% overhead guard, not a zero-regression guarantee.
- Every native cell: sampled candidate max peak RSS / baseline max peak RSS
  <=1.00. Report the absolute maxima and distribution as well as the ratio;
  unchanged storage or a noisy A/A diagnostic does not erase a miss.
- All quality, failure, layout, resource, provenance and complete-validation
  gates above must pass. Correctness failures are not negotiable for speed.

**Full experiment pass** requires all 52 latency/RSS cells and every mandatory
gate. It is still not the release/competitor gate: the separate fair complete
matrix's 0.80 per-cell target, source/runner requirements and independent quality
requirements remain untouched. No worldwide, cross-engine or net-stack claim
follows from an incremental synthetic experiment.

**Partial experimental retention** preserves the standing policy: at least one
of the original two targets must reproduce median upper <=0.95 in the sole
fresh confirmation, with every mandatory correctness/resource/layout/provenance
gate complete. Keep target p95 failures, every protected/RSS miss and any
supported regression visible. A target median pass alone is not a full target
latency pass. A protected median or p95 lower bound >1.02 is a supported material
signal under the frozen marginal calculation and requires revision or an
explicit, reasoned experimental restriction; it cannot be silently dismissed.
An upper-bound miss alone is not proof of a regression. Retention is a recorded
coordinator decision bound to the completed audit, not an automatic evaluator
side effect. No deployed/default/release improvement claim is implied.

**Reject/revise** on independent accuracy, alpha/premultiplication, source/error
context, work/allocation admission, tile contract or layout failure; on a
completed confirmation without the required target median benefit; or when
bookkeeping/maintenance cost defeats the useful gain. Keep the original patch
and all failed evidence. **Inconclusive/unavailable** covers incomplete samples,
missing/tampered artifacts, contention, canceled runs and unavailable layout or
portability validation. It gives no new speed/admission evidence and does not
justify another attempt until a new plan is explicitly authorized.

## Freeze and publication checklist

The new machine-readable plan must enumerate all 52 unique case/mode/operation
triples, roles and phase settings. Bind pristine baseline commit; candidate
patch/product fingerprint; both native and WASI binaries; actual build receipts
and flags; the unchanged public example; new entry/fixture files; paired runner;
every transitively imported fixture/helper; independent oracle derivations; fixed
policy; runtime/OS/CPU; layout probe source/results; observer/runner; and each
phase's report/receipt/audit. Source_identity omits benchmark Python scripts, so
explicit harness hashes are mandatory. The actual imported checkout must match
the recorded candidate root. Inputs are owned/frozen before invocation; all
output bytes are reread and verified outside render timers.

Publish all target, protected and RSS pass/miss counts, lower/upper bounds,
incomplete gates and limitations, plus the explicit retention decision. Keep
profiling, analytic correctness, native timing, WASI triage, layout observations
and independent error adjudication distinguishable. The final conclusion should
say only what the completed experiment establishes.
