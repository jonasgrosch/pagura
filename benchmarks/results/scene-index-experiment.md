# Lazy root glyph bounds experiment

**Status: memo removed without performance admission.**
The candidate at `9ebac1cb` passed the complete repository check on 2026-10-21.
Final confirmation005 refused external compute contention before launching a
benchmark child and produced zero measured samples. The keep-only-if-all-gates-pass
condition therefore required removal; no numerical speed or memory regression
was established. The [decision record](scene-index-decision.json) binds the
final refusal evidence. The original direct hull calculation is restored.
The [machine-readable record](scene-index-experiment.json) preserves the pilot
metrics, evidence hashes, invalid-run chronology and replacement protocol.

## What was tested

The historical `compositor/index.rs` lazily reused the existing glyph-run device
hull across bands/chunks. It covered root command ordinals 0–63 with at least two
glyph sources, repeated regional execution and finite canonical-device bounds.
Both compositor policies called the same code. Painter order, all command
visits, transfer validation, semantic work charges and error positions stayed
unchanged. `regions.rs` remained the visibility-padding and pixel-rounding owner.

This removed repeated source-hull calculations; it did **not** implement sparse
command dispatch. Groups, including ordinary text knockout groups, were outside
its eligibility. Exact's separate opaque renderer also bypassed the memo.
The index object occupied 2,064 bytes on the measured native target, including
when disabled, and allocated no heap storage. This was object size, not measured
total stack growth, and did not waive the peak-memory gate. The object and reuse
path are no longer present in the product.

The simpler original direct hull calculation has been restored. No cache
extension or scheduler is admitted by this experiment.

## Independent checks and measured scope

The benchmark generator independently writes rectangular TrueType glyphs and
PDF content. Integer-aligned black glyphs at quarter opacity over opaque white
have the exact RGB8 result `round(255 × 0.75) = 191`. The expected image is
written directly as rectangles; it is not produced by Pagura. Every measured
output and warmup must equal that reference, have opaque alpha, and be
byte-identical to the unchanged renderer in the same mode.

The favorable target is explicit `/TK false` text: 64 direct root glyph runs,
each with 64 distinct glyph sources, on a 2048 × 2048 page at 72 dpi. A separate
public-API debug probe confirmed 64 runs, 4,096 sources and no group wrappers.
Its `root_commands` output label was incorrect: `DisplayList::len()` counts
painted primitives. The corrected interpretation is retained locally.

The 24 predeclared cells cover both modes: target full rendering; protected
cold, first-tile and complete tiled operations; dense, tiny, opaque, single-source,
over-capacity, nested and default-knockout controls. `full` is the first render
of a newly compiled retained list, with index construction included. `cold`
includes open, compile and render after reading the file. Input I/O, output
serialization and verification are outside those timers; the filesystem is
warm. This is a native synthetic experiment, not a host, WASM or competitor
comparison.

A separate half-opacity preflight exposed existing mode rounding: the exact
reference is 128, while Competitive returned 127. Those artifacts are retained.
Quarter opacity supplies a common exact reference without relaxing tolerance.
This candidate changes neither mode's accuracy policy.

## Results and stopping decision

The five-pair pilot, with five warmups per cell, completed with all 480 outputs
independently correct and baseline-byte-identical. Target medians were:

| Mode | Baseline | Candidate | Candidate/baseline |
|---|---:|---:|---:|
| Exact | 9.609 ms | 7.853 ms | 0.8172 |
| Competitive | 1.823 ms | 1.491 ms | 0.8176 |

These are exploratory observations, not accepted gains. Exact's target median
and p95 upper bounds were 1.139; only one of 22 protected cells met both latency
bounds. Eight cells exceeded the raw peak-RSS ratio limit. A large RSS ratio
fails the policy gate but does not identify its cause. Ordinary desktop activity
and repeated untimed evidence I/O can contribute noise.

The frozen confirmation called for 200 fresh pairs and five warmups per cell.
An unrelated `friendly-rosalind` full check began at
**2026-10-20 22:28:43 UTC**, after the pilot ended at 22:28:00.941 UTC.
Confirmation001 began at 22:31:04.439 UTC. On detecting the concurrent job,
the coordinator sent SIGTERM only to its own runner; it ended at
22:33:22.690 UTC with exit 143. The receipt reports cancellation and unchanged
source. The entire partial confirmation is invalid. Its timings have not been
inspected, summarized or pooled.

A separately reviewed replacement protocol permits one fresh fixed run with
unchanged code, cells, references, thresholds and analysis, order seed 982451,
and active exclusion monitoring. It requires an enforced compute window first.
The replacement preflight at **2026-10-20 22:45:32.875 UTC** detected a new
`practical-gould` release build and refused to launch. No confirmation-002
samples exist. Performance admission was therefore inconclusive at that stage;
that protocol authorized no third automatic attempt. No partial data enters analysis.

The local contention guard passed controlled checks for successful execution,
preflight refusal, live cancellation and deadline expiry. Each cancellation
affected only its owned child; the simulated foreign job stayed alive. A
separate first guard test also refused an actual profiling job. The guard polls
known jobs every five seconds and cannot prove OS-level exclusivity.

## Validation and evidence

