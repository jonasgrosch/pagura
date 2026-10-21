# Rectangle reserve RSS investigation — unexecuted draft

This is one fixed, descriptive A/A investigation after the completed frozen
rectangle-reserve confirmation. It cannot accept the candidate, replace that
confirmation, erase any pilot/confirmation miss, create a noise allowance, or
establish a maximum-memory bound. No script, test, import, build or renderer has
been executed by the author.

The confirmation independently verified 15,580 outputs. The primary median
ratios are 0.9008598719870057 and 0.9270244661436559; their upper bounds pass
0.95, while their p95 upper bounds 1.0561882232604984 and 1.0524201619616782
fail 1.0. Complete target latency gates pass 0/4, protected latency 8/34, and
observed peak-RSS 26/38. No cell's latency lower bound exceeds 1.02. This is
not equivalence or all-gates admission. Four confirmation max-RSS ratios above
1.05 and the earlier Competitive many-rectangles pilot event remain unresolved.

## Frozen workload

Seven cells, each `cold` at 72 DPI, on each of the two historical binaries:

1. classic/graphics-state Exact
2. paint/text-knockout-true Exact
3. paint/text-knockout-true Competitive
4. paint/image-direct-gray-quarter-turn Exact
5. classic/many-rectangles Competitive (the retained pilot trigger)
6. classic/many-operands Exact (primary control)
7. classic/many-operands Competitive (primary control)

The baseline and candidate are separate strata. Labels A and B always invoke
the **same absolute binary** within a stratum. Each of the 14 strata has two
warmup pairs and 200 measured pairs: 2,828 pairs and 5,656 outputs total.
Seed 20261113 fixes the schedule. Every phase balances AB/BA per stratum, then
each round shuffles the 14 strata. There is one trial, no retry/resume, no
post-data cell changes, no discarded extremes, no resampling or new latency
inference. The 900-second deadline stops launches and caps each owned child;
cleanup may extend wall-clock completion. Failure/cancellation makes the
diagnostic incomplete, never an invitation to silently rerun it.

## Minimal invocation and pins

After root's static review and bounded tests, freeze source files and invoke
the producer directly through the existing runner and strict analytic19 load
observer. The auditor must receive the exact absolute argv, including Python
and `-B`; no implicit tests-then-exec wrapper is assumed.

```
ABS_PYTHON -B ABS/rss_aa.py \
  --root /Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura \
  --bindings ABS/benchmarks/results/rectangle-reserve-bindings.json \
  --bindings-sha256 ff8e7edb86715b42b9b07ed9129154d958411fb7666daa5a447ee2ff70223308 \
  --output NEW_ABS_EVIDENCE_DIRECTORY
```

Do not rewrite the original rectangle bindings. The producer retains their
exact bytes, original build report, completed build receipt/guard, and both
historical source/binary identities. It verifies binary bytes before each
invocation and at the end. No binary is rebuilt by this diagnostic. Current
helper/runtime source and current runner source are recorded separately from
historical binary source; the final receipt, not the copied running receipt,
authenticates completed execution.

The producer reuses `color_reuse_fixtures.fixture`, whose original manifest and
reference file hashes are pinned. Both their bytes and every selected PDF/PPM
are retained. All outputs must have exactly matching RGB, opaque alpha, the
expected mode/cold/72-DPI identity, and same-binary A/A bytes. The independent
auditor must load the externally pinned reference bytes independently; do not
substitute a candidate-generated image for a reference.

Freeze the diagnostic producer/tests/auditor, original bindings, original
pilot and confirmation report/analysis/receipt pins, exact invocation, loaded
helpers/runtime, strict observer and externally recorded new runner source.
The outer runner must hash `report.json`, `events.jsonl`, and the strict
observer's `guard.json` after the child exits. Preserve raw observer snapshots
and pin them for independent review. All code/runtime closure files are checked
again after the workload. Root should archive executed helper sources
byte-for-byte without changing the historical sources.

## Evidence schema

`plan.json` schema1 has the fixed schedule, cells, binary roles and identities,
input/oracle hashes/dimensions, original bindings hash, runner-start hash/source,
loaded file/runtime hashes, environment and RSS collector identity. Schedule
rows have `phase`, `pair`, `binary_role`, `case`, `mode`, `operation`, `order`.

`report.json` schema1 has diagnostic `rectangle-reserve-darwin-rss-aa`, current
runner pointer/source, original identities, `producer_argv`, plan hash,
start/end wall timestamps, duration, records, final status, and journal
path/hash/bytes/event count. Records contain their schedule row plus `sequence`,
pair start/end monotonic timestamps and A/B arm records. Arms retain command,
start/end monotonic timestamps, process result, log path/hash/size, raw RSS
bytes, output blob/hash, stage counters, and `verified`. Failed/interrupted
arms retain available fields plus `error`, `interrupted` or `artifact_error`.

The append-only `events.jsonl` has contiguous `event` integers, `type` and
`monotonic_ns`. Its completed sequence is:

- `run-start` with runner, source_before, producer_argv, started_unix_ns;
- for each pair: `pair-start` with pair_sequence, scheduled, started_monotonic_ns;
- for each label in its fixed order: `arm-start` with pair_sequence, label,
  command and started_monotonic_ns; then `arm-result` with pair_sequence, label
  and the complete available arm record;
- `pair-finished` with pair_sequence and finished_monotonic_ns;
- `run-finished` with status, finished_unix_ns, duration_seconds and error.

A completed run has 16,970 events. Each append is flushed before continuing;
there is no claim of fsync/power-loss durability. The initial report is written
once, the final report once. No growing full report is serialized per pair.
JSONL event/total bounds are 1MiB/128MiB. Retained metadata, inputs, references,
logs and deduplicated raw blobs are bounded at 128MiB, with a 64MiB per-file cap.
The report/journal are accounted separately. No successful sample is dropped
on exhaustion: the run fails incomplete.

Darwin RSS is parsed from exactly one full ASCII `/usr/bin/time -l` field and
cross-checked against the existing collector. Raw combined logs are retained;
non-ASCII, ambiguous, missing or potentially truncated logs block completion.
`run_process` may not return its temporary log on cancellation. The journal
retains attempt identity and any available raw output; the final outer receipt
and interrupted record remain authoritative, without inventing missing logs.

## Interpretation, not acceptance

Summaries are keyed `binary_role:case:mode:cold`. For complete measured pairs
only, they preserve all RSS values, minimum, inclusive quartiles, median, IQR,
maximum; A/B differences; first/second-position differences; order strata; and
ratios of sampled maxima in both directions. Warmups and partial records remain
in the raw evidence. There are no confidence bounds, latency summaries, gates
or pooled baseline/candidate ratio tests in this diagnostic.

The original four confirmation flags have lower candidate median RSS but higher
observed maxima. That motivates studying process/order variability; it does not
prove a noise cause. Similar same-binary variation supports a limited descriptive
explanation, not subtraction of an allowance or proof of equal tails. If
candidate-specific elevation or material unexplained tails remain, retention
stays blocked under the original plan. An uninformative trial is inconclusive.
Only the coordinator may adjudicate all evidence; all prior misses stay visible.
