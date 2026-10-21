# N16 painted interval: coordinator checklist

Source-only preparation; nothing below has been executed. Product/test draft:
`painted-interval.patch`, SHA256
`c71d0c06c2f7c621d8c4a67b875e91070b780caa9dc600a71da5d9df7d676ecb`,
based on `a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`. Finish and audit the
frozen start/current campaign before applying this separate candidate. Preserve
its binaries, inputs, reports and gates. This proposal supplies no new results.

## 1. Freeze the actual incremental experiment

- Review/apply only after the coordinator releases the active source freeze.
  Record actual baseline/candidate commits, product digests, patch and build
  receipts; update the base if another product change lands first. Do not reuse
  historical timing as the new baseline or invent candidate identity fields.
- Reconcile `/private/tmp/n16-painted-interval-plan.md` before freezing it: name
  the current patch, not v3; ordinary admitted translucent controls now use F32
  in **both** modes; nonopaque backgrounds and relevant groups use F64. Verify
  actual routes rather than relying on fixture names. Retain direct U8 tests.
- Timed `scene_index_profile` uses default Repair open and full `compile_page`;
  it emits no diagnostics. Strict public correctness probes remain separate.
- Reuse the draft at `/private/tmp/n16-painted-interval-harness-v2/overlay`.
  Do not apply its entire old shared `evaluate_n16_output.py` replacement: the
  current shared helper already has counts, target operation, strict-guard and
  newer `target_cases` support. Preserve all existing callers/defaults and test
  the reconciled experiment wrapper. Historical helpers/evidence stay intact.
- Freeze imported Python closure, literal oracle derivations, example bytes,
  toolchain/config/flags, runtimes, observer/runner, actual executables/modules,
  plan and phase bindings. Runner source and paired-render source hashes are
  distinct schemes. Final receipts must bind reports, logs, outputs and guards.

## 2. Correctness and resource checks before timing

Root runs serially. The existing native command form is:

```sh
cargo test --locked --offline --release -j 1 -p pagura-render --lib FILTER -- --test-threads=1
```

Use these exact filters, then the integrated `python3 -B tools/check.py --jobs 1`:

| Filter | Required invariant |
| --- | --- |
| `compositor::narrow` | Both N16 writers; untouched/empty/invalid/zero/full stores; long stacks; all 65,536 quantizer inputs; dense output despite empty interval; existing F32 bit equality and clip order |
| `compositor::plan::tests` | Existing opaque N16, translucent F32 and group/F64 admission; avoid attributing an image fast-path bypass to narrow output |
| `compositor::output::tests` | Colored initialized output; nonzero origins/strides; neighboring chunks; multirow holes; independent partial-coverage bytes; unchanged work/allocation counters; F32/U8 dense controls |
| `tests::painted_interval` | Both modes, independent integer occupancy, complete irregular 1/255/256/257 partitions, offset x/y seams, final 1px chunks, painted→blank→painted context reuse and changed backgrounds |
| `display::tests::direct_root` | Independent normal accumulation and premultiplied backgrounds with alpha 0/128/254/255; retain F64 background guard |
| `compositor::sources_tests` | Off-region source/stroke failures, source context, shared function-work ledger, depth and /None behavior |
| `tests::compositing_memory_work_and_shared_function_budgets_are_bounded` | Existing work/function/decoded-byte failures |
| `tests::group_bounds_and_page_chunks_fit_a_small_surface_budget` | Bounded chunk/group allocation |
| `tests::recycled_` | Existing storage reuse and retained decoded-byte accounting |
| `tests::competitive_page_surfaces_check_the_pixel_budget_first` | Pixel-limit priority before allocation |

Specific new tests include `n16_fragment_interval_keeps_prior_rows_when_a_later_clip_row_fails`,
`n16_distant_fragment_rows_keep_the_holes_and_full_interval`, and
`n16_page_output_preserves_holes_in_a_multirow_painted_interval`. They distinguish
validated earlier stores from a later error and verify holes, not just endpoints.
Do not replace independent expected bytes with candidate/Exact agreement.

Run the focused narrow/output/public/direct-root checks under WASI SIMD and
scalar too. Existing runner reads module bytes once, prints their SHA256 and
executes those same bytes, with no preopened directories:

```sh
env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS='-C target-feature=+simd128' \
  CARGO_TARGET_WASM32_WASIP1_RUNNER='node /ABS/ROOT/benchmarks/results/sources/main-reconciliation/run_wasi_tests.mjs' \
  cargo test --locked --offline --release -j 1 -p pagura-render --lib \
  --target wasm32-wasip1 --target-dir target/n16-interval-wasi-simd FILTER -- --test-threads=1
```

Repeat with `-simd128` and a separate `target/n16-interval-wasi-scalar` directory.
Pin actual Node and wrapper bytes, module hash and forwarded test arguments.
These are coordinator commands, not execution evidence or new dependencies.

## 3. Actual enclosing layouts and interval reach

- Use the same temporary internal test probe in baseline and candidate to print
  `std::mem::{size_of,align_of}` for actual `Buffer`, `Option<Buffer>`, `Surface`,
  `MaskSink<'_>` and `Context`; additionally record old Vec/new Words and pointer
  width. Put it in the owning compositor module to access private types. Run
  with `--nocapture` natively and through the hashed WASI test runner above.
- Use actual target builds, identical profile/toolchain/config, and check no
  layout-affecting test-only fields differ from production definitions. A mirror
  enum, source field order, 32-bit arithmetic or assumed niche is not proof.
  Words adds two usize fields; estimated 40/20 bytes is not an observation.
