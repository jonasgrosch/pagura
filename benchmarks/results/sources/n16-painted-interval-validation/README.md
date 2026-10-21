# Output-interval diagnostic checkpoint

The candidate remains unvalidated. The 14 benchmark-harness unit tests passed,
but there is no accepted candidate renderer test/layout result and no incremental
performance sample. See `../../n16-painted-interval-validation.json` for terminal
record hashes and the restored source identities.

The initial candidate build found E0716 in the new public test's temporary
expected-pixel array. A local array binding corrects that test; no production
behavior changed. The original failure is retained.

| Attempt | Outcome |
| --- | --- |
| diagnostics-001 / run.py | Baseline native and WASI SIMD tests passed; parser rejected an empty WASI `std::env::consts::OS` label |
| diagnostics-002 / run_v2.py | Baseline passed all 231 tests on native, WASI SIMD and scalar; candidate artifact was actually the baseline binary, caught by missing required candidate tests |
| diagnostics-003 / run_v3.py | Fresh separate arm/platform directories; canceled when an external workspace check started |
| diagnostics-004 / run_v3.py | Same isolated procedure; canceled when an external release build started |

Do not interpret the candidate observation in diagnostics-002 as candidate
validation: Cargo reported its manifest path and a fresh artifact, but the
shared output directory returned the baseline executable hash and test inventory.
The explicit required-test check prevented acceptance. All future builds use
separate directories per arm and target. Historical start/current measurements
used distinct binary identities and remain separate evidence.

`run_v3.py` is the next execution driver. It requires a fresh target root and
creates baseline/candidate children with native, wasi-simd and wasi-scalar
directories. It builds and runs serially, checks all renderer tests, eight
layout rows and 47 error/work-counter records, and requires the candidate's
new public tests by name. The empty WASI OS label is checked alongside the
pinned compiler target cfg, Cargo target, module execution and SIMD flag.
The separate WASI wrapper exposes only the exact arm root; this is a filesystem
preopen, not a read-only capability. Sources and generated fixtures are hashed
before and after execution. A final guard/receipt is required independently.

`run.py` and `run_v2.py` are preserved executed revisions, not alternative
acceptance paths. `LAYOUT-DRAFT.md`, the original selector and PI-Q4 notes are
historical source preparation; their shared-target suggestion is superseded by
the observed artifact collision. Raw patch context whitespace is intentional
and retained byte-for-byte with its original hash.

Both temporary probes have been removed. Baseline is clean `a9814ee4`, source
`b3a124d48e4761b66a717edb542ceebf56ffa91b0e3f702f997b005d1a30a2c7`.
The candidate's formatted production/test delta remains uncommitted, source
`f3bc6749d2d33c8b5248fd74489a679609ef8d538c9f612be41e0330bc6e37a6`.
Record a fresh preparation identity and reinstall the exact temporary patches
before the next diagnostic run; do not reuse a pre-probe HEAD after doc commits.
Remove only those patches afterward and verify restoration again. Timed product
binaries must be built after removal. Full repository validation, actual fixture
route/reach observations, independent pixels and the fixed performance phases
are still outstanding.
