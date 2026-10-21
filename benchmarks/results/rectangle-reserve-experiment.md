# Rectangle reservation: measured gains preserved, candidate parked

2026-10-21. The five-segment reservation lowered the primary many-operands
median by **9.9% in Exact and 7.3% in Competitive**, with all 15,580 confirmation
outputs passing the frozen independent references. The full repository check
also passed. The complete target latency gates pass **0/4**, protected latency
**8/34**, and peak RSS **26/38**; no complete admission is claimed.

The candidate is parked because a subsequent pinned-toolchain capacity probe
confirmed a deterministic retained-memory increase on valid compound paths.
The frozen resource-regression rejection rule is not waived. The complete
[224-line candidate patch](sources/rectangle-reserve/candidate.patch), including
six tests, remains committed and reproducible with `git apply --unidiff-zero`.
The measured gains and all evidence remain available for a bounded revision.
This is preservation of an experiment, not acceptance of its resource tradeoff.

## What was compared

The baseline is `52d8e7f41e2bfc4d38bff394adcb2f4a1783a8c2`. The candidate is
the frozen rectangle overlay on `2331c658686e4f361c30a98505a9fd37c75bbf63`,
identified by its source fingerprint and binary, not that commit alone.
The local retained example invokes the full-validation public compile/render
API. All 19 fixed analytic pages run in both modes at 72 DPI, using `cold`:
internal open + compile + render. It excludes input reads, output emission,
process startup and destruction. The filesystem is preflight-hashed/warm.
RSS is the fresh child's whole-process OS high-water value.

Confirmation used 200 fresh pairs and five warmups in every cell, independently
verifying all **15,580 outputs**, including warmups, against the frozen RGB
oracles, opaque alpha, and same-mode paired bytes. The five-pair pilot is
separate, with 760 verified outputs. No pilot samples enter confirmation.
Neither analysis reports observed known-job contention; polling does not
establish OS exclusivity or a noiseless desktop.

**Shared upstream divergence:** both arms retain image-sampler reorder
`788db1ef`, differing from main correction `c19c5374`. It is
common to both rectangle arms. This comparison isolates the rectangle change
on those frozen stacks; it neither validates that sampler divergence nor
establishes equivalence to current main. Reconciliation must preserve these
historical identities and receive its own validation. No universal PDF-accuracy
claim follows from the selected analytic image models passing.

## Target results

Ratios are candidate/baseline. Bounds are the frozen one-sided 95% percentile
bootstrap endpoints; lower is faster/smaller. A target requires median upper
at most 0.95 **and** p95 upper at most 1.0. RSS requires the ratio of sampled
per-arm maxima at most 1.0.

| Target / mode | Baseline → candidate median ms | Median ratio / upper | p95 ratio / upper | Max-RSS ratio | Result |
|---|---:|---:|---:|---:|---|
| Primary many-operands / Exact | 13.6933755 → 12.3358125 | 0.900860 / 0.906789 | 0.905048 / 1.056188 | 0.877687 | Median and RSS pass; tail gate misses |
| Primary many-operands / Competitive | 17.192125 → 15.9375205 | 0.927024 / 0.948067 | 0.920176 / 1.052420 | 0.884903 | Median and RSS pass; tail gate misses |
| Secondary many-rectangles / Exact | 1.2622705 → 1.183146 | 0.937316 / 0.951294 | 0.955890 / 1.049218 | 1.038462 | Median, tail and RSS gates miss |
| Secondary many-rectangles / Competitive | 1.5267915 → 1.441604 | 0.944205 / 0.952422 | 0.876818 / 1.014020 | 0.981982 | Median and tail gates miss; RSS passes |

Both primary RSS median ratios are `0.870992963252541` (about 12.9% lower).
Their observed maxima are 21.078125 → 18.5 MiB in Exact and
20.90625 → 18.5 MiB in Competitive. These are sampled process distributions,
not bounds on future maximum memory.

No confirmation cell's median or p95 lower bound exceeds 1.02. This satisfies
that specific partial-retention prerequisite; it does **not** prove absence of
regression, adequate tail precision or that all protected upper-bound gates
passed. Latency point estimates are independently checked by the auditor, but
its `bootstrap_bounds_independently_recomputed` is false. RSS median/p95 bounds
are recomputed from the raw pairs. Marginal intervals are not simultaneous
familywise guarantees, and 200 pairs do not guarantee temporal independence.

## Every confirmation gate remains visible

Latency below means both applicable median and p95 upper-bound checks.
Protected thresholds are 1.02/1.02; the two target families retain 0.95/1.0.
`MISS` is an unmet acceptance bound, not automatically a proven regression.