- Twelve focused tests passed: storage formats, repeated regions, work thresholds,
  exact error context, nested/view isolation, changed transforms and large-origin
  numeric-limit behavior.
- `cargo clippy --locked --offline -p pagura-render --all-targets -j1 -- -D warnings`
  passed.
- The complete `python3 tools/check.py --jobs 1` passed, including all 182 Python
  tests and 1,237 generated-PDF conformance cases. The previously refused
  preflight is retained at `local/scene-index/full-check-001-guard/guard.json`.
  The successful check used reduced priority after the user said to proceed;
  it is correctness evidence, not exclusive performance evidence.
- Baseline: `676a872eb9f6c279cbf1e3929ace3a0c99db9fc3`; source, harness and binary
  hashes are pinned in the pilot and frozen confirmation plan.
- Local durable evidence: `local/scene-index/pilot-001/report.json`,
  `local/scene-index/pilot-001-receipt.json`,
  `local/scene-index/confirmation-001/report.json`, and
  `local/scene-index/confirmation-001-receipt.json`,
  `local/scene-index/confirmation-002-receipt.json`, and
  `local/scene-index/confirmation-002-guard/guard.json`, including raw outputs,
  references, inputs, warmups, process diagnostics and sample timings.

The [frozen plan](scene-index-confirmation-plan.json) is retained unchanged.
Its sole-confirmation identifier is superseded only for the documented external
validity failure, as recorded in the experiment JSON. The pilot cannot admit the
candidate, and correctness checks alone do not satisfy performance acceptance.

## User-authorized resumption (2026-10-21)

The user subsequently confirmed that all other Pagura compute was paused.
The [resume plan](scene-index-resume-plan.json) froze the same code, binaries,
24 cells, 200 pairs, five warmups, seed 982451 and thresholds before launching
any measured process. An independently reviewed
[scheduling clarification](scene-index-scheduling-clarification.json) treats
zero-child preflight refusals as retained scheduling events, without exposing
performance outcomes; the opportunity is consumed on the first child launch.

Preflight003 refused an active `practical-gould` release test without launching
a child. After three quiet snapshots, confirmation004 launched at
**2026-10-20 22:51:48.025 UTC**. `practical-gould` then started a new release
build at **22:52:51 UTC**. The active guard detected it and stopped its own
benchmark child; cleanup completed at **22:52:56.056 UTC**, exit 143.

The entire partial run is invalid. No timings from it have been inspected,
analyzed or pooled. There was no automatic measured restart. At that point
full validation was held and the candidate remained uncommitted. Guard, runner
and report paths
and hashes are recorded in the experiment JSON.

## Completed correctness validation (2026-10-21)

After the user instructed “just go ahead”, the complete check ran at reduced
priority with one Cargo build worker, one Rust test worker and one conformance
worker. Other Pagura work was active, so no timing or exclusivity claim follows
from this run. All required checks passed: policy, fixtures, bundled data,
formatting, workspace Clippy/tests, doc tests/documentation, native/WASM builds,
host integration, 182 Python tests and all 1,237 PDF conformance cases. The
12 new bounds-reuse regressions passed as part of the full suite.

Duration: 965.647 seconds. The runner recorded both exit
codes as zero and identical source identities before/after validation.

- Source SHA-256: `44dbd6cee7134c2304dbd51b73291d46a7ff630bd0937c02d71c4dfb38bad91f`.
- Log: `local/scene-index/full-check-002.log`, SHA-256
  `24dc9734beaebfb1c46e0c433bb63dec5eb3ca64cec831fed8e0ef2ba9017283`.
- Receipt: `local/scene-index/full-check-002-receipt.json`, SHA-256
  `1ab4d04110f9430aa7e45abf09fc75f7428fd31bc782cf7c96c1dd0ef7d10f7a`.

The correctness-validated isolated experiment was committed as `9ebac1cb`.
That validation did not satisfy P1 or admit an accepted speedup.

## Final confirmation and removal (2026-10-21)

The user explicitly requested confirmation and retention only if every gate
passed. The [final frozen plan](scene-index-final-confirmation-plan.json)
preserved the 24 cells, 200 fresh pairs, five warmups and thresholds, with order
seed 20261021. After bounded scheduling, confirmation005's final preflight at
**2026-10-20 23:31:15 UTC** refused external compute contention with exit 75.
The coordination wrapper launched the guard; the guard launched **zero benchmark
children**, so there are **zero measured samples** and no confirmation005 report.

Under keep-only-if-all-gates-pass, absence of valid complete confirmation could
not authorize retaining the memo. Its product code and fixed storage have been
removed. This decision is performance non-admission, not a numerical speed or
memory regression or a correctness finding. Earlier partial timing data remains
excluded without analysis or pooling. The prior pilot and full-check history,
frozen plans and candidate binary remain retained; paths and hashes are bound
by the [decision record](scene-index-decision.json).

The subsequent viewport-construction candidate moved clip viewport preparation
into the soft-mask-source branch. It is preserved outside the renderer after
a subsequent measured confirmation missed the complete gates under background load. See its
[separate evidence record](clip-viewport-experiment.md) for the independent pixel
preflight, source patch and static code-generation observations. Its evidence
must be assessed independently of the removed memo.
