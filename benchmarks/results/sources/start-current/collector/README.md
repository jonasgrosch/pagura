# Fixed session-start/current collection

Draft for central review/execution only. No generator, tests, imports, builds or
renderer commands were executed while authoring this directory. Historical
helpers are unchanged. `collect.py` delegates child ownership, cancellation,
timeouts and RSS to the existing `benchkit.measurement.run_process`.

The two frozen product commits are baseline
`676a872eb9f6c279cbf1e3929ace3a0c99db9fc3` and candidate
`a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`. Build correspondence is a trusted,
externally pinned coordinator record; the collector rereads actual binaries,
source identities and identical public profile example before/after collection.
The baseline example overlay is deliberate and is not a pristine-checkout claim.

## Fixed scope

All46 `opaque_composition.CELLS` plus all19 analytic fixtures ×both modes/cold
remain84 namespaced cells. This is native one-thread default-repair opening and
full-compilation retained rendering, page0/72DPI/opaque white. The unchanged
profile example uses `OpenOptions::default()` (repair), and does not emit open or
compile diagnostics. Do not call this strict-open or diagnostic-parity evidence.
The separate accuracy adapter may explicitly use strict opening.

Preflight has one B→C pair for each cell,168 child attempts. Confirmation has
five warmup pairs then200 fresh measured pairs per cell,34,440 attempts. One
`random.Random(20261114)` shuffles each cell's3BC/2CB warmup order, then shuffles
cell order for each warmup round; continuing the same generator, it shuffles
each cell's100BC/100CB measured order and cell order for each measured round.
B is baseline, C is candidate. The complete schedule is written before children.
No retry, resume, interim statistics, changed count or removed cells exist.

`cold` selected metric is `total_ns` (open+compile+render); all other operations
select `render_ns`. Process startup/file I/O/output emission are outside those
internal metrics. Each child additionally records whole-process elapsed and
Darwin high-water RSS. Preflight hashing warms files. No cold-I/O, generic
document, competitor or global aggregate claim follows.

## Invocation and freezing

Use an absolute Python launcher, `-B`, and absolute script/path arguments. Root
first copies these four reviewed sources to their final immutable-by-convention
location, freezes their hashes, builds both profile binaries serially and runs:

```text
PYTHON -B freeze_inputs.py --root CANDIDATE_ROOT --output NEW_INPUT_DIRECTORY
```

`inputs.json` freezes35 independently constructed or previously frozen PDF/PPM
pairs before renderer preflight. It records the ordered84-cell matrix, absolute
input descriptors and every loaded fixture/helper/runtime file hash. Never
regenerate an oracle from either renderer. Root constructs externally pinned
base bindings, then runs each phase under the existing exclusive runner and
strict contention observer:

```text
PYTHON -B collect.py --root ROOT --bindings BINDINGS --bindings-sha256 SHA --phase preflight --output NEW_PREFLIGHT
PYTHON -B collect.py --root ROOT --bindings BINDINGS --bindings-sha256 SHA --phase confirmation --output NEW_CONFIRMATION --preflight-pins PREREQUISITES --preflight-pins-sha256 SHA
```

Base bindings have `experiment: "start-current"`, `identities` keyed baseline and
candidate (root, commit, source_sha256, binary, binary_sha256), `build_report`,
`completed_evidence`, `bound_files_sha256`, and `inputs`. The build fields use the
existing frozen paired-build schema. `inputs` points to `inputs.json`.
`bound_files_sha256` maps candidate-root-relative paths to frozen source hashes.
External descriptors are `{path: ABSOLUTE, sha256: HEX}` with optional exact
`bytes`; retained descriptors always have all three fields and relative paths.

The independent audit must finish before root freezes confirmation prerequisites:

```json
{
  "schema": 1,
  "experiment": "start-current",
  "bindings_sha256": "BASE_BINDINGS_SHA256",
  "preflight": {
    "report": {"path": "/absolute/report.json", "sha256": "..."},
    "plan": {"path": "/absolute/plan.json", "sha256": "..."},
    "journal": {"path": "/absolute/events.jsonl", "sha256": "..."},
    "receipt": {"path": "/absolute/final-receipt.json", "sha256": "..."},
    "guard": {"path": "/absolute/guard.json", "sha256": "..."},
    "audit": {"path": "/absolute/audit.json", "sha256": "..."}
  },
  "accuracy": {
    "report": {"path": "/absolute/report.json", "sha256": "..."},
    "receipt": {"path": "/absolute/final-receipt.json", "sha256": "..."},
    "guard": {"path": "/absolute/guard.json", "sha256": "..."},
    "audit": {"path": "/absolute/audit.json", "sha256": "..."}
  }
}
```

