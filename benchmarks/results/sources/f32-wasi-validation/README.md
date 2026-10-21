# F32 interval WASI diagnostic and scalar accuracy preflight

Off-tree adaptation of the retained `n16_painted_interval_wasi.py`; no source was
executed, imported, built, formatted or tested while drafting. The copied
`run_profile_wasi.mjs` is unchanged. No production changes or new framework.

Install the new driver and optional six-test protocol/binding module in candidate
`benchmarks/` before freezing native preflight-inputs bindings. Keep the existing
wrapper unchanged. Bind the driver, wrapper, F32 fixture helpers and complete
listed historical helper closure in `bound_files_sha256`. The collector checks
those files against the preflight-inputs bindings and rehashes them at the end.
Both SIMD and scalar run before final quality/native-admission bindings. Their
audited results are later pinned by final admission, preserving one immutable
source/plan/helper core without circular prerequisites.

The fixed matrix is `opaque64`, `sparse64`, `image-only`, `f32-shading-dense`,
`f32-pattern-dense`, `default-knockout64`, each Exact then Competitive, all full at72dpi.
SIMD uses seed20261111,2 warmup+12 measured pairs/cell,6AB/6BA measured orders:
336 verified outputs. Scalar uses one AB pair/cell, no warmup or timing summary:
24 verified outputs. No retry, replacement samples or configurable counts.

Each invocation gets a fresh default-tiering Node/WASI instance. The scalar
program still emits internal stages; they are retained as raw diagnostics and
never summarized as timing evidence. SIMD `render_ns` medians/p95/paired ratios
are descriptive only, with no calibrated inference, speed/RSS gate or admission.
Exported linear-memory size is not peak memory or Node RSS. The unchanged Rust
example uses default Repair/full compile and does not report PDF diagnostics.

## Build prerequisites (root executes serially)

Use four distinct target directories: baseline/candidate × SIMD/scalar. This is
mandatory: prior shared-target builds reused the wrong worktree's artifact.
For each arm and feature, run the existing owned runner/strict observer around
`ABS_PYTHON -B PINNED_CAPTURE_BUILD NEW_CONTEXT_JSON --` followed by:

```
/usr/bin/env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS='-C target-feature=+simd128' \
  /Users/jonasgrosch/.cargo/bin/cargo build --locked --offline --release \
  -p pagura-render --example scene_index_profile --target wasm32-wasip1 \
  --target-dir ABS_UNIQUE_ARM_SIMD_TARGET -j 1 --message-format=json-render-diagnostics
```

Scalar substitutes `-simd128` and its own target directory. The exact command
above (including `/usr/bin/env`, absolute Cargo, explicit target directory and
Cargo JSON format) must be the suffix of the actual final build receipt argv;
the existing capture helper may precede it. Record the actual effective command
and environment in a separate context JSON with `schema:1`, `root`, `command`,
`CARGO_TARGET_DIR` (canonical absolute path), `CARGO_BUILD_JOBS:"1"`, `RUSTFLAGS`
and `CARGO_ENCODED_RUSTFLAGS:null`, plus exact `.cargo/config.toml` path/SHA.
The wrapper rejects nonempty ambient `CARGO_*`/`RUST*` overrides other than the
four explicitly replaced variables; clear wrappers/toolchain/profile overrides
before running. The final build receipt must bind this context
file and module. The capture wrapper records actual argv and effective explicit
env overrides, then `os.execv`s the command; capture does not mean build success.
It sets `CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS` to the identical explicit Cargo
flag values before recording. The collector rejects mismatched context/argv/feature/module
paths and shared arm target directories. The scalar target directories must
also differ from the SIMD directories (coordinator cross-run check).

Select the example
artifact from each successful invocation's Cargo JSON, hash/archive it, and
bind it to that build's completed final receipt. Review the actual command,
environment, pinned toolchain, unchanged example and source before declaring
the feature fields in a completed copy of `build-bindings.template.json`.
Do not derive source-to-binary identity from a filename or this declaration.
Use one independently pinned build-bindings JSON per feature.

## Invocation (root supplies frozen absolute paths and hashes)

Run direct under the existing runner and strict observer, with its exported
`PAGURA_RUN_ID` / `PAGURA_RUN_RECEIPT`; no new launcher is needed:

