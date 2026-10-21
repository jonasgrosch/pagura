# Direct session-start to reconciled-current comparison

Status: execution plan; source, build and harness bindings pending. No samples
have been collected for this comparison.

No executable harness is added by this note. The coordinator must finish the
main merge and validation, review the final plan, bind every input and build,
and reserve serial compute before any measurement. The bounded rectangle draft
and parked color/rectangle candidates must remain absent from both arms.

## Identity and question

- Session implementation baseline: `359e19dda5b78a23675678b6a153da7ab7fdc2c5`
  (`docs/plans/unified-renderer-progress.md:3–5`).
- Available control: `/Users/jonasgrosch/.codex/worktrees/renderer-baseline/pagura`,
  `676a872eb9f6c279cbf1e3929ace3a0c99db9fc3`; its production crates, Cargo
  manifests/lock and toolchain have no diff from the implementation baseline.
  The first scene experiment pins this commit in the other worktree's
  `benchmarks/results/scene-index-final-confirmation-plan.json:7–11`.
- Candidate: exact committed reconciled source, hash still pending. Existing
  source identities and historical result files stay unchanged.
- Question: what does the complete retained stack change in accuracy, latency
  and process RSS on these exact workloads? This is not a worldwide ranking,
  release admission, or representative-document aggregate.

The old control has an untracked `scene_index_profile.rs`. Do not call that
checkout pristine or reuse its stale binary blindly. Freeze the same exact
example source in both arms, record this deliberate non-product harness overlay,
prove the rest of each source/build identity, then build both serially with
identical compiler, flags, dependencies and example code. Different product
binaries are intentional; the benchmark driver/example source must be identical.

## Fixed matrix and metrics

1. Keep all 46 cells from `opaque_composition.CELLS` (also inherited by
   `n16_output_u16`): opaque, sparse/dense glyph, clip, image, tiny, source-count,
   knockout, cold, first-tile and assembled-tile controls in both modes.
2. Keep all 38 analytic19 cold cells from `color_reuse.CELLS`, using
   `color_reuse_fixtures` and its existing independently pinned PDF/PPM records.
3. Namespace the two suites in case keys; retain **84 cells** without deduping
   apparently similar scenes or changing the matrices after seeing pixels.

Use the same public retained-list API on both sides: strict/full compilation,
page 0, 72 DPI, white background, requested Exact or Competitive. `cold` is internal
open+compile+render; `full`, `tile` and `tiled` use their declared render interval.
The filesystem is prehashed/warm. Record OS per-child peak RSS and stage fields;
do not compare Node/CLI startup, or call this cold-I/O evidence.

Proposed fixed trial: five warmup pairs then 200 fresh measured pairs per cell,
seed 20261114, randomized paired order declared before launch. This is 34,440
sample outputs, plus 168 all-cell preflight outputs. Root may change this
proposed budget before freezing; do not change it or retry after launch to seek
better intervals. No historical samples, pilot pooling, optional stopping,
cell removal, threshold changes or interim performance inspection.

## Independent quality qualification — baseline failures must remain visible

All 168 preflight outputs must be collected/classified before timing decisions.
For the 84-cell matrix require exact frozen RGB, expected dimensions and alpha 255
on each side; separately record paired raw-byte equality. Hash and retain failed
outputs and diagnostics as well as correct ones. No candidate output supplies
an oracle and no oracle is regenerated from either engine.

Classify every cell explicitly: both correct/equal; baseline incorrect/current
correct; baseline correct/current incorrect; neither correct; process or evidence
failure. A baseline-only mismatch is retained as an accuracy improvement and its
timing, if both processes finish, describes correction cost only. It cannot
qualify as an equivalent-output speed win. Do not quietly drop it, replace its
reference, normalize away alpha, or shrink the required matrix denominator.
Unexpected candidate quality failures block a successful current-stack claim;
completed quality preflight still records all cells. Evidence/lifecycle/source
failures abort and preserve an incomplete run rather than authorize a retry.

Add separately pinned, untimed accuracy-changing probes before timing: both
direct-root rational precision cases in both modes/full+tile (16 side outputs),
the declared-coverage/transparency tests and colored nonopaque-background cases
already used for the retained fixes. The analytic19 matrix itself includes
Competitive text-knockout-false, whose old wrong 63 versus independent 64 must not
be hidden. Use the existing rational/reference definitions and scoped model;
do not substitute an old engine oracle or claim continuous-scene perfection.

## Smallest reuse and required seams

- Reuse `scene_index_profile.rs`, fixture loaders, `expected_image`, source and
  harness identity helpers, `run_process`, raw artifact readers and
  `statistics.ratio_bounds`. Reuse the existing strict observer/final receipt
  chain and independent raw-output audit structure.
- `paired_render.run_experiment` currently aborts at the first wrong baseline
  pixel and enforces paired equality. It cannot run this protocol unchanged.
  Add an explicit opt-in quality-classification path with historical strict
  defaults unchanged, or a thin experiment-local collector that calls the same
  small helpers. Keep ordinary process failures distinct from invalid evidence.
  The current evaluator likewise requires strict outputs; update only through
  an explicit scoped mode, never silently weaken historical audit behavior.
- Avoid rewriting the growing full report after every pair: use the already
  reviewed append-only start/result journal pattern and final report, with
  final receipt pins covering both. This support change is frozen before this
  trial and has no product-performance meaning. Do not build a new framework.
- Recheck raw bytes and qualification independently for every warmup/sample;
  a preflight pass alone does not qualify later outputs. Preserve all failures
  and classify unavailable per-cell ratios explicitly.

## Reporting and decision

Report each suite/mode/operation/case separately: both side medians/p95, paired
ratio point estimates and one-sided 95% lower/upper bounds, peak-RSS ratio and
distribution intervals, quality status and every failure. Keep the existing
nominal bootstrap limitations, including p95 uncertainty and temporal dependence.
Do not form one geometric mean, multiply old gains, or count passed cells as a
percentage of the worldwide goal. If useful, retain the preexisting role/gate
labels within each suite (46-cell opaque targets and 38-cell preparation targets)
without interpreting their conjunction as the full release matrix.

The published clip, memo, guarded-output and u16 records are incremental pairs;
none measures the complete start-to-current stack. The shared opaque correction
has a known 2.731×/3.003× target pilot cost against its old route, so reporting
only later converter improvements would misrepresent the work. Fresh equivalent
output cells can establish a scoped net gain or regression; accuracy-changing
cells establish correction cost. Main reconciliation may change either and is
not covered by the previous competitor pilot at `c70b4678`.
