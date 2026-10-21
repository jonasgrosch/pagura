# Required coordinator quality record

This is a schema description, not test evidence. Before pilot, the coordinator
assembles an artifact from completed validation, pins its underlying raw logs
and records the reviewed result under a completed source-unchanged runner
receipt. The launcher checks the following fields; it does not independently
interpret every Rust assertion or reconstruct structural instrumentation.

```json
{
  "schema": 1,
  "experiment": "n16-painted-interval",
  "status": "passed",
  "source": {"commit": "ACTUAL-CANDIDATE-40-HEX", "sha256": "ACTUAL-RUNNER-SNAPSHOT-64-HEX"},
  "gates": {"PI-Q1": "passed", "PI-Q2": "passed", "PI-Q3": "passed", "PI-Q4": "passed", "PI-Q5": "passed", "PI-M1": "passed", "PI-REACH": "passed"},
  "layouts": [],
  "evidence": []
}
```

`layouts` must contain exactly eight actual observation rows: cross product of
targets `native`, `wasm32-wasip1` with `Buffer`, `Option<Buffer>`, `Surface`,
`MaskSink`. Each row has `target`, `type`, `baseline: {size, alignment}` and
`candidate: {size, alignment}` in bytes. No value is supplied by this draft.
Every candidate size and alignment must be no greater than its baseline value.
Execute both targets' probes; static layout guesses cannot populate this record.

`evidence` should identify the completed validation/probe receipts, commands,
source/module/runtime pins and raw artifacts supporting every gate. Preserve
source snapshots for diagnostic-only instrumentation separately and explain how
they correspond to the production definitions. Do not overwrite production
build receipts with probe-source identities. This manual aggregation is trusted
coordinator evidence; the launcher requires all seven gate names and measured
layout numbers, but does not verify the interpretation of the raw logs.

The completed receipt supplied as `prerequisites.quality.receipt` must have the
production candidate's runner source snapshot and bind the exact quality-record
artifact supplied in `prerequisites.quality.artifact`. It may be a small final
read-only validation-summary job after the raw checks complete. Do not claim the
summary job itself executed all checks. Missing, failed or unavailable gates
must remain explicit and block pilot; never synthesize a passing record.
