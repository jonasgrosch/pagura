# Exact coordinator envelope for `quality.evidence`

This additional evidence envelope is coordinator-owned; the current launcher
does not parse it. Pin and review its raw contents using README.md.

```json
{
  "id": "six-arm-layout-q4",
  "kind": "instrumented-validation",
  "gates": ["PI-Q1", "PI-Q2", "PI-Q3", "PI-Q4", "PI-M1"],
  "decision": "pending",
  "report": {"path": "ABSOLUTE-PATH", "sha256": "ACTUAL-SHA256"},
  "receipt": {"path": "FINAL-RECEIPT-ABSOLUTE-PATH", "sha256": "ACTUAL-SHA256"},
  "guard": {"path": "ABSOLUTE-PATH", "sha256": "ACTUAL-SHA256"},
  "raw_artifact_manifest": {"path": "ABSOLUTE-PATH", "sha256": "ACTUAL-SHA256"},
  "restoration": {"path": "ABSOLUTE-PATH", "sha256": "ACTUAL-SHA256"},
  "checks": [],
  "review_note": "Fill only after terminal raw-evidence checks."
}
```

`raw_artifact_manifest` may be the same pinned primary report if it already
lists all required logs/binaries/module/runtime/source/snapshot artifacts.
Otherwise use a separate manifest covered by that job's final receipt or by an
explicit completed archival job; never imply it was a pre-run pin if it was
collected afterward. Each artifact descriptor gives its resolved path, actual
SHA-256 and byte count. Record both original path and archived-copy path when
different. These are byte-for-byte archived copies, not immutable files.

Use separate entries for production-full-check, six-arm-layout-q4, native-reach,
precision-host and (if needed) independent full/tile precision. Restoration
applies to instrumented entries; omit it only for unmodified production jobs.
Every gate maps to the relevant completed entries and an explicit reviewed
check list. Historical failures belong in `attempts` with actual terminal
status and pins; their presence does not support a passed gate.

The final `quality.layouts` rows use this schema, with observed positive
integer values in place of placeholders:

```text
target: native | wasm32-wasip1
type: Buffer | Option<Buffer> | Surface | MaskSink
baseline: {size: observed_integer, alignment: observed_integer}
candidate: {size: observed_integer, alignment: observed_integer}
```

Eight rows exactly, no duplicates. Both WASI SIMD configurations must agree
before representing them as one WASI row. Keep all six raw observations and
their Context/payload values in the evidence; do not discard differing results.

Restoration record minimum fields:

```text
schema: 1
status: pending | verified-restoration | failed
patches: [{path, sha256}]
files: [{relative_path, before_sha256, installed_sha256, restored_sha256}]
production_before: {runner_snapshot, benchmark_identity}
instrumented: {runner_snapshot, benchmark_identity}
production_after: {runner_snapshot, benchmark_identity}
fixture_helper_inventory: pinned descriptor
source_diff_review: recorded observation
```

Retain each overlay's restoration independently. Equal benchmark identities
alone cannot prove unchanged benchmark Python helpers omitted by that digest;
their separately frozen closure and all touched file hashes are mandatory.
