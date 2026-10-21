# Unified renderer: paused status

Latest handoff: [ADR 0033](../adr/0033-prepared-renderer-and-session-handoff.md)
records the proposed prepared-renderer redesign, current-main migration,
publication inventory and new-session instructions. The historical checkpoint
below remains evidence for its pinned revisions; it is not the new execution
queue. Optimization remains paused pending a new implementation request.

2026-10-21. Optimization remains paused. The user subsequently authorized a
comparison of the fixed combined stack with Hayro, including accuracy and
memory, with no optimization. That comparison is complete and independently
audited. Do not start further optimization or a rebase until requested.

## Latest retained improvement

The F32 painted interval skips final page output conversion outside the
conservative interval that received stores. N16 and F32 share interval storage;
dynamic image, color and shading operations mark their full interval once.
Pixel arithmetic, dense conversion, background initialization, memory charges
and error order remain unchanged. No dependency or unsafe code was added.

The baseline is the retained N16 commit `f9738174`; the measured candidate
product hash is
`4d6f7ab3f68500009aa938b1e3680d8c26a6fc052f706dd3a4ce5ad697b94a8e`.
This experiment was kept fixed while upstream progressed. It is not based on
latest main: the cached upstream ref at wrap-up was `cc36eee8`, with common
ancestor `d81d0c82`. No fetch or upstream integration was performed during
the fixed measurement.

| Sparse F32 full-render target | Baseline median | Candidate median | Less time | Sampled maximum RSS |
| --- | ---: | ---: | ---: | ---: |
| Exact | 7.668646 ms | 3.833896 ms | 50.0% | 31.031 to 33.078 MiB, +6.6% |
| Competitive | 6.825750 ms | 3.067417 ms | 55.1% | 33.828 to 29.531 MiB, -12.7% |

Both target median and p95 gates pass. Full admission fails:

- Protected latency: 17/54 pass; 37 miss.
- Sampled maximum RSS: 30/56 pass; 26 miss.
- Competitive tiny cold median rises 7.48%, with lower ratio bound 1.0571.
- Competitive clipped-page p95 rises 22.51%, with lower ratio bound 1.0430.
- The largest control RSS increase is 25.32%.

Those two latency controls have positive regression evidence under the declared
2% material threshold. Other upper-bound misses need not establish a regression,
but none are silently waived. Layout equality does not explain RSS changes.
Retain the useful partial win on this experimental branch; no main/default/
release promotion is justified.

[Complete results and every cell](../../benchmarks/results/f32-painted-interval-continuation-experiment.md),
[machine-readable evidence](../../benchmarks/results/f32-painted-interval-continuation-experiment.json),
and [final publication receipt bindings](../../benchmarks/results/f32-painted-interval-retention.json).

## Validation and preservation

The full repository check passed, including all 1,249 conformance cases,
native/WASM and host checks. Separate six-arm diagnostics verified unchanged
resource/error records and no growth in the checked layouts. Independent
native preflight, dynamic-route checks, 336 WASI SIMD outputs and 24 scalar
outputs passed. WASI measurements do not establish calibrated speed or RSS.

The final independent audit verified all 22,960 scheduled output records across
20 fixtures and 56 cells against regenerated expected RGB, opaque alpha and
same-policy baseline bytes. There were 200 measured pairs and five warmups per
cell. All 3,310 complete pairs from the interrupted original run remain
unchanged; the continuation supplied the remaining 8,170. One incomplete
terminal attempt is retained separately and its pair was executed in full.

The original guard stopped on another process being present. It recorded no
CPU consumption, so this is not proof of contamination or proof of zero impact.
The user explicitly amended the method to preserve completed pairs. Original
failed receipts and artifacts remain untouched. The continuation guard and
independent audit passed. The new segment used a flushed journal instead of
rewriting the growing report after each pair. Both the time boundary and
checkpoint change are explicit method differences. Marginal bootstrap bounds
come from the pinned producer; the independent auditor rechecks raw evidence,
counts and point estimates, not those bootstrap calculations.

Raw original and continuation evidence remains under the ignored
`benchmarks/results/local/f32-painted-interval/` and
`benchmarks/results/local/f32-painted-interval-continuation/` directories.
The commit contains source, plans, result tables and evidence hashes; it does
not embed the large local raw archives.

## Where the overall goal stands

There are real targeted gains, but the fastest-and-most-accurate goal is not
achieved. The preceding N16 interval slice reduced its opaque target time by
29.38% Exact and 37.04% Competitive. Earlier glyph reuse and exact output
conversion improvements remain in the stack. These experiments have different
baselines and workloads: do not add or multiply their percentages. A direct
session-start versus final-stack comparison is still missing.