| Case | Exact latency | Competitive latency | Exact RSS | Competitive RSS |
|---|---|---|---|---|
| classic/blank | PASS | MISS | PASS | PASS |
| classic/rectangle | PASS | MISS | PASS | PASS |
| classic/graphics-state | MISS | MISS | MISS | PASS |
| classic/negative-rectangle | MISS | MISS | PASS | PASS |
| classic/inherited-box | PASS | MISS | PASS | PASS |
| classic/clipped-to-page | MISS | MISS | MISS | PASS |
| classic/many-rectangles (secondary) | MISS | MISS | MISS | PASS |
| classic/content-array-split | PASS | MISS | MISS | MISS |
| classic/many-operands (primary) | MISS | MISS | PASS | PASS |
| paint/text-heavy-page | MISS | MISS | MISS | PASS |
| paint/text-mode-4 | MISS | MISS | PASS | PASS |
| paint/type3-d0-colour | PASS | MISS | PASS | MISS |
| paint/text-knockout-true | MISS | MISS | MISS | MISS |
| paint/text-knockout-false | MISS | MISS | MISS | PASS |
| paint/image-soft-mask | PASS | MISS | PASS | PASS |
| paint/image-direct-binary-halves | MISS | MISS | MISS | PASS |
| paint/image-direct-gray-quarter-turn | MISS | MISS | MISS | PASS |
| paint/image-direct-rgb-fractional | PASS | PASS | PASS | PASS |
| paint/image-direct-bilinear | MISS | MISS | PASS | PASS |

The complete machine-readable analysis retains all 38 cells' exact ratios,
upper/lower bounds and per-arm medians/maxima. For visibility, the remaining
eight RSS misses below the investigation trigger are text-heavy Exact
`1.0038461538461538`; content-array-split Exact `1.029940119760479` and
Competitive `1.0175438596491229`; many-rectangles Exact `1.0384615384615385`;
clipped-to-page Exact `1.005952380952381`; type3-d0-colour Competitive
`1.0153846153846153`; binary-halves Exact `1.0218579234972678`; and
text-knockout-false Exact `1.0298507462686568`. They remain failures of the
unchanged at-most-1.0 gate.

## RSS investigation and limits

Any observed max-RSS ratio above 1.05 requires evidence-based investigation
before partial retention. The four confirmation triggers are:

| Case / mode | Max RSS MiB baseline → candidate | Max ratio | RSS median ratio |
|---|---:|---:|---:|
| graphics-state / Exact | 2.578125 → 2.78125 | 1.0787878787878789 | 0.9810126582278481 |
| text-knockout-true / Competitive | 3.203125 → 3.40625 | 1.0634146341463415 | 0.9898477157360406 |
| text-knockout-true / Exact | 3.328125 → 3.59375 | 1.07981220657277 | 0.9898477157360406 |
| image-direct-gray-quarter-turn / Exact | 2.796875 → 2.96875 | 1.0614525139664805 | 0.9819277108433735 |

Lower central RSS with higher observed maxima motivates the separately frozen
same-binary investigation; it does not explain the maxima away. That diagnostic
must retain every extreme and cannot create an empirical noise allowance,
change a threshold, rerun the confirmation or manufacture a full-admission
claim. Its completed outcome and qualified independent audit are recorded below.

## Pilot concerns remain part of the record

The exploratory pilot passed 0/4 target latency, 8/34 protected latency and
34/38 RSS gates. Its primary median point ratios were
`0.9544214283520661` Exact and `0.9144734056815456` Competitive, without a
qualifying median upper bound below 1 in either mode.

Exact text-knockout-false had median ratio `1.0542373090496286`, p95 ratio
`1.045270119523699`, and p95 lower bound `1.0288187126800141`. The coordinator's
completed raw-pair review found all five candidate times higher. The fresh
confirmation did not reproduce the greater-than-2% lower-bound signal:
median/p95 lower bounds are `0.9980633619817169`/`0.9753641374566613`.
Its p95 upper bound still misses the protected gate at `1.0457387869604582`.
Neither result is deleted or characterized as proof of noise.

The pilot Competitive many-rectangles max-RSS ratio was
`1.0828220858895705`. The coordinator's completed raw review identified pair4:
candidate 5,783,552 versus baseline 5,341,184 bytes, alongside 3,325,583 versus
1,675,375 ns. The confirmation max-RSS ratio is `0.9819819819819819`, but the
pilot trigger still requires adjudication and remains in the A/A scope.

The pilot's other RSS misses remain recorded: type3-d0-colour Competitive
`1.0161290322580645`, text-knockout-true Competitive `1.0050761421319796`, and
graphics-state Exact `1.0063291139240507`. No pilot data were discarded or
pooled into confirmation. Its tiny five-pair tail estimates are exploratory.

## Completed same-binary investigation

One fixed diagnostic ran two warmup and 200 measured A/A pairs in seven cells
for each historical binary: 14 strata, 5,656 outputs. A and B use the identical
executable within each stratum. The run completed in 212.591 seconds; the strict
observer made 43 scans and recorded no known competing job. All outputs, raw
Darwin RSS fields, commands, journal entries and descriptive summaries passed
the independent revised audit. No sample was repeated or discarded.

