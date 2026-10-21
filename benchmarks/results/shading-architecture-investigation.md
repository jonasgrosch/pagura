# Shading execution investigation

2026-10-21. Read-only diagnosis of product `402e3afe`, not an optimization.
The [architecture and handoff ADR](../../docs/adr/0033-prepared-renderer-and-session-handoff.md)
contains the proposed response and migration gates.

## Measured stages from the retained cold comparison

All stage values below are medians in milliseconds; medians of stages need not
sum to the median total. Engine time is opening plus rendering/preparation.
Emission is separate. Settings and independent output qualification come from
the [audited 39-page comparison](current-stack-hayro.md).

| Case / mode | Pagura open / render / emit | Hayro open / render / emit | Engine ratio |
|---|---|---|---:|
| Direct shading / Exact | 0.0531 / 19.3894 / 0.7753 | 0.0373 / 0.3070 / 0.6033 | 56.3218 |
| Pattern shading / Exact | 0.0559 / 21.6907 / 0.4870 | 0.0392 / 0.3067 / 0.2844 | 62.7959 |
| Direct shading / Competitive | 0.0585 / 20.2909 / 0.5743 | 0.0425 / 0.3189 / 0.2965 | 56.1793 |
| Pattern shading / Competitive | 0.0571 / 19.4861 / 0.5061 | 0.0413 / 0.3191 / 0.2947 | 54.0696 |

Whole-process times are approximately 25--27 versus 5.2--5.5 ms, a 4.5--5.2
ratio. The 54--63 ratio must not be described as whole-process latency. This
campaign has per-page cold stages; it does not measure a persistent multi-page
document or exclusive per-function costs in both engines.

## Focused sampling

The already built Pagura binary was used unchanged, SHA-256
`e8013da445f4a2d5e45d3629a95403e11feb1ddb4428b2ced7954864b934b057`.
Three profiles ran serially under tools/runner.py and the retained known-process
observer, with no observed prohibited work. Each invoked the existing public
CLI `microbench INPUT render-warm 700 72 --accuracy MODE`, sampled for three
seconds at one-millisecond intervals, then captured vmmap summary while the
renderer remained alive. This repeats page compilation/rendering through a
warm context, not a precompiled bare compositor. No PPM emission occurs.
Profiler/VM inspection perturbs execution; these times are not new comparative
speed measurements. There is no corresponding sampled Hayro cost attribution.

| Exclusive top-of-stack sample category | Direct Exact | Pattern Exact | Pattern Competitive |
|---|---:|---:|---:|
| shading_sample self | 36.955% | 43.564% | 39.362% |
| Function evaluation including pow | 19.198% | 15.699% | 19.110% |
| Color/source setup | 24.572% | 13.736% | 23.719% |
| Blending | 5.218% | 5.534% | 4.965% |
| Output conversion | 1.012% | 6.436% | 1.221% |

These disjoint, symbol-based categories are approximate: inlining, sampled
instruction positions and short capture windows limit attribution. The system
report omits flat entries below five samples. Remaining samples include loop,
memory, transfer and other work. pow alone occupies about 5--6%, so eliminating
it cannot explain a prospective large speedup.

The raw sample reports, original helper, stdout, metadata and receipts are in
[sources/shading-investigation](sources/shading-investigation/README.md).
One preceding helper attempt failed an input-selector assertion before any
renderer/profile child started. It is preserved as run-001; the corrected
run-002 completed all three profiles. This was an orchestration correction,
not retrying an unfavorable measured outcome. Input and binary hashes were
independently rechecked after completion. Repository source was unchanged.

## Source/layout diagnosis

Both fixtures in benchmarks/f32_painted_interval_fixtures.py are 512-square,
DeviceRGB axial Type 2 functions with equal black endpoints, exponent one and
opacity one quarter. The independent RGB-over-white result is 191 everywhere.
Each page has 262,144 samples, with no extra antialias samples. Four chunks
execute sequentially.

Pagura's compositor/shading/function paths evaluate the function and construct
generic color/source state per covered pixel. Type 2 fills caller-owned storage;
there is no per-sample Vec allocation on this path. Hayro 0.8 prepares these
functions with five samples and uses Vello's backend gradient, compact ramp
and SIMD sampling. No equal-stop-to-solid collapse was identified in that
backend path. Hayro's general gradient tolerance and U8 accumulation are not
automatically compatible with Exact; the opportunity to hoist invariant work
is independent of those approximations. See the ADR's source-study provenance.

| Large live renderer payload | F32 direct / Competitive pattern | F64 Exact pattern |
|---|---:|---:|
| Full-page output | 1 MiB | 1 MiB |
| One chunk surface | 1 MiB | 3.5 MiB |
| Coverage | 64 KiB | 64 KiB |
| Total modeled payload | 2.0625 MiB | 4.5625 MiB |

F64 Pixel holds seven doubles; F32 holds four floats. Exact pattern admission
is conservative and selects F64; direct shading selects F32 in both modes.
PPM output later creates RGB/PPM temporary copies after chunk surfaces drop.
Warm vmmap snapshots are consistent with roughly 2.1 versus 4.6 MiB allocator
live allocations, while retained resident pages differ substantially. Neither
these snapshots nor source payloads attribute the original cold RSS extremes.

For Exact pattern, the cold sampled maximum is 19.422 MiB versus 4.031 MiB
(4.818 times), while median per-process peak RSS is 7.125 versus 3.922 MiB
(about 1.82 times). The planner penalty explains concrete storage, not every
observed process-memory difference. Dense shaders mark the entire output and
do not benefit from untouched-interval conversion skipping.

Conclusion: shared prepared paints and efficient span execution are the main
architecture priority. Independent precision/storage work can reduce memory.
Constant-function admission alone is a narrow first proof, not evidence of
general shading or renderer leadership.