Accuracy work corrected real composition defects, rather than merely matching
the old engine: independently derived opaque composition results, premultiplied
nonopaque backgrounds, and Competitive translucent text (expected channel 64
instead of the previous 63). Resource integration also repaired glyph storage
accounting and a WASI overflow probe. This is finite evidence for the corrected
cases, not a proof of perfect rendering for all PDFs.

The composition path is more shared, including high-precision composition and
output handling. The modes still differ in geometry, image/font validation and
shading policies. Full mode unification is unfinished.

The completed [current-stack comparison](../../benchmarks/results/current-stack-hayro.md)
measured the fixed product at `402e3afe` against a fresh standalone Hayro 0.8.0
release build. It retained the prior 19-case panel and added all 20 interval
fixtures, in both modes: 78 cells, 100 measured pairs plus five warmups, and
16,380 output attempts. Both builds and every renderer execution ran serially
under the owned runner and observer. The independent audit rechecked the exact
schedule, input/output pixels, full diagnostic timing records and per-child RSS.

| Panel | Mode | Pagura/Hayro median case time ratio | Both match reference | Peak RSS no larger |
| --- | --- | ---: | ---: | ---: |
| Prior 19 cases | Exact | 1.1543 | 13 | 8/13 |
| Prior 19 cases | Competitive | 1.1842 | 13 | 8/13 |
| Added 20 cases | Exact | 2.0216 | 20 | 1/20 |
| Added 20 cases | Competitive | 1.6447 | 20 | 1/20 |

Ratios are Pagura/Hayro open-plus-render time, including page preparation;
lower is faster. The case-ratio medians are unweighted, cover mutually correct
cases only, and must not be treated as a representative-document global score.
Every wrong-output cell remains in the full result with unqualified descriptive
times. There is no 20% median-and-p95 bound pass in any qualified cell. Exact
wins none of its 33 qualified median comparisons; Competitive wins one of 33.

Exact matched all 39 reference images. Competitive matched 38: the fractional
RGB image differed by one level in 64 channel values. Hayro matched 33.
All 16,380 attempts executed without a process failure. These finite RGB-policy
checks do not establish arbitrary-PDF correctness, a PDF-standard violation
from every mismatch, or alpha superiority.

The original panel's Exact median case ratio moves from the older pilot's
1.203 to 1.1543. This is a fresh comparative result, not an isolated causal
estimate of the stack's speedup: runs, binary builds and the adapter differ.
The newer, larger sample is authoritative for this fixed current comparison.

Material current gaps include:

- Many operands: Exact 16.046 ms versus Hayro 6.804 ms (2.358x);
  Competitive 19.547 versus 6.807 ms (2.872x).
- Sparse target after the retained optimizations: Exact 4.280 versus 3.018 ms
  (1.418x); Competitive 3.471 versus 2.928 ms (1.185x).
- Constant dense shading and shading-pattern cases: approximately 54-63x
  Hayro's engine time. The worst Exact pattern case is 21.750 versus 0.346 ms;
  both outputs match the independent constant-color reference.
- Exact's worst sampled peak-RSS ratio is 4.818x on that pattern case;
  Competitive's worst is 4.039x. RSS includes the whole process and emission,
  not renderer-only heap. Only 9 of 33 qualified cells per mode have maximum
  RSS no larger than Hayro.

The strong incremental self-comparison gains remain useful, but they have not
produced dominant comparative speed or memory. The overall goal is still open.
No renderer optimization was made during this comparison.

[Current evidence bindings](../../benchmarks/results/current-stack-hayro-evidence.json),
[prior comparison](../../benchmarks/results/analytic19-conversion.md), and
[implementation history](unified-renderer-progress.md).

## Resume point

1. Reconcile the retained stack with latest main, preserving the accuracy and
   resource corrections, then validate that integrated product.
2. Preserve the completed current-stack/Hayro comparison. A direct session-start
   comparison, representative documents and wider platform coverage remain open.
3. Resolve protected tail/cold regressions and RSS misses before admission.
4. Profile the integrated product. The prepared page-surface reuse proposal
   remains an unimplemented hypothesis; preserve background, chunk, error and
   resource behavior if testing it.
5. Continue the remaining shared rendering policies and expand independent
   accuracy evidence, representative documents and native/WASM/platform coverage.

No further work is scheduled by this report. Optimization remains paused.
