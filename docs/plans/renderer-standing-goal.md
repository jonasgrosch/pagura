# Standing renderer goal

Activated by the user on 2026-10-21: keep improving Pagura until the unified
renderer meets the existing independent accuracy and competitive performance
goals. The active task goal tracks continuation; this file makes the engineering
completion criteria and next steps reviewable. It is not a new release policy.

## Completion evidence

- Complete the shared architecture and outstanding work packages in the
  [unification plan](unified-renderer.md), with bounded work, memory accounting,
  unchanged required diagnostics and explicit fallbacks.
- Pass the applicable independent accuracy gates A0–A5, including the final
  per-channel bound, alpha/composition, adversarial cases, cache/tile invariance,
  and native/WASM equivalence. Fix known defects separately; baseline parity
  cannot bless them. A finite test set cannot establish perfection for all PDFs.
- Complete the existing release matrix and P2 gate: equivalent verified work,
  median and p95 one-sided 95% upper ratios at most 0.80 against each applicable
  required competitor, with no peak-memory regression and controlled runner
  evidence. Keep unsupported, failed and unverified outputs in the report.
  Comparative accuracy claims require independently adjudicated output, not
  majority agreement or using Pagura Exact as universal truth.
- Commit reproducible evidence and validation for each retained slice. Broader
  leadership claims remain limited to the measured versions, settings and
  workload coverage; do not shrink the matrix to finish the goal.

## Execution order

1. Glyph-hull reuse against `776fd4b9` is now a
   [validated experimental partial win](../../benchmarks/results/glyph-memo-experiment.md).
   Preserve its measured gains and investigate the disabled-path regressions
   with a separately frozen revision. Lazy initialization is an unmeasured
   candidate; it does not remove maximum inline storage or establish causality.
   [Retained stage analysis](../../benchmarks/results/glyph-memo-stage-analysis.md)
   locates most of the tiny-page cold excess before rendering. The completed
   [image profiles](../../benchmarks/results/glyph-memo-disabled-profile.md)
   show sampling/painting as the dominant work and do not identify memo
   initialization as the cause. Keep lazy initialization unapplied until an
   isolated experiment justifies its complexity; protected failures stay open.
2. Use measured reach and cost to select the next small improvement. Preserve
   useful partial wins on the experimental branch; distinguish incomplete
   admission from evidence of regression. Never add historical percentages.
   The [plain opaque composition gap](opaque-composition-accuracy.md) is now
   independently reproduced and corrected by shared composition, with measured
   performance costs retained explicitly. Recover those costs through separate
   [measured slices](n16-next-slices.md), preserving the strict accuracy bounds
   and newly corrected nonopaque-background behavior.
3. Extend shared scene/clip/image preparation where profiles justify it, while
   closing independent reference coverage and renderer memory-accounting gaps.
4. Complete persistent/region benchmark adapters and matrix aggregation, then
   run fair native/WASM competitor comparisons and an untouched holdout.

Only the coordinator launches local builds, tests or benchmarks, serially.
Agents may review or draft independently. Check for foreign compute before each
heavy launch; cooperative locks are not evidence that unrelated jobs are paused.
Freeze each experiment before measurement and retain failed/incomplete attempts.
An inconclusive result does not authorize retries until a passing sample appears.

The goal remains active across partial deliveries. Missing evidence means work
remains; useful partial gains are neither a completed release claim nor discarded
solely for missing a full admission bound.
