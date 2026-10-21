# Retained glyph-memo stage analysis

Exploratory description of the completed confirmation only. All 200 measured
pairs per listed cell are included; the five warmup pairs are excluded. No
resampling, confidence intervals, tests, builds, renders or new benchmark runs.
Nothing below changes the original admission decision or its estimand.

## Bindings and timing meaning

- Current checkout: `d16c07f8ea7ae6cc5e680cd8b669640be19e5f12`.
- Report: `/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura/benchmarks/results/local/glyph-memo/confirmation-001/report.json`.
- Report SHA-256: `0e26445deb7cecefc589cac8ac5c15b198816063be0b6672f7ffcd21ceaca6d1`.
- Measured control source: `735cef328e06f63dd4e3c0e5f452d949874275f221640a6aac1c31e7471dee33`.
- Measured candidate source: `6f89444e8688e0174abcaf427195a35e8ad2cc17e82d163146f716f49cd27c29`.
- Measured profile example: `8e89a087d351e7b89289e0897cafeb75422d4c151280734fde8c958a841279f2`.

`glyph-memo-stage-analysis.json` retains measured binary/harness identities, all stage summaries,
order strata, fixed temporal quartiles and descriptive process counters.
Source and binary identities above are the retained measurement identities,
not an assertion that the later committed checkout was itself benchmarked.

The public-API example times open, compile and render separately. `full` uses
render time; `cold` uses their enclosing total. Both operations still execute
open/compile/render once in a fresh process; the render path is identical.
Input-file reading and output emission are excluded. These are warmed-filesystem
measurements, not cold I/O. Medians of stages or paired differences do not add
up to a median total; paired means do, apart from the small timing gaps.

## Stage medians

All values are microseconds, control → memo. The final ratio is the original
ratio of metric medians, not the median of pairwise ratios.

| Case / mode / operation | Open | Compile | Render | Metric ratio |
| --- | ---: | ---: | ---: | ---: |
| Image / Exact / full | 24.834 → 26.479 | 52.667 → 55.146 | 4263.688 → 4683.771 | 1.0985 |
| Tiny / Competitive / cold | 25.354 → 28.771 | 168.708 → 174.479 | 34.938 → 36.333 | 1.0445 |
| Tiny / Competitive / full | 27.438 → 27.708 | 174.313 → 174.583 | 35.146 → 35.896 | 1.0213 |
| Tiny / Exact / full | 27.250 → 27.542 | 172.646 → 174.021 | 52.396 → 52.917 | 1.0099 |
| Image / Competitive / full | 24.625 → 26.979 | 54.667 → 55.604 | 5460.292 → 5418.563 | 0.9924 |
| Tiny / Exact / cold | 29.458 → 30.209 | 184.542 → 186.646 | 57.522 → 58.208 | 1.0163 |

**Image Exact/full:** the measured regression is in rendering. The median paired
render difference is **+75.104 µs**, mean paired difference **+190.593 µs**, and
the candidate is slower in 116/200 pairs. The difference of marginal render
medians is much larger, **+420.084 µs**; that distinction exposes distribution
shape and pairing effects, not a reason to replace the frozen metric. Open and
compile median paired differences are only +1.521 and +2.396 µs and are excluded
from this cell's performance metric.

**Tiny Competitive/cold:** median paired open/compile/render differences are
**+3.271 / +5.167 / +1.271 µs**; median paired total difference is **+10.709 µs**.
Paired means decompose total excess +8.486 µs into +3.537 open, +4.057 compile,
+0.886 render and +0.006 timing gaps. Thus **89.5% of the mean total excess is
before rendering**. The candidate is slower in 142/200 total-time pairs.
Direct memo initialization inside compositor rendering cannot alone explain
that upstream excess; binary layout or execution conditions remain hypotheses.

Tiny Competitive/full's paired render median difference is +0.792 µs; Tiny
Exact/full is +0.792 µs. Image Competitive/full instead has paired render median
−54.501 µs and mean −265.585 µs. These controls do not support a universal fixed
render penalty. They ran in separate time blocks, so they do not isolate mode
or cause while holding machine state constant.

## Order and temporal descriptions

Each row lists control-first / candidate-first counts, ratios of metric medians
within those strata, and their median paired metric differences in µs.

| Cell | Counts | Ratios | Paired differences |
| --- | ---: | ---: | ---: |
| Image Exact/full | 95 / 105 | 1.0504 / 1.1177 | +2.334 / +122.041 |
| Tiny Competitive/cold | 95 / 105 | 1.0515 / 1.0400 | +10.542 / +10.750 |
| Tiny Competitive/full | 104 / 96 | 1.0095 / 1.0363 | +0.251 / +1.355 |
| Tiny Exact/full | 108 / 92 | 0.9968 / 1.0123 | +0.625 / +1.188 |
| Image Competitive/full | 101 / 99 | 0.9891 / 0.9953 | −59.834 / −30.541 |

Contiguous 50-pair quartiles of Image Exact/full have metric ratios
**1.1562, 1.1110, 1.1250, 1.0104**; paired median differences are
+113.604, +215.062, +99.188, +19.146 µs. Its order split and final-block change
deserve profiling attention. Tiny Competitive/cold's quartile ratios are
**1.0315, 1.0272, 1.0550, 1.0407**; its positive effect persists in both order
strata and all four blocks.

Pairs were sequential within each shuffled cell, not interleaved across cells.
Image Exact/full occupied cell 10, 309.5–329.9 seconds after the first pair;
Image Competitive/full cell 14, 466.5–490.7 seconds; Tiny Competitive/cold cell
18, 594.0–617.6 seconds; Tiny Exact/full and Competitive/full cells 24–25,
844.0–901.4 seconds. Cross-cell differences can therefore include time effects.

Descriptive time-rank/log-paired-ratio correlations are −0.098 for Image Exact
and +0.166 for Tiny Competitive/cold; lag-one log-ratio correlations are −0.002
and −0.023. Near-zero coefficients do not prove independence or validate the
original bootstrap. These post-hoc partitions have no inferential confidence
claim and must not be used to discard pairs or rerun until a favorable block.

## Profiling implications

Profile Image Exact's render path and Tiny's open/compile stages separately.
For Image Exact, retained whole-process instruction medians are nearly equal
(ratio 0.9997), while cycle medians have ratio 1.0643. These process counters
include setup and emission; they neither attribute the measured stage cost nor
prove an instruction-level cause. Lazy memo initialization may target the small
render overhead, but these data do not justify promising recovery of the entire
Tiny cold regression. Preserve the original protected failures and freeze any
code revision and fresh experiment independently.
