# Painted-interval harness reconciliation against a9814ee4

Source-only package prepared against
`a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c` at
`/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura`.
No active checkout was edited. No imports, fixture generation, tests, builds,
renders, profiles or benchmarks were executed. Apply only after the coordinator
releases the current start/current experiment's source freeze.

## Package

- `overlay/`: eight experiment files and four JSON templates, adding the same
  experiment-local native/WASI drivers, fixture source, wrappers, launcher and
  tests from `/private/tmp/n16-painted-interval-harness-v2/overlay`.
- `harness.patch`: the twelve overlay additions, with repository-relative paths.
- `PLAN.md`: reconciled copy of `/private/tmp/n16-painted-interval-plan.md`.
- `QUALITY-RECORD.md`: unchanged trusted coordinator quality-record schema.
- `SHA256SUMS`: byte identities of the package files after reconciliation.

No product patch is included here. The current proposed product/test patch is
`/private/tmp/pagura-n16-interval-current/painted-interval.patch`, SHA256
`c71d0c06c2f7c621d8c4a67b875e91070b780caa9dc600a71da5d9df7d676ecb`.
Use its `VALIDATION-CHECKLIST.md` for actual correctness/resource/layout work.
That patch and this package remain unexecuted proposals.

## Exact changes from v2

1. Omitted the old whole-file `benchmarks/evaluate_n16_output.py` replacement.
   The current helper already has explicit case/cell counts, strict-guard mode,
   target operation and newer `target_cases` support. Its current default
   remains16 cases/46 cells, opaque64/full and observation-only unless a caller
   opts in. The painted-interval wrapper already requests18/52 and strict mode;
   it needs no shared-helper change. `paired_render.py` is also reused unchanged.
2. Added one focused, unexecuted test that exercises current multiple cold
   targets, their protected control, duplicate-target refusal and unchanged
   historical opaque64/full defaults. Existing matrix/oracle/guard tests remain.
3. Updated native and WASI explanatory limitations to describe the actual
   unchanged profile: default Repair opening, full `compile_page`, diagnostics
   not emitted. This is distinct from strict correctness probes.
4. Updated plan wording to the current patch/source lineage. Ordinary admitted
   translucent controls now use F32 in both modes; nonopaque backgrounds and
   relevant groups remain F64. Direct U8 tests remain legacy controls. The
  171/540 profile is identified as an earlier source-specific observation, not
   a new measurement of a9814ee4. Removed the superseded v3/public-test-patch
   pointers in favor of the current integrated patch/checklist.
5. Repaired the experiment-local WASI cancellation gap at the coordinator's
   request: scoped SIGINT/SIGTERM handling, an atomic initial/final report,
   persisted attempt metadata before invocation, and bounded capture of every
   available raw output/stdout/stderr/metadata file after interruption. Existing
   `run_process` still owns child signal forwarding, timeout and group cleanup;
   no scheduler, retry or alternate timing interval was introduced. Two small
   unexecuted tests exercise interrupted artifact retention and a failed read
   without losing later available evidence or fabricating a process result.
   They do not exercise live main-path signal handling. The diagnostic boolean
   is named `regression_signal` so it cannot shadow the imported signal module.

Current reused helper pins at review time:

```text
feb245618601265124f11cb2af090ea24fc6a7bff03f8a729ee2708714ae98eb  benchmarks/paired_render.py
cd71c7124e89cb6751bc2bf23cd655b187b88552f1dd6beb4b51fcefa4ae8692  benchmarks/evaluate_n16_output.py
```

The only diff between the old v2 shared evaluator and the current helper is
the current `selected_targets` import/selection and propagation of optional
`target_cases`. Copying the old replacement would regress that support; this
package deliberately contains neither shared file.

## Frozen experimental choices preserved

Native18 cases/52 cells,2 opaque64/full targets and50 protected. All earlier46
cells/roles remain. New cells are full renders of empty/distant/existing-tiny in
both modes; the two new case oracles are byte-for-byte unchanged.

- Pilot5 measured+5 warmup pairs/cell, seed20261105,1,040 outputs.
- Confirmation200 measured+5 warmup pairs/cell, seed20261106,21,320 outputs.
- WASI12 full cells,12 measured+2 warmup pairs/cell, seed20261107,336 outputs.
- Bootstrap5,000 resamples, one-sided95%, seed42017; inherited evaluator
  authenticates the reported bootstrap bounds rather than resampling.
- Target median upper≤0.95 and p95 upper≤1.00; protected median/p95 upper≤1.02;
  every native sampled max-RSS ratio≤1.00 for a full incremental experiment pass.
- Existing independent zero-tolerance RGB/opaque alpha and same-mode byte
  equality; unchanged mandatory safety/resource/layout/portability gates.
- The original partial-retention policy is unchanged: at least one original
  target median upper≤0.95 in the sole fresh confirmation, mandatory gates
  complete, all p95/protected/RSS misses disclosed and supported protected
  regressions explicitly handled. Retention remains a coordinator decision.

No historical samples or start/current samples can enter this experiment.

## Remaining prelaunch requirements and known gap

This is not a frozen launch package. Actual current baseline/candidate commits,
binary/module paths, source identities, build receipts, runtimes, plan/binding
hashes, layout/reach evidence and quality-record gates remain null/unavailable
until root creates them. Do not substitute the historical c70/u16 identities.

The launcher requires per-side successful build receipts whose `source_root`
matches that side and whose argv contains identical native Cargo suffixes.
A new build driver that wraps both sides in one candidate-root receipt does not
satisfy that contract without an explicit, independently reviewed adaptation.
Use the existing specified per-side build form instead; do not fake receipt
fields or weaken the artifact binding. WASI uses its exact documented Cargo
suffix. Use the actual pinned Python executable path to avoid a launcher/app
handoff mismatch in this older direct-exec contract.

The inherited native evaluator does not establish the additional launcher argv
chain. Root must independently verify receipt.argv → strict guard.command →
receipt-bound launch-intent argv → exact driver command, as the checklist already
requires. The WASI auditor has this explicit chain. The strict observer must
retain the declared5-second scans/5,400-second window/three quiet observations;
these are cooperative observations, not OS-enforced exclusivity.

The new experiment-local cancellation repair is source-only and still requires
root's serial validation. A canceled child's temporary combined log can remain
unavailable from `run_process`; the report keeps the attempted command and
available wrapper/raw files without inventing a returned process/log result.
The shared native helper is deliberately unchanged; its periodic snapshots and
failed outer receipt do not constitute a complete per-invocation journal. Final
receipt/observer authentication is always required; no successful or canceled
execution has been claimed by this package.

Root must run the focused harness tests and integrated validation serially.
Nothing in this package replaces actual product tests, native/WASI layout and
reach probes, independent output preflight, or final receipt/audit checks.
