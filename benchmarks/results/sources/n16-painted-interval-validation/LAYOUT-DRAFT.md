# Temporary N16 enclosing-layout diagnostic

Source-only draft. Nothing here has been applied, imported, built or executed.
Run only after the frozen start/current campaign has completed and been audited,
under the coordinator's existing serial runner and strict observer. This probe
is a layout observation, not a speed, RSS, peak-stack or accuracy test.

`layout-probe.patch` adds one `cfg(test)` function to `compositor.rs`. It prints
the real `size_of` and `align_of` of Buffer, Option<Buffer>, Surface,
MaskSink<'static> and Context, plus three representation controls. All types
are accessible from that module; no visibility, fields, cfgs or constructors
change. No renderer state is constructed. The lifetime choice has no layout
effect. Context is reported even though the proposed interval is not stored
directly in Context. Test-only code is absent from ordinary product builds.

The observed candidate checkout HEAD was `210287a7` (isolated-experiment docs),
with the interval source delta on the retained `a9814ee4` product. The baseline
must be a separately created clean `a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`
checkout. Never put the probe in the frozen renderer-stacked-gains checkout.
The baseline path below is a proposed name; substitute the coordinator's
actual dedicated control path in the frozen command record before execution.

## Freeze and prepare

1. Record both roots, HEAD/tree and complete dirty/untracked inventory, current
   source_digest using the already reviewed recipe, toolchain (`rustc -vV`,
   Cargo, Node), resolved runtime files and relevant build environment. Save
   exact before-probe `compositor.rs` bytes/hash independently for each root.
   The clean control must contain no product delta from a981. The candidate
   delta must match the reviewed interval; preserve its other authorized
   untracked harness files. Do not stage or reset either checkout.
2. Retain/hash this patch, this README, `select_artifact.py`, and the existing
   `benchmarks/results/sources/main-reconciliation/run_wasi_tests.mjs`. Copy
   that existing wrapper unchanged into the new evidence directory and pin it.
   Its Node WASI imports, test argv and module hash are already explicit.
3. Apply this identical patch to each root with `git apply --check` followed
   by `git apply`. Record the source digest with probe installed. The only
   new delta must be this one function. Do not format or modify product code
   as part of the diagnostic.

Use absolute, coordinator-pinned paths. The following are command payloads
for the existing owned-process runner, not permission to run jobs in parallel.
Keep stdout/stderr and exit status for every command, including failures.
Shell examples use `set -e` so a failed build/selection/run stops that sequence.

```sh
set -eu
PAGURA_LAYOUT_BASE=/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval-base/pagura
PAGURA_LAYOUT_CANDIDATE=/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura
PAGURA_LAYOUT_DRAFT=/private/tmp/n16-layout-probe
PAGURA_LAYOUT_EVIDENCE=/ABSOLUTE/NEW/EVIDENCE/DIRECTORY
PAGURA_LAYOUT_CARGO=/ABSOLUTE/PINNED/cargo
PAGURA_LAYOUT_PYTHON=/ABSOLUTE/PINNED/python3
PAGURA_LAYOUT_NODE=/ABSOLUTE/PINNED/node
PAGURA_LAYOUT_TARGET=/ABSOLUTE/SHARED/LAYOUT/TARGET
PAGURA_LAYOUT_TEST=compositor::temporary_n16_layout_probe

git -C "$PAGURA_LAYOUT_BASE" apply --check "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_BASE" apply "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_CANDIDATE" apply --check "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_CANDIDATE" apply "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
```

Create the evidence directory and its baseline/candidate subdirectories once,
outside both source roots. Set PAGURA_LAYOUT_TARGET to a dedicated absolute
build directory, with separate native, wasi-simd and wasi-scalar subdirectories.
Both roots may use those same three directories **sequentially**. Cargo owns
dependency reuse and fresh artifact selection; never copy fingerprints, use a
deps glob, or assume a previous executable belongs to the new arm. Distinct
source roots may still force dependency rebuilds. This builds only the render
crate's library test target, not every workspace target or a host binary.

Before building, reject unexpected inherited CARGO_PROFILE_*, CARGO_TARGET_*,
RUSTC/RUSTC_WRAPPER/RUSTC_WORKSPACE_WRAPPER, RUSTUP_TOOLCHAIN, CARGO_HOME config
or target overrides. Keep the documented toolchain and release profile. The
commands explicitly clear flag variables that can override .cargo/config.toml;
the coordinator must pin the complete effective environment, not infer it
solely from this listing. Clear NODE_OPTIONS and NODE_V8_COVERAGE for Node.

## Six serial build/run observations

Run this entire block once with ROOT=BASE/ROLE=baseline, then once with
ROOT=CANDIDATE/ROLE=candidate. Archive each freshly selected executable/module
before another build can replace the same Cargo target path. No Cargo invocation
is repeated merely to execute the test. Three build variants per arm suffice.

