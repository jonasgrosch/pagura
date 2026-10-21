# Unified renderer: paused status

2026-10-21. Pause after the completed F32 confirmation, audit and local commit,
as requested by the user. Do not start another optimization, benchmark, rebase
or automatic continuation until the user resumes the work.

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

The last direct competitor pilot predates the latest slices. On 13 independently
matching cases, Pagura/Hayro median case time ratio was 1.203. Reaching the
declared 0.80 ratio implied about 33.5% less time at that older baseline, with
individual needs of 25-66%. These are stale directional gaps, not current-stack
measurements. There is no current worldwide speed or accuracy ranking and no
credible completion percentage or delivery date.

[Prior competitor measurements](../../benchmarks/results/analytic19-conversion.md)
and [implementation history](unified-renderer-progress.md).

## Resume point

1. Reconcile the retained stack with latest main, preserving the accuracy and
   resource corrections, then validate that integrated product.
2. Measure the net session-start/final stack and refresh fair competitor
   comparisons under identical workloads and independent references.
3. Resolve protected tail/cold regressions and RSS misses before admission.
4. Profile the integrated product. The prepared page-surface reuse proposal
   remains an unimplemented hypothesis; preserve background, chunk, error and
   resource behavior if testing it.
5. Continue the remaining shared rendering policies and expand independent
   accuracy evidence, representative documents and native/WASM/platform coverage.

No further work is scheduled by this report.
