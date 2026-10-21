# F32 quality assembly — unexecuted draft

`assemble.py` only reads pinned retained evidence and writes one NEW quality JSON.
It never builds, tests, renders, measures or launches subprocess work directly;
the existing `verify_core` performs its read-only Git/source identity checks.
Root alone executes it under the existing runner and strict observer.

Install the source before freezing the immutable input binding/helper closure.
Invoke with the pinned actual Python app launcher, with this exact shape:

```
PYTHON ASSEMBLE --root ROOT --request REQUEST --request-sha256 SHA --output NEW_QUALITY_JSON
```

Every descriptor is a canonical absolute `{path,sha256}`. The root-reviewed,
independently SHA-pinned request contains:

```
schema: 1
experiment: f32-painted-interval
status: reviewed-for-quality-assembly
review_reason: nonempty coordinator assessment
input_bindings: descriptor of immutable stage=preflight-inputs bindings
helpers: {assembler: descriptor, check: descriptor of actual tools/check.py,
          diagnostics: descriptor of archived F32 run_v3.py}
evidence: {
  repository_validation: JOB whose artifact is the actual full-check raw log,
  six_arm_diagnostics: JOB whose artifact is the actual six-arm report.json,
  route_reach: JOB whose artifact is the root-reviewed route report
}
diagnostic_runner_source: actual instrumented outer-runner snapshot
diagnostic_preparation: {before: descriptor, installed: descriptor, removed: descriptor}
route_log: descriptor of retained actual focused Cargo test log
route_restorations: nonempty list of owned restoration-record descriptors
source_review: descriptor of bounded product/source/error/resource review
route_review: {
  reviewed: true, reason: nonempty text,
  before_product: clean candidate {root,commit,source_sha256},
  after_product: same exact clean product identity,
  installed_runner_source: actual instrumented route receipt source snapshot,
  before_runner_source: clean frozen candidate runner snapshot,
  after_runner_source: same clean runner snapshot,
  owned_patch_restoration_verified: true,
  records: nonempty descriptor list pinning actual Cargo JSON/binary artifact
           identity, six-PDF input manifest, before/installed/removed records,
           owned patch and route test source
}
```

JOB is `{receipt:descriptor,guard:descriptor,artifact:descriptor,
producer_argv:[actual child argv...]}`. Its final successful receipt must bind the
raw artifact and strict guard. The observer command matches producer_argv exactly.
Only the two known observer wrapper path positions resolve against verified
receipt.cwd==ROOT; argv0 Python, flags and child suffix stay exact. This accepts
the actual full-check's retained relative paths without rewriting them.

The helper derives the exact14 Checking command lines from pinned check.py with
jobs=1. It requires their ordered equality and one final success marker, followed
only by blank lines or the exact known completed-observer footer. It reuses the
pinned six-arm parser on every actual raw test log: six distinct arms/platforms,
47 Q4 records each, same-platform equality, required candidate tests,24 measured
layout size/alignment rows and nonpositive deltas. It checks the known
before→installed→report→removed schema and current restored source-file hashes.

The route parser checks the source-declared12 first-chunk rows, exact format,
side/range/call/pixel counts and path/mask/dynamic route, with the focused test's
1passed/243filtered successful summary. It does not invent a report schema:
actual route build/input/preparation/restoration linkage is the explicit reviewed
request boundary above. Root must inspect the pinned raw records and exact owned
patch restoration before setting those assertions. Current clean product and
runner/build identities are independently reread through verify_core.

The output matches the admission helper's seven named gates and24 layout rows.
All input artifacts are rehashed before writing. Output creation is exclusive;
an interrupted or failed outer job is never usable even if a file exists.
The final clean quality receipt MUST bind the output and EVERY descriptor in its
`restorations` list; also retain request/source/helper pins and the guard. No
attempt to embed that not-yet-final receipt in the quality artifact is made.

Native112 and SIMD336/scalar24 remain separate mandatory `verify_admission`
checks. This scoped quality artifact grants no timing/retention/release admission,
RSS claim or general all-input proof. No draft has been executed or imported by
its author; root owns syntax/tests and all actual assembly.
