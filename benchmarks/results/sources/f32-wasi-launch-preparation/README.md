# Prepare the two WASI launch records

`prepare.py` is preparation only: standard library, no product/helper imports,
subprocesses, build, launch, fixture regeneration or source changes. It creates
only one **new preparation directory**. It does not create either future
collector directory or any audit result path. Both run directories must be
absent and separate from each other and the preparation directory.

Invoke with the frozen Python path from preflight bindings (the same runtime
as the completed native preflight); all listed JSON SHA values are explicit
coordinator pins, not guessed from the contents. Example with absolute paths:

```sh
ABS_FROZEN_PYTHON -B /private/tmp/f32-launch-preparation/prepare.py \
  --bindings RESULTS/f32-painted-interval-preflight-bindings.json --bindings-sha256 HEX \
  --selection LOCAL/build-selection-001.json --selection-sha256 HEX \
  --preflight-report LOCAL/native-preflight-001/report.json --preflight-report-sha256 HEX \
  --preflight-receipt NATIVE_FINAL_RECEIPT --preflight-receipt-sha256 HEX \
  --preflight-guard NATIVE_FINAL_GUARD --preflight-guard-sha256 HEX \
  --simd-build-bindings RESULTS/f32-painted-interval-wasi-simd-build-bindings.json --simd-build-bindings-sha256 HEX \
  --scalar-build-bindings RESULTS/f32-painted-interval-wasi-scalar-build-bindings.json --scalar-build-bindings-sha256 HEX \
  --node /opt/homebrew/bin/node --node-sha256 HEX \
  --simd-output LOCAL/wasi-simd-001 --scalar-output LOCAL/wasi-scalar-001 \
  --output LOCAL/wasi-launch-preparation-001
```

`RESULTS` and `LOCAL` above are placeholders, not shell variables: replace each
with its absolute path. `LOCAL` is candidate
`benchmarks/results/local/f32-painted-interval`. The plan descriptor comes from
the immutable preflight binding. Selected source/module/context/receipt paths
come from the two feature bindings and must agree with build-selection-001.
The six PDF pins come from `report.inputs[case].pdf`; their actual retained bytes
are rehashed. The full independent native pixel audit is a separate prerequisite.

Outputs are `simd-argv.json`, `scalar-argv.json` (exact JSON argument arrays),
`simd-audit-pins.json`, `scalar-audit-pins.json` (the existing independent auditor
schema), and `manifest.json` (`status: prepared-not-launched`, `admission: false`,
all input/output/helper/runtime descriptors). Every file is exclusively created.
No `--run-id` is generated; the existing runner supplies its ID and receipt env.

The producer prefix is exactly frozen Python, `-B`, frozen WASI driver, followed
by its fifteen option/value pairs. Pass that array unchanged after the existing
strict observer's `--`; the auditor expects the observer itself to run as frozen
Python, `-B`, pinned observer, `--evidence-dir`, absolute guard directory, `--`,
then the producer array. The final outer receipt must bind report.json, plan.json
and guard.json. Pin the preparation manifest/argv/audit-pins externally before
each launch. Final receipt/guard hashes go to the separate auditor after execution.

Python launcher/app/reported-executable bytes are checked against the frozen
descriptors and completed native preflight runtime pins. Node is explicitly pinned
and resolved to the canonical executable. Driver/wrapper/capture/observer must
already occur in the immutable helper closure. This new preparation script stays
outside that closure and is separately pinned by its manifest and coordinator;
do not amend frozen bindings merely to add it. The auditor checks the complete
actual build command, raw outputs, final launch chain and all evidence afterward.

Drafted source-only. No execution, imports or tests performed by the author.
