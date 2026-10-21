# Current-stack comparison: the remaining competitor gap

2026-10-21. The fixed pilot identifies **Hayro as the clearest remaining speed
and memory target in this selected matrix**. Where both engines matched the
independent pixels, Pagura used 1.07–2.37 times Hayro's open-plus-render time.
The largest case also used 2.89 times its peak process RSS. This is useful
directional evidence, not a calibrated tail result or a worldwide ranking.

Product baseline: validated integration `c70b4678`. Measurement/plan commit:
`7cea2a8c`. Exact/default policy, 19 single-page analytic cases, 72 DPI, six
competitors, two warmup pairs and five measured pairs, fixed seed 20261021.
One campaign completed all **1,596 attempts** in its 161-second outer window,
including the release build and preflight. No retry or extra measurements.
The observer made 31 scans and recorded no known competing jobs.

## Accuracy and evidence

All **798 Pagura outputs matched all 19 frozen RGB references**. All engine
processes were available; the independent audit classified 1,309 outputs correct
and 287 incorrect under that policy, with no evidence-binding errors. The 287
mismatches are 41 competitor/case combinations repeated seven times. Every
warmup and measured output was retained and audited. The audit exits 1 for those
policy mismatches; its wrapper's `failed` status is preserved rather than
rewritten as a successful all-engine quality check.

The references are independent analytic pixels, including four separately bound
image-model profiles. Some image resampling and antialiasing choices can differ
legitimately between engines. A mismatch here does **not** automatically establish
a PDF-standard violation or prove Pagura globally more accurate. PPM outputs do
not preserve alpha, and this campaign cannot establish independent alpha
superiority. The prior continuous-scene coverage limitations also remain.

## Speed and memory

Ratios are **Pagura / competitor**, lower is better. A cell qualifies for ratios
only when both engines match the reference on every warmup and measured output.
All other cells and their complete five-pair summaries remain in the
[machine-readable result](analytic19-conversion.json); the bound local conversion
report retains the individual attempts. Subsets differ by engine;
their summary medians must not be used as a common-workload ranking.

| Competitor | Matching cases / 19 | Median of case open+render ratios | Range of case ratios | Largest process RSS ratio |
| --- | ---: | ---: | ---: | ---: |
| PDFium | 10 | 0.456 | 0.047–1.023 | 0.547 |
| MuPDF | 12 | 0.169 | 0.035–1.883 | 0.399 |
| PDF.js | 16 | 0.0033 | 0.0024–0.0975 | 0.110 |
| Hayro | 13 | 1.203 | 1.072–2.374 | 2.894 |
| Quartz | 11 | 0.0347 | 0.0301–0.9304 | 3.793 |
| Poppler | 11 | unavailable | unavailable | 2.937 |

Poppler lacks an internal timer in this adapter. Its median of case **process**
ratios was 0.338. Process time includes runtime startup and teardown; process RSS
includes the runtime and adapter. Engine time excludes most imports/startup but
includes page loading, compilation, lazy resources, decoding and allocation.
PDF.js is a fresh process/document for every attempt, not a warmed browser/viewer.
Successful original diagnostic streams are not retained, so the audit checks
authenticated stage fields and their arithmetic rather than reparsing those logs.

Eight classic cases match across all six competitors. On those identical cases,
the median case open+render ratio against Hayro is 1.193. Its lead is therefore
not explained merely by the different per-engine matching subsets.

The important matching-case gaps are:

| Case / competitor | Pagura open+render | Competitor open+render | Time ratio | Peak process RSS ratio |
| --- | ---: | ---: | ---: | ---: |
| Many operands / Hayro | 17.513 ms | 7.377 ms | 2.374 | 2.894 |
| Many rectangles / Hayro | 1.299 ms | 0.906 ms | 1.434 | 1.552 |
| Text-heavy page / Hayro | 0.559 ms | 0.373 ms | 1.499 | 1.183 |
| Text clipping / Hayro | 0.193 ms | 0.119 ms | 1.627 | 1.025 |
| Many operands / MuPDF | 15.378 ms | 8.168 ms | 1.883 | 0.399 |

Against Hayro's observed values, reaching the existing 0.80 time-ratio target
would require roughly **25–66% less Pagura engine time** across these 13 matching
cases; the median case-ratio estimate implies about 33.5%. These are arithmetic
targets from five-pair point estimates, not confidence bounds or a forecast.
No Hayro-matching cell meets the 0.80 median/p95 point targets. Five of those
13 cases also exceed its sampled maximum RSS. The memory gate remains unchanged.

## Next implementation priorities

1. Profile the many-operands case including preparation. It paints 16,384 small
   rectangles; its pending operand vector is reused and never exceeds four
   values. Source review instead identifies per-mark color and path allocation
   as hypotheses to investigate. An already-compiled scene loop would omit this
   preparation. Preserve malformed-input handling, source positions, work budgets
   and error order; source inspection alone does not establish the bottleneck.
2. Continue the independently planned N16 painted-interval experiment on this
   validated baseline. Preserve all 46 existing protected/target cells and the
   six new controls; do not infer a gain on operand-heavy work from that profile.
3. Extend beyond these analytic pages to separately defined warm-document,
   representative-document and retained-memory operations, with independent
   quality/adjudication gates. This pilot implements none of those missing tracks.

The full goal remains open: complete accuracy coverage, median and p95 ratios
at or below 0.80 against every required engine, no memory regression, and the
full workload/platform/runner matrix. Prior self-comparison partial wins stay
retained; this campaign does not measure their net combined contribution.

[Frozen plan](analytic19-conversion-plan.json),
[sources and stage boundaries](sources/analytic19-conversion/README.md), and
[all cell summaries and finalized bindings](analytic19-conversion.json) preserve
the protocol, preflight tests, exact engine versions and evidence limits.
Raw reports, outputs and receipts remain under `local/analytic19-conversion/` and
the artifact store named in its bound conversion report.
