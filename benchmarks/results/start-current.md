# Session-start to current: measured cumulative result

2026-10-21. The retained stack improves selected Exact workloads and the
independent accuracy probes, but it is not an overall speed improvement.
Several Competitive text workloads regress substantially, the opaque-text
full-page fixture is roughly twice as slow, and many process-memory observations rise.
The completed comparison supports keeping useful improvements while making
recovery of these costs a priority. It does not support release admission or a
claim to beat competing engines.

## Fixed comparison and independent checks

The starting production source is `359e19dd`, represented by control `676a872e`
with the identical common profile driver. The current product is `a9814ee4`,
which merges main `d81d0c82` and retains the experimental renderer stack. This
is a merge basis, not a claim that today's latest main has been rebased again.
Both arms use the same pinned compiler, flags, driver, inputs and public API.

Confirmation 003 completed all 84 cells, five warmup pairs and 200 measured pairs
per cell: 34,440 output records. The independent auditor verified the complete
schedule, raw pixels, process records, source/binary/input bindings, terminal
receipts and observer snapshots, then recomputed the fixed paired statistics.
The confirmation and audit both exited zero, with unchanged sources and no
known foreign compute detected. The observer samples known processes; it is
not proof of absolute host exclusivity.

All current outputs pass the independent references. Both arms produce correct,
equal output in 83 cells. Competitive translucent text is the remaining cell:
the starting product produces 63 instead of 64, so its timing is retained as
correction cost only. No cell was dropped. Confirmations 001 and 002 were
externally interrupted, retained and excluded; their timings were never
inspected or pooled into this result.

## Selected effects, with the complete matrix retained

These are median elapsed-time point estimates, not a weighted total. The
[full 84-cell tables](start-current-cells.md) include every case, latency median
and p95, their separate one-sided bounds, memory distributions and maximum RSS.
The [audited JSON](start-current.json) also preserves stage summaries and signed
paired differences. Ratios below are current / starting; smaller is faster.

Across the 83 equivalent-output-qualified cells, the median latency upper bound
is below one in 14, the lower bound is above one in 49, and the endpoints span
or touch one in 20. These are
descriptive per-cell classifications, not a weighted aggregate or simultaneous
confidence statement. The asymmetry is a reason to prioritize regressions.

| Workload and operation | Mode | Starting median ms | Current median ms | Ratio | Change |
| --- | --- | ---: | ---: | ---: | --- |
| Outlined text, full | Exact | 5.4014 | 2.4584 | 0.4551 | 54.5% lower latency |
| Outlined text with knockout, full | Exact | 8.3005 | 5.2465 | 0.6321 | 36.8% lower latency |
| Sparse text, full | Exact | 8.4558 | 7.0115 | 0.8292 | 17.1% lower latency |
| Dense text, full | Exact | 10.9269 | 9.6772 | 0.8856 | 11.4% lower latency |
| Image-only, full | Competitive | 5.3839 | 4.4086 | 0.8189 | 18.1% lower latency |
| Sparse text, full | Competitive | 1.6187 | 6.3114 | 3.8989 | 3.90 times the latency |
| Dense text, full | Competitive | 1.8161 | 8.2092 | 4.5203 | 4.52 times the latency |
| Nested text, full | Competitive | 1.8039 | 9.2211 | 5.1117 | 5.11 times the latency |
| Opaque text, full | Exact | 1.9247 | 4.0626 | 2.1108 | 2.11 times the latency |
| Opaque text, full | Competitive | 1.7191 | 3.4016 | 1.9787 | 1.98 times the latency |
| Opaque text, complete tiles | Exact | 13.6176 | 8.1247 | 0.5966 | 40.3% lower latency |
| Many operands, open + compile + render | Exact | 10.8728 | 13.9203 | 1.2803 | 28.0% higher latency |

Every listed latency direction is supported by its frozen marginal median
bound. That does not provide simultaneous coverage over the matrix. Operation
matters: Exact opaque first-tile and complete-tiled rendering improve while its
full and cold operations regress. Tiny-page and preparation costs remain
visible in the full table. Exact sparse first-tile and complete-tiled medians
regress 5.9% and 7.3%, despite its full-page gain. Exact many-rectangles cold
latency rises 31.3%. The current Competitive result is still faster than
current Exact on several text cases, but it has lost much of its starting speed.

Memory does not pass a blanket non-regression claim. For example, observed
maximum process RSS for Competitive sparse full rendering rises from 20.66 to
31.75 MiB (1.5371 times), nested full from 20.61 to 33.83 MiB (1.6414 times),
and late outlined text from 5.67 to 10.94 MiB (1.9284 times). Exact sparse full
falls from 31.94 to 28.80 MiB. These are sampled OS process high-water marks,
including runtime costs, not deterministic allocator-capacity measurements or
confidence bounds on maxima. All memory cells remain published.
Among the 83 qualified cells, median RSS bounds are above one in 53 and p95
bounds in 39; observed maximum RSS exceeds the baseline in 54. These counts
retain the same marginal-comparison and sampled-maximum limitations.

## Accuracy and next decision

The [separate strict accuracy report](start-current-accuracy.md) independently
checks 108 outputs, 54 per version. Current passes all 54; the starting product
fails 23 of its 54 selected outputs. The cases establish particular root
precision, low-opacity and premultiplied-background corrections, including
improvements over original Exact. They are selected regression probes, not a
document-population accuracy percentage or continuous-scene perfection claim.

Retain the useful experimental improvements and accuracy corrections. Do not
claim that their historical incremental percentages combine into an overall
gain: this direct measurement exposes the costs that adding those percentages
would hide. Nor does this multi-change comparison isolate each regression's
cause. Profile the current routes before assigning individual blame.

The isolated N16 painted-interval implementation is the next bounded experiment:
avoid conversion of untouched background pixels while preserving dense buffer
output, arithmetic, work/error ordering and allocation admission. It has no
validated gain yet. Run its native/WASI layout, accuracy and resource checks,
then its fixed pilot and confirmation. Separately prioritize the larger
Competitive text losses; an N16-only win cannot be assumed to repair F32 text
costs. Preserve successful partial wins under the recorded retention rules,
while keeping every protected latency and memory miss visible.

## Evidence and limits

- [Frozen protocol](../../docs/plans/start-to-current-measurement.md) and
  [build/input bindings](start-current-bindings.json).
- [Complete independent confirmation audit](start-current.json), SHA256
  `cbffe989ac870d059a0e467ac90247edcec6827f022506bef13747b2ddbb0f31`.
- [Preflight audit](start-current-preflight.json) and
  [strict accuracy audit](start-current-accuracy.json).
- [Final evidence manifest](start-current-evidence.json) binds terminal local
  records, excluded interruptions, archived results and helper sources. Raw
  logs, inputs and outputs remain local under `local/start-current/`; the
  committed summaries and hashes are not a substitute for those raw artifacts.

This is native arm64 evidence. The timed driver uses default repair opening,
full compilation, page zero, 72 dpi and an opaque white background; it does not
emit repair diagnostics. Cold means internal open + compile + render, not cold
filesystem I/O. Prior native/WASI product validation is separate evidence.
There is no new competitor result, geometric mean, worldwide percentage,
release admission or promised completion date.
