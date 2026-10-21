# Staged authoritative validation, unexecuted draft

This coordinator preserves the exact fourteen `tools/check.py.planned_commands(jobs=1)`
commands and per-command environment. It runs ONE indexed stage per invocation;
the owner schedules stages in order under the existing exclusive runner and
strict observer. It adds no scheduler, build flags, skipped checks or acceptance
allowance. All files are new, experiment-local evidence. No previous interrupted
full check contributes a stage. Nothing here has been imported, run or tested.

The complete freeze includes candidate runner snapshot/HEAD, check.py and helper
bytes, explicit web/generated-fixture/.cargo and inherited Cargo config inventory,
resolved runtime/toolchain executables, relevant build/runtime environment and the
exact command list. Missing runtime tools or a changed command count fail freeze.
Use the same absolute **actual Python app launcher** for freeze, runner, observer,
stage and aggregate, with no `-B` flag. Set PYTHONDONTWRITEBYTECODE=1 consistently.
The plan records `python` from resolved `sys.orig_argv[0]` for exact launch chains,
and separately `python_executable` from resolved `sys.executable`; both are pinned.
This preserves the observed macOS framework app/bin distinction. The freeze
uses rustup `which cargo/rustc` to pin selected executables; it builds
nothing. All paths in commands below are coordinator-selected absolute paths.

## Freeze once, then launch one stage at a time

From the candidate root, under the coordinator's serial scheduling:

```sh
PYTHONDONTWRITEBYTECODE=1 /ABS/ACTUAL/PYTHON /ABS/staged_check.py freeze \
  --root /ABS/CANDIDATE --plan /ABS/NEW-FREEZE/plan.json \
  --guard /ABS/CANDIDATE/benchmarks/results/sources/analytic19-conversion/load_observer.py
```

Record the printed plan SHA independently. Freeze and retain the five pure tests
via `ACTUAL_PYTHON /ABS/test_staged_check.py` before real stages. They exercise
protocol decisions only; they do not prove live process/signal behavior. Owned
child cleanup remains the existing `run_process`/ProcessGroup implementation.

For index 0, attempt `s00_a01`, after the existing read-only quiet preflight:

```sh
PYTHONDONTWRITEBYTECODE=1 /ABS/ACTUAL/PYTHON /ABS/CANDIDATE/tools/runner.py \
  --owner OWNER --task n16-stage-00 --kind validation --cwd /ABS/CANDIDATE \
  --receipt /ABS/NEW-FREEZE/attempts/s00_a01/receipt.json \
  --artifact /ABS/NEW-FREEZE/attempts/s00_a01/observer/guard.json \
  --artifact /ABS/NEW-FREEZE/attempts/s00_a01/stage/report.json \
  --artifact /ABS/NEW-FREEZE/attempts/s00_a01/stage/stage.log -- \
  /ABS/ACTUAL/PYTHON /ABS/CANDIDATE/benchmarks/results/sources/analytic19-conversion/load_observer.py \
  --evidence-dir /ABS/NEW-FREEZE/attempts/s00_a01/observer -- \
  /ABS/ACTUAL/PYTHON /ABS/staged_check.py stage --root /ABS/CANDIDATE \
  --plan /ABS/NEW-FREEZE/plan.json --plan-sha256 PLAN_SHA --index 0 --attempt s00_a01
```

Use successive indices through 13, a new attempt name each time, and the exact
argument order shown. Each stage has the existing 1200-second owned-group timeout,
independent of the observer's 90-minute cancellation window. Completed stages
retain their report/log after later interruption. The active stage's combined
log can remain unavailable when run_process raises; it is explicitly recorded,
not invented or accepted. No new production lifecycle code is introduced.

A retained external-contention refusal/cancellation can be retried explicitly
at the SAME index with a new name. The aggregate inventories every attempt and
requires source equality, known observer statuses and actual foreign matches or
outer SIGINT/SIGTERM. Actual test/assertion/nonzero/timeout/evidence failure blocks
this freeze, even if a later attempt passes. Fix source and make a new freeze;
this small implementation has no override for a diagnosed semantic failure.
Do not delete failed attempts or reuse successful indices.

## Aggregate only after every stage is terminal

Run a separate summary job under the existing runner/observer, outside `attempts/`:

```sh
/ABS/ACTUAL/PYTHON /ABS/staged_check.py aggregate --root /ABS/CANDIDATE \
  --plan /ABS/NEW-FREEZE/plan.json --plan-sha256 PLAN_SHA \
  --output /ABS/NEW-FREEZE/aggregate.json
```

Retain the aggregate's own final receipt and strict observer. It authenticates
all attempts, exact runner -> guard -> stage argv, fourteen serial successful
stages and source/env/config/fixture/runtime equality; it reads each raw log and
observer snapshot and pins the entire retained attempt inventory. Stage receipts
must bind their actual report/log and guard. Existing observer events contain
snapshot filenames but **no hashes**: the aggregate pins those raw bytes after
the stage and checks original receipt pins where present. It does not pretend
those were pre-run or original-stage snapshot pins. Missing/changed/unavailable
evidence, overlapping or out-of-order attempts, duplicate successes and unresolved
failure all reject aggregation. Interrupted aggregate output is not admission.

The result is `verified-staged-check`, stage_count=14, admission=false. It proves
the authoritative command scope across separate serial jobs, **not** that one
uninterrupted tools/check.py process completed. Amend the quality assembly recipe
explicitly to accept this alternative PI-Q5 workflow before pilot, keeping the
separate precision host/resource/layout/reach requirements. Its source must equal
the production quality record and frozen runner snapshot. No timing gate changes.