The preflight audit exposes `status: verified-preflight`,
`candidate_preflight_correct: true`, `bindings_sha256`, and `report_sha256`.
The separate accuracy audit exposes `status: verified-accuracy-probes`,
`candidate_quality_verified: true`, and `report_sha256`; its report carries
matching source identity subsets, while its actual binary identities may differ.
Final prerequisite receipts must bind required reports/plans/journals and guards;
they must be completed, exit0, source unchanged and error-free. The externally
pinned audit checks the complete exact argv/snapshot/source chain. This collector
does not treat an arbitrary self-described report as authenticated evidence.

## On-disk schema and failures

`plan.json` schema1 contains experiment/phase,84 cells,35 inputs, full schedule,
identities, external binding/input manifest pins, exact producer argv, app and
launcher runtime hashes, loaded source closure, and running-receipt snapshot.
Input keys are `opaque:<case>` or `analytic19:<case>`. Each input has PDF/oracle
descriptors and full width/height; tile comparisons crop the independently
frozen reference exactly as the unchanged profile's first-tile operation does.

`events.jsonl` has consecutive zero-based `event`, `type`, `monotonic_ns` fields.
Order is run-start, then pair-start/arm-start/arm-result/arm-start/arm-result/
pair-finished for each pair, then run-finished. A completed preflight has506
events; a completed confirmation has103,322. Failure journals may stop mid-pair
but preserve every invocation start and available result. Flushing is not fsync
or power-loss durability. The terminal report is written once; there is only an
initial running snapshot before collection, not a growing report per pair.

Each report record has sequence, phase, pair, cell_index, suite, case, mode,
operation, order, start/end monotonic timestamps, B/C records, raw_equal and
classification. Each arm records actual command, process (including elapsed/RSS),
raw combined-log descriptor, output descriptor if present, stages if the process
succeeded, process_success, quality and start/end times. `run_process` returns
provisional `status: failed` even for exit0; success therefore requires integer
exit0 and non-timeout status. The independent audit rereads this raw evidence.

Expected RGBA bytes are constructed only from the frozen RGB oracle plus alpha255.
Correct outputs use a byte-equality fast path; detailed mismatches are calculated
once per distinct raw output/oracle/operation hash. Every child output is still
read, hashed and retained. Output hashes also bind the same independently checked
classification on identical later bytes. This saves verification work outside
the measured interval and makes no renderer speed claim.

Ordinary renderer failures and wrong pixels remain visible for every planned
attempt. Completed preflight with any candidate failure has
`timing_launch_allowed:false` and exits1. Baseline-only failures never shrink the
matrix. Unavailable RSS/stages, nonroundtripping/truncated logs, source drift,
retention exhaustion or cancellation abort incomplete without a retry. A
cancelled child's temporary log may be unavailable from `run_process`; its start,
available output and outer cancellation remain. Existing120s per-child timeout
and the outer observer's cancellation window are separate; owned cleanup must
finish before resources are released.

Successful classification collection uses `completed-preflight` or
`completed-confirmation`; this is not admission. Candidate failures make exit1.
Incorrect baseline timings may describe correction cost only. A cell is eligible
for equivalent-output interpretation only when all its required preflight,
warmup and measured outputs are independently correct and pairwise equal.
No metrics may discard incomplete/nonpositive pairs. Post-completion statistics
are independently computed:5,000 paired resamples, one-sided0.95,seed20261114,
median/p95 latency and RSS ratio bounds; sampled maximum-RSS ratio has no CI.
All distribution/stage and temporal limitations remain explicit.

Raw artifacts are content-addressed and deduplicated with64MiB per-file/1GiB
total retention limits; journal and terminal report each have separate256MiB
limits. No successful sample may be silently dropped. Root must pin final
report, plan, journal and guard in the final outer receipt, then freeze the
independent auditor's pins/result. Running-receipt snapshots are never final
receipt evidence. Both Python launcher and actual framework app are bound before
children, including their distinct paths on macOS.