```sh
PAGURA_LAYOUT_ROOT="$PAGURA_LAYOUT_BASE"
PAGURA_LAYOUT_ROLE=baseline
# For the second pass, use ROOT="$PAGURA_LAYOUT_CANDIDATE" and ROLE=candidate.
cd "$PAGURA_LAYOUT_ROOT"

env -u CARGO_ENCODED_RUSTFLAGS -u RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
  -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR CARGO_BUILD_JOBS=1 \
  "$PAGURA_LAYOUT_CARGO" test --locked --offline --release -p pagura-render \
  --lib --no-run --message-format=json --target-dir "$PAGURA_LAYOUT_TARGET/native" \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native-build.jsonl" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native-build.stderr"
"$PAGURA_LAYOUT_PYTHON" -B "$PAGURA_LAYOUT_DRAFT/select_artifact.py" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native-build.jsonl" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native-test"
"$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native-test" \
  "$PAGURA_LAYOUT_TEST" --exact --nocapture --test-threads=1 \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native.stdout" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/native.stderr"

env -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
  -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR CARGO_BUILD_JOBS=1 \
  RUSTFLAGS='-C target-feature=+simd128' \
  "$PAGURA_LAYOUT_CARGO" test --locked --offline --release -p pagura-render \
  --lib --no-run --message-format=json --target wasm32-wasip1 \
  --target-dir "$PAGURA_LAYOUT_TARGET/wasi-simd" \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd-build.jsonl" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd-build.stderr"
"$PAGURA_LAYOUT_PYTHON" -B "$PAGURA_LAYOUT_DRAFT/select_artifact.py" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd-build.jsonl" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd-test.wasm"
env -u NODE_OPTIONS -u NODE_V8_COVERAGE "$PAGURA_LAYOUT_NODE" \
  "$PAGURA_LAYOUT_EVIDENCE/run_wasi_tests.mjs" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd-test.wasm" \
  "$PAGURA_LAYOUT_TEST" --exact --nocapture --test-threads=1 \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd.stdout" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-simd.stderr"

env -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
  -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR CARGO_BUILD_JOBS=1 \
  RUSTFLAGS='-C target-feature=-simd128' \
  "$PAGURA_LAYOUT_CARGO" test --locked --offline --release -p pagura-render \
  --lib --no-run --message-format=json --target wasm32-wasip1 \
  --target-dir "$PAGURA_LAYOUT_TARGET/wasi-scalar" \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar-build.jsonl" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar-build.stderr"
"$PAGURA_LAYOUT_PYTHON" -B "$PAGURA_LAYOUT_DRAFT/select_artifact.py" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar-build.jsonl" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar-test.wasm"
env -u NODE_OPTIONS -u NODE_V8_COVERAGE "$PAGURA_LAYOUT_NODE" \
  "$PAGURA_LAYOUT_EVIDENCE/run_wasi_tests.mjs" \
  "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar-test.wasm" \
  "$PAGURA_LAYOUT_TEST" --exact --nocapture --test-threads=1 \
  > "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar.stdout" \
  2> "$PAGURA_LAYOUT_EVIDENCE/$PAGURA_LAYOUT_ROLE/wasi-scalar.stderr"
```

Each run must exit zero and report exactly one passed test and eight uniquely
named PAGURA_N16_LAYOUT_V1 rows. Zero tests is not success. Require the expected
native arch/OS/pointer width, and wasm32/wasi/32 for both WASI variants; SIMD
must report simd128=true and scalar false. Match each WASI wrapper module hash
and test_args to the archived module and the exact filter/options above.
For all five enclosing types retain actual size/alignment for both arms and
their signed deltas, separately on all three platforms. Compare the controls
too. A passing test does not assert equal layouts: growth must remain visible
and is a rejection under the currently frozen no-growth gate. Do not waive it
because the allocation counter or vector payload is unchanged.

## Remove, verify and retain

Before removal, record/hash the post-run source inventory and verify it equals
the probe-installed pre-build inventory. Then remove only this probe:

```sh
git -C "$PAGURA_LAYOUT_BASE" apply --reverse --check "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_BASE" apply --reverse "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_CANDIDATE" apply --reverse --check "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
git -C "$PAGURA_LAYOUT_CANDIDATE" apply --reverse "$PAGURA_LAYOUT_DRAFT/layout-probe.patch"
```

Check exact compositor.rs bytes/hash and full source inventory against each
root's own pre-probe state. Baseline returns clean a981; candidate retains its
original interval delta and harness files. No reset, clean or wholesale restore
is needed. If a failure interrupts removal, identify the remaining temporary
function explicitly and complete removal before later product validation or
benchmark freezing. Preserve failed attempts instead of replacing their files.

Freeze a final coordinator receipt/guard that binds all raw build/run logs,
archived modules/executable and their selection records, toolchain/environment,
patch/helper/wrapper hashes, six layout observations, and before/installed/after
source identities. Live receipt snapshots are not final execution evidence.
The final product binaries must be selected/built after probe removal. These
test executables are diagnostic artifacts and must never serve as benchmark
renderer identities. No performance or total-memory claim follows from this
probe; callback code generation, stack frames and dynamic allocation remain
separate checks.
