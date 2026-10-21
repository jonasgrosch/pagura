# F32 native admission handoff — unexecuted source only

Historical helpers stay unchanged. Install `f32_interval_admission.py`,
`launch_f32_painted_interval.py`, `evaluate_f32_painted_interval.py` and the updated
`f32_painted_interval.py` in `benchmarks/`. The driver changes only by recording
its actual post-exec argv. Install `native_preflight.py` as
`benchmarks/results/sources/f32-native-preflight/run.py`; put `admit.py` and
`capture_native_build.py` beside it. `plan.template.json` remains non-executable
until root fills actual identities/cells and freezes it. No results are assumed.

## Immutable sequence

1. Finish product validation and exactly remove diagnostic overlays. Build both
   native arms using the capture helper below; pin actual binaries and terminal
   receipts/guards/contexts. Baseline must be pristine retained f9738174.
2. Freeze ONE canonical `f32-painted-interval-plan.json` BEFORE preflight, with
   actual identities, 20 cases/56 cells/roles, original phases, gates and policy.
   Set status `frozen-before-pilot`; never modify its bytes after this point.
3. Create `f32-painted-interval-preflight-bindings.json`, stage `preflight-inputs`.
   Native112, SIMD336 and scalar24 ALL bind this same immutable file. Both WASI
   runs precede native pilot, with descriptive timing only.
4. Assemble the completed quality manifest and immutable admission request.
   Run `admit.py`; it verifies the native raw outputs and all completed evidence,
   then exclusively creates canonical `f32-painted-interval-bindings.json`,
   stage `native-admission`, copying the frozen input core exactly.
5. A pilot config pins that final binding. After its complete independent audit
   and explicit review, a separate confirmation config adds only the pilot
   audit. Both phases use the same plan/binding. Maximum one launched phase each;
   no historical samples, automatic retry, altered cells or altered thresholds.

A source correction requires a new declared freeze. All earlier artifacts remain.

## Core and evidence schema

Descriptors are canonical absolute `{path,sha256}`. Input binding fields:
`schema:1`, `experiment:'f32-painted-interval'`, `stage:'preflight-inputs'`,
`plan`, `identities:{baseline,candidate:{root,commit,source_sha256,binary,binary_sha256}}`,
`runner_sources:{baseline,candidate}`, `candidate_runner_source_sha256`,
`python`, `python_executable`, `guard`, `bound_files_sha256`, and
`native_builds:{baseline,candidate:{receipt:PATH,receipt_sha256:SHA,context:DESCRIPTOR,guard:DESCRIPTOR}}`.
Pin both actual macOS Python app launcher and separately reported sys.executable.
Freeze all real helper/runtime/config imports as well as the validator's required
source set; plan and helper hashes are candidate-root-relative in bound_files.

An evidence entry is `{receipt:DESCRIPTOR,artifact:DESCRIPTOR,guard:DESCRIPTOR}`.
Admission request: schema1, experiment name, status `frozen-for-admission-check`,
`preflight_bindings`, `native_preflight:EVIDENCE`, `quality:EVIDENCE`,
`wasi:{simd:EVIDENCE+audit:DESCRIPTOR,scalar:EVIDENCE+audit:DESCRIPTOR}`, and
`wasi_review:{status:'reviewed-for-native-experiment',reason:NONEMPTY_TEXT}`.
Both WASI audits require verified status, exact profile/count336|24, receipt,
report, guard, INPUT-stage binding and plan SHA fields, and contention false.
There are no old N16 suspension/fallback fields or implied latency admission.

Quality JSON: schema1, experiment name, passed status, clean candidate runner
snapshot in `source`; all seven `gates` passed: repository-validation,
independent-rgba, tile-reuse, errors-resources, layout, route-reach, wasi-portability.
`layouts` has exactly24 `{platform,type,baseline:{size,align},candidate:{size,align},
delta:{size,align}}` rows: native/wasi-simd/wasi-scalar × Buffer, Option<Buffer>,
Surface, MaskSink, Context, Vec<[u16;4]>, Range<usize>, [u16;4]. Exact positive
integers and true candidate-minus-baseline deltas must show no growth.
`evidence` has repository_validation, six_arm_diagnostics and route_reach entries;
artifacts may be raw logs. `restorations` is a nonempty descriptor list ALSO bound
by the final clean quality receipt. Do not require intentional mutation receipts
to claim source unchanged. Root reviews raw-log/diagnostic/route correspondence
and restoration; this small adapter authenticates those explicit conclusions,
not their entire parser implementations. Include bounded source-review limits.

## Exact invocation contracts

Native build (same installed candidate CAPTURE script in either arm cwd):

    PYTHON CAPTURE NEW_CONTEXT_JSON -- ABS_CARGO build --locked --offline --release -p pagura-render --example scene_index_profile --target-dir ABS_TARGET -j 1 --message-format=json-render-diagnostics

Use this directly as the observer child. Context captures fixed argv/env and
capture/Cargo/config hashes; final receipt binds context, guard and binary at
ABS_TARGET/release/examples/scene_index_profile. Equal arm commands may differ
only in explicit target-directory value. Ambient Rust overrides are refused.

Native preflight, as direct observer child with this exact argument order:

    PYTHON ROOT/benchmarks/results/sources/f32-native-preflight/run.py --root ROOT --bindings INPUT_BINDING --bindings-sha256 PIN --output NEW_PREFLIGHT_DIR

Admission (never launches measurements):

    PYTHON ROOT/benchmarks/results/sources/f32-native-preflight/admit.py --root ROOT --request REQUEST --request-sha256 PIN

Phase config: schema1, experiment name, phase pilot|confirmation, frozen-before-launch
status, nonempty decision_reason, canonical plan/final-bindings descriptors and
identical python/python_executable/guard descriptors. Confirmation adds only
`prerequisites.pilot` completed evidence entry plus its audit descriptor.

    PYTHON ROOT/benchmarks/launch_f32_painted_interval.py --config CONFIG --config-sha256 PIN --phase pilot

Add --dry-run only for verification/printing; it cannot write intent or execute.
Real enclosing receipt argv must be `PYTHON GUARD --evidence-dir LOCAL/pilot-001-observer --`
plus that exact launcher command. LOCAL is results/local/f32-painted-interval.
Receipt path is LOCAL/pilot-001-receipt.json. Launcher exclusively writes
pilot-001-launch.json then execs the direct driver. The driver verifies actual
sys.orig_argv, pinned intent and run/receipt and exclusively writes
pilot-001-driver-start.json. For confirmation substitute confirmation in those
positions. Existing output/intent/start refuses another launch.

Final receipt MUST bind config, launch intent, driver start, observer guard and
RUN/report.json. These first two records alone never imply completion.

    PYTHON ROOT/benchmarks/evaluate_f32_painted_interval.py pilot-001 --config CONFIG --config-sha256 PIN

The thin audit closes receipt→guard→launcher→actual driver, then reuses the fixed
shared20/56-cell audit. It verifies all warmup/measured outputs and point estimates;
bootstrap intervals are the pinned runner's estimates, not independently rerun.
It adds no aggregate, gate changes, retention decision, RSS neutrality, release
admission or superiority claim. All sources/tests remain unexecuted by this agent.