```
ABS_PYTHON -B CANDIDATE/benchmarks/f32_painted_interval_wasi.py \
  --runtime-profile simd \
  --baseline-source BASELINE --candidate-source CANDIDATE \
  --baseline-wasm BASELINE_SIMD_WASM --candidate-wasm CANDIDATE_SIMD_WASM \
  --baseline-build-receipt BASELINE_SIMD_RECEIPT \
  --candidate-build-receipt CANDIDATE_SIMD_RECEIPT \
  --bindings PREFLIGHT_INPUTS_BINDINGS --bindings-sha256 HEX \
  --build-bindings SIMD_BUILD_BINDINGS --build-bindings-sha256 HEX \
  --plan FROZEN_F32_NATIVE_PLAN --plan-sha256 HEX \
  --node ABS_NODE --output NEW_OUTPUT_DIRECTORY
```

Scalar uses `--runtime-profile scalar`, scalar modules/receipts/build bindings,
and a separate fresh output directory. The runner assigns `PAGURA_RUN_ID` and
`PAGURA_RUN_RECEIPT` at launch; no run ID is frozen in producer argv. `NODE_OPTIONS` and
`NODE_V8_COVERAGE` must be unset. Before launch, externally pin Python launcher
and actual framework executable, Node, complete argv, driver, wrapper, plan,
preflight-inputs bindings and feature-specific completed build binding.

The report retains each attempt before launch, hashes full raw RGBA, exact
wrapper arguments/preopens, stdout/stderr/Node logs and module/input/wrapper
bytes. It requires strict independent RGB, opaque alpha and same-mode arm byte
equality for every warmup and measured/preflight output. Empty/timing-invalid
or failed children stop without a replacement. Partial artifacts are retained.
The fixed historical fixture oracles are reused; dense black alpha1/4 gives
RGB191/alpha255. Default-knockout64 is the source-identified F64 control in both
modes; Exact shading-pattern is another F64 control. The nested64 native control
stays in the unchanged56-cell native matrix and is not mislabelled F64 here.

## Validation boundaries and remaining coordinator work

- `status=completed`, `admission=false`, `verified_outputs=336` (SIMD) or24
  (scalar) requires all fixed rows, strict bytes, source/module/helper end pins.
- Final outer receipt and clean strict observer must independently authenticate
  report, retained plan and exact direct producer argv. The collector's live
  receipt pointer is not final evidence and is not used as proof.
- Build flag checks include receipt-bound actual command/environment context,
  exact env overrides in receipt argv, target paths and module bytes. Capture
  helper integrity, toolchain and fresh Cargo artifact selection remain reviewed
  external build evidence; the collector does not pretend to infer machine-code
  feature use from a filename or scan arbitrary WASM bytes for SIMD opcodes.
- The supplied independent `audit_f32_painted_interval_wasi.py` reconstructs the
  fixed pixels/schedule without producer imports and verifies each raw log,
  output, build context, runtime and final receipt/observer binding. Freeze a
  completed `audit-pins.template.json` before each run, including all six PDF
  hashes from existing frozen fixture preparation/native preflight and exact
  producer argv. The report's assigned run ID must match the final receipt;
  Python framework handoff files must be prelaunch pinned.
- After final receipt/guard completion, run the new auditor under the existing
  coordinator with `--directory RUN_DIR --receipt FINAL_RECEIPT --receipt-sha256
  HEX --guard FINAL_GUARD --guard-sha256 HEX --pins FROZEN_AUDIT_PINS
  --pins-sha256 HEX --output NEW_AUDIT_JSON`. Final producer receipt must pin
  `report.json`, `plan.json` and `guard.json`; the four component tests do not
  substitute for this complete retained-run audit. The old N16 auditor must
  not certify this different matrix/launch schema unchanged.
- Successful audit exposes `status:verified`, `runtime_profile`, counts336/24,
  receipt/report/guard/native-bindings/frozen-plan hashes, and
  `background_contention_observed:false`, always `admission:false`. It emits no
  old suspension/fallback fields or scalar timing summaries. The final native
  binding can pin both audits plus an explicit coordinator diagnostic review;
  no timing admission is inferred from a missing field.
- Comparing strict independent opaque RGBA across both modules establishes
  equality for these12 cells only. Native112 preflight, six-arm lib tests,
  resource/layout/error contracts and the56-cell native experiment remain
  separate prerequisites. No global portability or performance claim follows.
