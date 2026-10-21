# F32 painted-output interval experiment

Status: implementation and validation in progress; no incremental timing yet.
Baseline is retained commit `f9738174757252567730ff62b5d36620c01d1e6c`, in the
pristine `renderer-f32-interval-base` worktree. Candidate extends its existing
N16 interval to F32. The [fresh profile](../../benchmarks/results/f32-post-interval-profile.md)
motivates this work; it supplies no comparative speed estimate.

## Implementation and accuracy boundary

N16 and F32 share one `Storage<T>` containing their existing pixel vector and a
conservative painted range. Validated spans and fragment rows extend the range.
Image, shading and shading-pattern operations mark the full F32 buffer once,
before their existing pixel loops. Preparation is infallible and a no-op for
other formats; existing pixel validation retains its original error ordering.
There is no tracking work per dynamic pixel.

Only page output may omit the range's exterior: the destination already holds
the same opaque background. Dense output, eager initialization, arithmetic,
source evaluation, allocation and work charges remain unchanged. Nonopaque
backgrounds and other existing admission exclusions keep F64. This implements
the shared-execution direction of ADR 0016 without changing either accuracy
policy or claiming that their raster geometry is equivalent.

Reject on any independent pixel/alpha, tile, precision, source-error, resource
accounting or enclosing-layout regression. Verify empty and distant ranges,
holes, failed spans, a late clip-row failure, dynamic partial failures, arbitrary
dense destinations, colored backdrops, irregular tiles and recycled storage.
Native, WASI SIMD and WASI scalar tests must pass. Compare actual Buffer,
Option<Buffer>, Surface, MaskSink and Context size/alignment and ordered
error/resource transcripts between arms. Smaller enclosing layouts are allowed;
growth is not. No extra heap or changed payload charge is allowed. Layout
success does not establish process RSS neutrality.

## Fixed incremental benchmark

Preserve all 52 prior cells and 18 cases. Only `sparse64/full` in Exact and
Competitive become targets; the previous `opaque64/full` targets become
protected. Add dense constant-black quarter-alpha shading and shading-pattern
full renders in both modes: **20 cases, 56 cells, 2 targets, 54 protected**.
Existing cold, tile, tiled, tiny, empty/distant, dense, nested/knockout, clipping
and image controls remain. Never remove a cell after inspecting results.

The new references are constant RGB191, alpha255, independently from
255*(1-1/4)=191.25 over white. Their constant Type2 functions eliminate
sample-position uncertainty. Existing `image-only` supplies dense F32 image
coverage. Direct shading should use F32 in both modes; shading-pattern Exact
must retain F64, Competitive should use F32. Verify those routes untimed rather
than inferring them from case names. Preserve failed reach predictions and
restore all temporary instrumentation before building timed binaries.

One pilot has 5 measured and 5 warmup pairs per cell, seed20261109, for 1,120
verified outputs. One fresh confirmation has 200 measured and 5 warmup pairs,
seed20261110, for 22,960 outputs. A launched failure consumes that phase; an
environmental recovery needs a separately named explicit replan. No pooling of
historical, pilot, canceled or other-platform samples, optional stopping or
automatic retry. The pilot cannot admit the change.

The unchanged harness shuffles cells, then each pair's AB/BA order with the
phase RNG. Cold cells use total process duration; other operations use their
reported render stage. Bootstrap uses all 200 paired samples, 5,000 resamples,
one-sided 95% bounds and analysis seed42017.

| Gate | Required confirmation result |
|---|---|
| Target median | Ratio upper bound <=0.95 |
| Target p95 | Ratio upper bound <=1.00 |
| Protected median and p95 | Both upper bounds <=1.02 |
| Peak RSS | Candidate maximum measured peak / baseline maximum <=1.00 in every cell |
| Accuracy | Independent RGB equality, alpha255 and same-mode baseline RGBA equality for every output, including warmups |

Preserve absolute RSS minima, medians, p95 and maxima and every missed gate.
Maximum RSS ratios have no confidence bounds. Marginal bootstrap bounds do not
establish simultaneous coverage. An upper-bound miss is neither proof of a
regression nor a passing noninferiority result.

## Execution prerequisites and decisions

Before timing, complete repository validation, native/WASI SIMD/scalar accuracy,
source/error/resource/tile checks, layout comparisons and route evidence. Pin
separate bounded WASI diagnostic settings before running them; never infer WASM
speed from native samples. Native preflight must verify all 112 arm/cell outputs.
Collect ordinary process/pixel failures over the complete preflight matrix;
evidence or owned-process cleanup failures abort and block timing.

Freeze the exact plan, candidate and baseline source/binary identities, identical
example, toolchain/build settings, helper/fixture closure and prerequisites.
Retain exclusive phase launch intents and inspect the direct
runner -> strict guard -> driver -> report chain. Final receipts must bind
reports, raw artifacts, guards and launch inputs and show unchanged source.
Use the existing single-job runner and quiet-slot monitor; no competing owned
compute or termination of foreign jobs. Five-second polling cannot detect every
short-lived foreign job.

Full experiment admission requires all 56 latency/RSS gates and all mandatory
quality, provenance and portability checks. Partial experimental retention
requires at least one original target's confirmation median upper bound <=0.95
and every mandatory quality/provenance/portability check. Disclose target tails,
all protected misses and all RSS misses. A protected median or p95 lower bound
above1.02 requires revision or an explicit reasoned experimental restriction.
Retention is a reviewed decision, never an evaluator side effect. Reject or
revise a completed confirmation without the required target median improvement;
preserve its evidence and patch.

The native scope remains one thread, warmed filesystem, default Repair opening,
full compilation and opaque-white analytic references. The timed example does
not emit document diagnostics; source/error gates are separate. No overall-stack
percentage, all-PDF accuracy proof, competitor superiority, main/default
promotion or release admission follows from this experiment.