- The existing quality-record schema requires eight size/alignment rows:
  native/WASI × Buffer/Option/Surface/MaskSink. Retain Context as an additional
  enclosing-stack observation in the raw evidence; do not mislabel it as one of
  those eight rows. Check both WASI feature configurations. No enclosing growth
  is the proposed gate; missing evidence is unavailable, not neutral.
- Probe source/results are diagnostic evidence. Remove probes and verify the
  intended product digest before building/pinning timed binaries. A final
  source-unchanged summary receipt can bind prior logs, but must not claim that
  the summary job itself ran their checks. See harness `QUALITY-RECORD.md`.
- Untimed instrumentation must establish actual N16 dispatch and interval reach
  for both writers, plus F32/F64 controls. Confirm same pixel_count*8 allocation,
  no extra heap allocation and unchanged charges. Inspect optimized F32 mask
  code for the unused callback-position overhead; source equality is insufficient.

## 4. Fixed controls and executable entry points

Native matrix stays **18 cases / 52 cells / 2 targets / 50 protected**: the
existing 16-case/46-cell matrix is unchanged; only empty and distant are new
cases, because opaque-tiny already had cold cells. All six additions are full
renders in both modes and protected; no post-result role changes.

| Control | Expected reach and purpose |
| --- | --- |
| `opaque64/full`, both modes | Original targets: sparse six-row strip per 256² chunk; analytical conversion reach 2.34375%, initialization still dense; verify actual route/reach |
| `opaque-empty/full`, both | 512² all white, empty interval; fixed per-chunk/row overhead |
| `opaque-distant/full`, both | Eight literal black pixels at each 256² chunk's device first/last pixel; **full** flat interval, all holes still converted |
| `opaque-tiny/full`, both | Existing 64² fixture, black x24..31/y32..39; protects small render overhead in addition to old cold cells |
| Existing `dense64`, `opaque-fill`, `opaque-clip` | Dense/full-output cost and clipped writes; protect no-saving cases |
| Existing cold/tile/tiled, short glyph spans, image-only, knockout/group cells | Compilation, both writer paths, tile independence, F32/F64/fast-path protected costs |

Draft fixture entry: `benchmarks/n16_painted_interval_fixtures.py::fixture`.
Native driver: `benchmarks/n16_painted_interval.py`, reusing `paired_render`.
Reconciled helper tests: `python3 -m unittest discover -s benchmarks -p test_n16_painted_interval.py`
with bytecode disabled. These paths are currently off-tree draft entries.

After real bindings/quality evidence are supplied, review the exact command:
`/PINNED/python3 benchmarks/launch_n16_painted_interval.py --phase pilot --config /ABS/CONFIG --config-sha256 SHA --dry-run`.
Launch through existing runner → pinned strict observer → launcher → driver;
use the launcher's exact argv contract. Check native receipt.argv/guard.command
linkage explicitly; the inherited native evaluator does not establish it alone.
Known-job observations are cooperative evidence, not proof of host exclusivity.
Retain the plan's three quiet preflight observations, 5s scans, 5,400s launched
window and 120s child limit. A preflight refusal is not a measured regression;
post-launch contamination or incomplete cleanup leaves the trial unavailable.

| Phase | Warmup + measured pairs per cell | Seed | Outputs |
| --- | --- | --- | --- |
| Native pilot | 5 + 5 | 20261105 | 1,040 |
| WASI diagnostic, 12 full cells | 2 + 12 | 20261107 | 336 |
| Sole fresh native confirmation | 5 + 200 | 20261106 | 21,320 |

WASI cells: opaque64/fill/empty/distant/tiny × both modes, plus image-only Exact
and default-knockout64 Exact. Use `n16_painted_interval_wasi.py` and
`run_profile_wasi.mjs`; audit via `evaluate_n16_painted_interval_wasi.py` with
external final pins. Fresh Node/instance per sample; render time excludes explicit
startup/instantiation but may include tiering. Node peak RSS and exported WASM
memory are distinct. A median ratio >1.05 plus ≥10/12 ratios >1.02 in an N16
cell suspends confirmation; comparable fallback signals require explicit review.

Native audit entry: `evaluate_n16_painted_interval.py pilot-001` / `confirmation-001`.
Keep fixed paired bootstrap 5,000 resamples, one-sided 95%, seed42017; inherited
evaluator authenticates reported bounds rather than independently resampling.
Retain every warmup/measured result, raw pixels, failures, process/stage timing
and RSS. No favorable filtering, retry, pooling or reuse of start/current data.

## 5. Separate safety, full admission and useful partial retention

Hard reject/revise: pixel/alpha/premultiplication error; changed source/error or
work/byte admission; tile/reuse failure; deterministic retained-memory growth;
enclosing-layout growth under this no-growth plan. Incomplete/tampered evidence,
contention or unavailable required checks cannot establish a performance result.

Full **incremental experiment** pass requires both target median upper≤0.95 and
p95 upper≤1.00, every protected median/p95 upper≤1.02, each native cell's sampled
max-RSS ratio≤1.00, and all mandatory checks. This is not the competitor/release
gate and says nothing by itself about total-stack or worldwide performance.

Preserve the interval plan's partial-retention rule: one original target must
reproduce median upper≤0.95 in the sole fresh confirmation, with every mandatory
safety/resource/layout/provenance gate complete. Keep all protected/p95/RSS misses
visible. Protected lower>1.02 is supported adverse evidence requiring revision
or an explicit reasoned experimental restriction; an upper-bound miss alone is
not proof of regression. Sampled RSS misses remain performance failures requiring
investigation; they neither prove a semantic memory-limit bug nor disappear
because storage is unchanged. Deterministic capacity growth is a resource issue.
Retention requires a recorded coordinator decision, not an evaluator side effect.
Freeze these rules before timing; do not silently import a different experiment's
partial threshold or retrospectively change any completed gate.
