# Output-interval diagnostic checkpoint

At HEAD `069d62d7`, the current uninstrumented candidate passed all 14 exact
repository-check stages serially under independent clean guards. This is a
complete staged check on one source freeze, not one uninterrupted invocation.
The six-arm baseline/candidate native/WASI SIMD/WASI scalar run passed all
renderer tests (231 baseline, 238 candidate including two diagnostic tests),
eight size/alignment comparisons and 47 ordered error/resource records per arm.
All observed layouts and same-platform transcripts agree. The two direct-root
precision host fixtures pass with native/WASI byte parity (RGB 219 and 5).

The original R1 route test failed its six-row Competitive prediction while raw
pixels passed. Its result remains failed. The separately frozen R2 passed all
24 requests, including fresh height, placement and boundary controls, and
confirmed the added row has zero coverage. All temporary instrumentation is
removed with exact source restoration. Native and WASI benchmark builds for
both arms completed. There is no incremental speed or RSS result yet; final
quality assembly, the 104-output preflight and fixed performance phases follow.
See `../../n16-painted-interval-validation.json` for terminal record hashes.

The initial candidate build found E0716 in the new public test's temporary
expected-pixel array. A local array binding corrects that test; no production
behavior changed. The original failure is retained.

| Attempt | Outcome |
| --- | --- |
| diagnostics-001 / run.py | Baseline native and WASI SIMD tests passed; parser rejected an empty WASI `std::env::consts::OS` label |
| diagnostics-002 / run_v2.py | Baseline passed all 231 tests on native, WASI SIMD and scalar; candidate artifact was actually the baseline binary, caught by missing required candidate tests |
| diagnostics-003 / run_v3.py | Fresh separate arm/platform directories; canceled when an external workspace check started |
| diagnostics-004 / run_v3.py | Same isolated procedure; canceled when an external release build started |
| candidate-native-001 | Actual uninstrumented candidate passed all 236 native tests, including both new public interval tests |
| full-check-001 | External compute triggered cancellation; no completed repository check or returned combined log |
| full-check-002 | Policy, fixtures, bundled data and formatting passed; Clippy rejected two new empty-diagnostics assertions, subsequently corrected |
| full-check-003/004/006 | External compute triggered cancellation; no completed check |
| full-check-005 | External compute refused launch before the check started |
| focused-clippy-001 | External compute interrupted the focused lint check |
| focused-clippy-002 | Current renderer and all its targets pass Clippy with warnings denied; completed receipt and clean guard |
| reach-unit-001 | External compute refused the four-test parser/oracle suite before execution |
| reach-unit-002 | All four original parser tests passed |
| full-check-007 | External compute interrupted the whole check; no final pass |
| staged-check-001 | All 14 exact authoritative stages passed on one freeze, with clean final receipts/guards |
| diagnostics-005 | External Cargo check interrupted baseline native testing; no completed matrix |
| diagnostics-006 | All six fresh isolated arms passed renderer, layout and error/resource checks |
| precision-host-001 | Both direct rational fixtures pass and native/WASI raw responses match |
| reach-001 / R1 | Competitive opaque64 submitted seven rows; fixed six-row hypothesis failed after pixel equality passed |
| reach-r2-unit-001 | All nine revised parser/oracle tests passed |
| reach-r2-001 / R2 | All 24 revised structural and independent-pixel requests passed; exact source restoration follows |
| native/wasi build-001 | Both baseline/candidate uninstrumented benchmark builds completed under clean guards |

Do not interpret the candidate observation in diagnostics-002 as candidate
validation: Cargo reported its manifest path and a fresh artifact, but the
shared output directory returned the baseline executable hash and test inventory.
The explicit required-test check prevented acceptance. All future builds use
separate directories per arm and target. Historical start/current measurements
used distinct binary identities and remain separate evidence.

`run_v3.py` produced the completed six-arm result. It requires a fresh target root and
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
The candidate's production/test delta remains uncommitted. After the assertion
lint repair its source identity is
`53062678753e6012a291b95dbf07cb8b069e7134ab836d587919975d1ed057c7`.
The successful diagnostic preparation/removal is `diagnostic-preparation-004`;
R1 and R2 have separate preparation/restoration records. The exact source and
helper pins of each temporary variant remain distinct from timed product builds.

Original R1 sources remain under `../n16-painted-interval-reach/`. R2 sources,
fixed requests, independent fresh fixtures and nine pure tests are separately
archived under `../n16-painted-interval-reach-r2/`. Its 24 native full requests
prove neither all-52-cell nor WASI structural reach. Diagnostic timings cannot
be used for performance claims. `quality-assembly/` describes the final evidence
recheck and contains the production correspondence review. Missing or changed
prerequisites block pilot; no test count substitutes for those checks.
