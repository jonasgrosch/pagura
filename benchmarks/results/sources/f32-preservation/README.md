# F32 preservation experiment

The frozen protocol is `../../f32-preservation-cost-plan.json`; the completed
result is `../../f32-preservation-cost.md` with a JSON companion. This experiment
retains an accuracy correction, not a confidence-bound performance admission.

- `build.py` builds both native arms serially, selects Cargo's emitted native
  artifact, and freezes binary/source/configuration identities.
- `cost.py` first checks all 76 outputs, then runs the one fixed 2,736-output
  diagnostic. Candidate output must meet the independent reference; baseline
  mismatches are retained and must remain byte-stable. No output is regenerated
  to obtain a favorable subset. The cost preflight was pinned before launch.
- `audit.py` imports no producer code. It independently parses the retained
  P6/RGBA bytes, checks the complete schedule, all raw stages/RSS and summaries,
  hashes and final receipt-to-observer-to-producer command linkage. It ran before
  either source tree changed. Its separately frozen final pins are
  `../../f32-preservation-audit-pins.json`; this is retrospective evidence binding,
  not a claim that postflight snapshot hashes existed before measurement.
- `check.py` uses the existing owned-process lifecycle with bounded time/logs for
  required validations. `run_wasi_tests.mjs` executes the Cargo-selected WASI test
  module with the workspace preopened for the test helper's absolute fixture
  paths. It does not measure WASI performance.
- `summarize.py` rechecks completed receipt/artifact hashes and retains every
  audited cell in the JSON/Markdown result. No statistics are used for admission.

The coordinator alone executed all local jobs, serially under the existing quiet
preflight, cooperative runner lock and strict known-process observer. These
mechanisms do not establish OS-level exclusivity. Agent reviews were static.
The source identities describe the uncommitted product overlay on `e45a3308`
and pristine product baseline `c70b4678`; a later commit changes the HEAD identity
without retroactively changing the recorded experiment. Replaying the auditor's
live-source checks requires the originally bound source state.

The first native renderer attempt retained one private diagnostic-message
failure. The repaired test still rejects the same pixel limit before the decoded
budget in both modes. The final native and WASI validations pass. An initial
build launch rejected an unsupported runner kind before any build began. The
measured trial was launched once and never retried.

Raw artifacts remain under `../../local/f32-preservation/`. Full source/binary,
input/reference and command hashes are retained; the audit is not a rebuild or
signature attestation. Fixture policies, larger dynamic/shading scenes, coverage
approximations, transparent output and global PDF correctness remain distinct
questions. No competitor or browser/WASI speed claim follows from this trial.