| Cell / mode | Baseline same-binary larger/smaller label maximum | Candidate same-binary larger/smaller label maximum |
|---|---:|---:|
| classic/graphics-state / exact | 1.072727 | 1.006024 |
| paint/text-knockout-true / exact | 1.008889 | 1.009756 |
| paint/text-knockout-true / competitive | 1.019231 | 1.004587 |
| paint/image-direct-gray-quarter-turn / exact | 1.039548 | 1.005682 |
| classic/many-rectangles / competitive | 1.056235 | 1.038860 |
| classic/many-operands / exact | 1.008975 | 1.005085 |
| classic/many-operands / competitive | 1.012715 | 1.006780 |

The graphics-state and many-rectangles baseline strata show 7.27% and 5.62%
label-maximum differences with identical code. This establishes observed
variability, not the cause of any historical candidate spike. The other three
flagged cells do not show a comparably large label-maximum spread in this run.
Every prior RSS flag remains unresolved as a causal question; no allowance is
subtracted and no original failed gate is cleared. Lower central RSS does not
establish equal tails, and this diagnostic is not a second A/B confirmation.

The first audit failed because framework Python replaced the requested launcher's
`argv[0]` with its app executable. The revised auditor preserves every subsequent
argument and separately checks both executable paths/hashes. The actual app
executable was absent from the prelaunch closure: its pin is explicitly
retrospective. This qualifies provenance; current bytes do not prove its
historical bytes. The original auditor, failed receipt, corrected auditor,
eight added tests and exact same retained samples are preserved. See the
[launcher correction](sources/rectangle-reserve-rss/AUDIT-v2.md).

## Deterministic retained-capacity finding and decision

A separate serial diagnostic used the actual `pagura_raster::Segment` type
(56 bytes) with the pinned Rust 1.99.1 vector implementation. It models the
compiler's five pushes and `mem::take`; it does not render a PDF or measure RSS.
Source review establishes that `PathBuilder::finish` transfers this allocation
unchanged into the retained path. Both reset builders have capacity zero.

| Construction | Length | Baseline capacity | Candidate capacity | Payload bytes baseline → candidate |
|---|---:|---:|---:|---:|
| One rectangle | 5 | 8 | 5 | 448 → 280 |
| Rectangle then one line | 6 | 8 | 10 | 448 → 560 |
| Three rectangles before finishing | 15 | 16 | 20 | 896 → 1,120 |

The latter two are 25% increases in the segment backing allocation, not total
page memory or process RSS. The primary benchmark finishes each rectangle as
its own path and does not exercise these compound histories. Reservation can
also happen earlier before a later invalid point. Logical segment/work limits,
geometry and tested error state remain equal when allocation succeeds; no claim
of identical allocation/OOM timing or a global memory bound follows.

**Decision: preserve the measured patch and results, remove this candidate from
the active product stack, and revise it after reconciling main.** The known
retained-capacity cost is sufficient to reject retention under the frozen
resource clause, regardless of whether the process-RSS tails have a noise
component. No threshold or oracle is changed to keep it. Primary median benefits
remain useful; they are not added to historical stack or competitor ratios.

The narrow follow-up hypothesis is to reserve the original eight-slot eventual
capacity only for a genuinely fresh zero-capacity builder and retain the old
route otherwise. It may avoid the extra allocation while preserving successful
compound-path growth, but is unimplemented/unmeasured and still needs invalid
input, resource, native/WASI and independent output checks. Compact rectangle
storage is a separate, larger design. Neither proposal inherits this trial's
measured gain or memory results.

## Validation and retained failures

- Full `tools/check.py --jobs 1` passed on the exact candidate source, including
  230 renderer tests, 144 raster tests, 196 tooling tests, 1,249 conformance
  cases, lint/docs, and native/WASI host checks. Its final receipt and source
  boundaries agree; no known competing job was observed.
- Six targeted native tests and the same six WASI tests passed. They cover
  path state, transformed/degenerate rectangles, overflow prefixes and logical
  limit/error ordering. They do not prove a retained-capacity nonincrease.
- Preflight independently verified 76 outputs. The separate pilot verified
  760, confirmation 15,580, and same-binary diagnostic 5,656. All original
  source, binary, fixture and runner bindings remain unchanged.
- The diagnostic harness/auditor passed 13 bounded tests; the revised launcher
  check passed eight more. A first capacity-probe link failed because Apple
  LTO could not read Rust LLVM bitcode; adding Rust thin LTO produced the
  completed native probe. Both attempts and all audit failures remain recorded.

The original full check validates the candidate, not a future main integration.
The active stack returns exactly to the prior product fingerprint when this
patch is parked. Reconciliation of the known main sampler divergence and any
subsequent optimization requires fresh validation and new source identities.

[Complete results, failed gates and evidence hashes](rectangle-reserve-experiment.json),
[frozen plan](rectangle-reserve-plan.json), [original source/binary bindings](rectangle-reserve-bindings.json).
Raw reports, logs, images, modules, receipts and observer records remain under
`local/rectangle-reserve/`. Dedicated-runner, broad holdout, browser/WASM timing,
full mode unification and fair competitor release evidence remain outstanding.
