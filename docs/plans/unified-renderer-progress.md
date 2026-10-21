# Unified renderer implementation log

2026-10-20. Branch `codex/unified-renderer-plan`, based on
`359e19dda5b78a23675678b6a153da7ab7fdc2c5`. The architecture, implementation
plan, and validation contract were committed as `89d9f30f`.

## First slice: evidence and execution controls

| Work item | Written | Remaining before completion |
|---|---|---|
| U1 reference verification | `tools/benchkit/oracles.py`: strict, pinned metadata and source-byte validation; actual RGB8 comparison; explicit zero/one final channel bound. Negative contract tests in `tools/test_bench_oracles.py` passed. | Independently review and annotate real fixtures through existing `Case.extra`/schema-2 manifests. Bind actual execution settings and persistent per-sample output artifacts. Integrate into conformance and, later, release evaluation. |
| U1 statistical rejection evidence | `statistics.py`: lower one-sided percentile-bootstrap endpoints using the same paired resamples as the existing upper endpoints. Reject unusable timing/memory evidence. Hand-calculated distribution and failure tests in `tools/test_bench_statistics.py` passed alongside existing harness tests. | Add the predeclared per-change decision policy; lower bounds alone do not implement a stopping rule. |
| U2 bounded validation | `tools/check.py --jobs N` forwards Cargo build jobs, Rust test threads, and conformance workers; `--dry-run` prints only. Orchestration tests in `tools/test_check.py`. | Execute tests and verify actual subprocess limits on an exclusively allocated runner. Define the exclusive-runner receipt; the flag provides no reservation. |

No renderer path, public API, default accuracy setting, fixture tolerance, or
release threshold is changed. These are partial U1/U2 deliveries, not completed
accuracy gates or an implemented speedup. No production references have been
labelled independent solely because their metadata says so.

### Narrow reference contract

`reference_digest(metadata)` validates and hashes canonical UTF-8 JSON
(`sort_keys=True`, compact separators, ASCII escaping, no NaN). The metadata
has exactly these keys:

- `schema`: integer 1; `case_id`: nonempty identity.
- `settings`: source PDF hash, render-policy hash, page, DPI, white background,
  RGB8 PPM output, matching MediaBox/CropBox, annotations enabled, accuracy,
  entrypoint, and compilation-validation scope.
- `dimensions`: positive integer width/height, within the existing PPM budget.
- `reference`: model, `exact_rational` evidence class, `nearest-half-up`
  rounding, and a derivation source path.
- `oracle_sha256`: hash of the entire expected P6 file.
- `sources`: canonical relative POSIX names mapped to SHA-256 hashes, including
  the source PDF, render-policy document, oracle code/data, and reviewed
  derivation. Completeness of this list is a review requirement.
- `max_channel_error`: integer 0 or 1, frozen before measuring output.

The render-policy document must describe font/provider identities, ICC profiles
and color intent, overprint/layers/recovery, rotation/UserUnit, filtering,
coverage quantization and source interpretation. A hash binds the document;
this helper cannot judge its completeness or mathematical correctness.

The initial supported models are `decoded-samples-rgb8`,
`polygon-coverage-rgb8`, and `per-mark-coverage-rgb8`. True-curve numerical
evidence, rigorous enclosures, continuous scenes, semantic adjudication, other
rounding contracts, arbitrary page boxes, and alpha output need separate
validated contracts. They are rejected here rather than silently normalized.

`verify_reference(actual_ppm, oracle_ppm, metadata, *, pinned_digest, settings,
sources)` receives immutable image/source bytes. Its pin must come from the
frozen, reviewed fixture/policy configuration, outside the candidate report.
It validates the complete source set, oracle bytes, settings and dimensions,
then recomputes output hashes and per-channel differences from the bytes.
Malformed provenance/images raise `ValueError`; a valid image with wrong
dimensions or pixels is `incorrect`. Test vectors exercise this binding contract
only; their placeholder source bytes are not conformance fixtures or proofs.

The caller must bind settings to the actual render invocation. A returned
dictionary is not an execution attestation, mathematical proof, or release
receipt. First-slice measurements discarded temporary outputs; the second slice
below adds durable per-sample artifacts. `gate.py` and `evaluate_cell` continue
requiring exact oracle pixel hashes; the existing test rejecting tolerance-only
release evidence remains in force. Release matrix aggregation remains blocked.

### Statistical interpretation

`median_lower` and `p95_lower` use quantile `1 - confidence`; the upper
endpoints continue using `confidence`. At 0.95 these are separate one-sided
95% percentile-bootstrap estimates, not a jointly 95% interval. The algorithm
does not promise calibrated coverage for arbitrary distributions, small samples,
or temporally dependent measurements. Lower bounds support the plan's
predeclared rejection criteria; they do not replace upper admission bounds or
authorize repeated attempts until a passing run appears.

### First-slice validation

On 2026-10-20, after user confirmation and a local process check, validation
ran sequentially with both existing local coordination locks held
(`/tmp/pagura-bench.lockdir` and `/tmp/pagura-bench.lock`). Other agents only
read or drafted outside the worktree, keeping this source tree frozen.

All 43 focused tests passed, followed by the complete workflow:

```sh
python3 -m unittest discover -s tools -p 'test_bench_oracles.py'
python3 -m unittest discover -s tools -p 'test_bench_statistics.py'
python3 -m unittest discover -s tools -p 'test_check.py'
python3 tools/check.py --jobs 1
```

The full check passed: policy, all 20 fixture areas/13,499 files, bundled data,
formatting, Clippy, workspace tests, documentation, native/WASM builds, host
checks, all 99 Python tests, and all 1,237 generated-PDF conformance cases.
Source digest: `389911420d249dc2c93f4b2e015fe6d16f74e265dd9e8268e73d9e5ed886b47d`.
The local log is retained in `benchmarks/results/local/validation/first-slice.log`.
Syntax, whitespace and dry-run inspection also passed. No performance benchmark
ran. Cooperative locking and `--jobs 1` are not dedicated-runner attestations.

The first slice was committed as `f530be92`.

## Second slice: reference, artifact and runner bindings

Implementation passed integrated validation on 2026-10-21 (Europe/Berlin).
No scene-index changes are included. The following evidence links are now implemented:

- Four existing `paint/image-direct-*` fixtures carry reviewed Exact and
  Competitive reference records. The manifest pins the PDF, PPM, rational
  derivation, rendering policy and repository-local generator/verifier source
  closure. The [policy](../references/image-direct-policy.md) and
  [derivation](../references/image-direct-derivation.md) specify this narrow
  opaque, 32-by-32, 72-DPI model. Existing pixels and tolerances are unchanged.
  Other families, true curves and rigorous enclosures remain future work.
- Conformance renders a verified input snapshot with explicit mode, overprint,
  page and DPI, then verifies actual bytes against independent frozen pins.
  Alternate corpus views lose their reference annotation and stay unverified.
- Render, conversion and document benchmarks retain owned input snapshots, raw
  outputs and immutable execution receipts, including successful warmups and
  failure evidence. Frozen contexts bind argv, engine identity, page/DPI,
  pair/order, reference pins and settings. Output normalization and hashing run
  outside timing. Input snapshots default to a 64 MiB per-file limit, independently
  of the default 1 GiB total artifact budget; exhaustion marks
  evidence incomplete and prevents later launches. This is warm-filesystem
  evidence because input and executable preflight hashing warms caches.
- The [runner wrapper](../runner-coordination.md) holds both shared local locks,
  retains ownership while its process group runs, and binds source boundaries
  and final report bytes. Measurement cancellation forwards into the separately
  owned renderer group and stops sampling after its cleanup. The wrapper cannot
  track deliberately escaped daemons or attest dedicated hardware.
- The release gate requires an independently supplied final runner-receipt pin.
  It rechecks the report, corpus, engine identities, retained input/output bytes,
  reference settings and all sample/warmup contexts. A pointer inside a candidate
  report cannot supply its own trust anchor. Exact oracle hashes, controlled
  runner evidence and the existing unimplemented matrix blocks still apply.

These bindings are necessary validation infrastructure, not proof that the
renderer is faster, universally accurate, or ready for release. The next renderer
change is S0's private, initially all-command scene index, after this slice passes
its checks. S1/I1/C0 still require their own independent accuracy and performance
gates and predeclared rejection criteria.

### Second-slice validation

All 182 Python tests passed, including real owned-process cancellation cleanup,
fault-injected lock cleanup/final receipt failure, missing/tampered artifacts,
reference/settings mismatches, output-format restrictions and retained warmups.
The four annotated fixtures also passed actual CLI scan/render verification in
both Exact and Competitive modes. Regeneration updated only their paint-manifest
metadata; all 304 existing paint PDF/oracle files remained byte-identical.

The complete `python3 tools/check.py --jobs 1` workflow then passed under
`tools/runner.py` during the user-reserved local window: policy, all 20 fixture
areas/13,499 generated files, all 107 bundled data files, formatting, Clippy,
workspace/doc tests, documentation, native/WASM builds, host checks, 182 Python
tests and all 1,237 generated-PDF conformance cases. Duration: 737.82 seconds.
The wrapper recorded both exit codes as zero, unchanged before/after source,
and a completed receipt after lock cleanup. No performance benchmark ran.

- Validated source SHA-256:
  `977fa8a203b4f3baacc9e7821735bf13d3042e8e07881377ac93925a058901a8`.
- Local log: `benchmarks/results/local/validation/bindings-full-001.log`, SHA-256
  `f5881c8c67d0c4626f2bc877a25445029e113a5d43cd287bcf570810849611c3`.
- Local receipt: `benchmarks/results/local/validation/bindings-full-001.json`, SHA-256
  `20aad9455bbe9973b4ed5ca8a99648b1d49422d246cfa1d50116c0e13d7f5316`.

These are local validation receipts, not release benchmark or dedicated-hardware
attestations. The first focused run exposed macOS temporary-path alias assumptions
in test fixtures; canonical paths fixed those assertions before the successful
rerun. Independent review also moved terminal receipt publication after lock
cleanup so a failed cleanup/write cannot leave an earlier success receipt.

## S0a: lazy root bounds experiment (2026-10-21)

The first renderer optimization candidate introduced reuse of
existing glyph-run device hulls across compositor bands/chunks in both policies.
It retained the original command traversal, error ordering and work charges.
Twelve focused tests and strict all-target Clippy passed. This was an experimental
candidate; at that stage the complete repository check and performance
admission were pending.

The 24-cell pilot retained and independently verified all 480 outputs, including
warmups. Eligible full-render median ratios were approximately 0.817 in each
mode. These small-sample results do not meet the whole P1 gate and do not show a
general text or competitor speedup. Default knockout text is outside admission.
The fixed index object occupied 2,064 bytes on the measured native target.

The planned 200-pair confirmation was stopped when another worktree started a
full check inside the reserved compute window. Its entire partial run is invalid;
its timings have not been analyzed or pooled. The foreign job began after the
pilot had ended. The single replacement preflight then detected a new release
build in `practical-gould` and refused to launch; it produced no samples. The
performance gate was then inconclusive, with no automatic third confirmation.
The active contention guard passed controlled success, refusal, cancellation
and deadline checks. It never signals another worktree's process. A subsequent
full-check preflight refused a fresh `friendly-rosalind` release build, before
launching any validation. At that point the candidate remained uncommitted
pending the required complete check.

See the [experiment record](../../benchmarks/results/scene-index-experiment.md)
for evidence paths, hashes, narrow scope and gate decisions. No candidate
commit or completed implementation acceptance was claimed at that stage.

The user later confirmed other compute was paused. A frozen, independently
reviewed resumption waited through zero-child preflight refusals, then launched
confirmation004 after three quiet snapshots. A new `practical-gould` release
build began about one minute later; the active guard stopped our child and
retained process evidence. The entire partial run is excluded without timing
analysis. Full validation and committing were still pending; no automatic
measured restart was planned. See the experiment record for exact timestamps
and hashes.

### Completed S0a correctness validation

On 2026-10-21 the user instructed “just go ahead”. The complete
`python3 tools/check.py --jobs 1` workflow then passed under the cooperative
runner at `nice -n 10`, without requiring source changes. Another worktree was
active; this is correctness validation, not an uncontended performance run.

Policy, all 20 fixture areas/13,499 files, 107 bundled files, formatting, workspace
Clippy/tests, documentation/doc tests, native/WASM builds, host integration,
182 Python tests and all 1,237 generated-PDF conformance cases passed. All
12 new bounds-reuse tests passed within the full suite.

The runner recorded 965.647 seconds, both exit codes zero,
and unchanged source SHA-256
`44dbd6cee7134c2304dbd51b73291d46a7ff630bd0937c02d71c4dfb38bad91f`.
The [experiment record](../../benchmarks/results/scene-index-experiment.md)
binds the log and receipt hashes.

This validated experiment was committed as `9ebac1cb`. P1 remained unpassed;
no production admission, general speedup or competitor superiority was claimed.

### S0a removal and next viewport experiment

The user then requested speed and memory confirmation with an explicit
keep-only-if-all-gates-pass condition. The
[final confirmation plan](../../benchmarks/results/scene-index-final-confirmation-plan.json)
froze confirmation005 before execution. Its final preflight at
2026-10-20 23:31:15 UTC refused external compute contention. The coordination
wrapper ran the guard, but the guard launched no benchmark child and produced
zero measured samples. The [decision record](../../benchmarks/results/scene-index-decision.json)
binds the guard and receipt hashes.

Without valid complete confirmation, retention was not authorized under that
condition. The memo and its fixed index storage have been removed, restoring
the original direct hull calculation. This is performance non-admission, not a
numerical speed or memory regression or a correctness finding. The pilot,
excluded partial runs, successful full-check history, candidate identity and
all frozen plan references remain preserved.

The subsequent isolated candidate constructed the `Context::clip` viewport only
for soft-mask sources. The release build and existing independent-pixel test
passed, followed by 40 independently verified renders across 20 cells in both
modes, with exact RGB, opaque alpha and same-mode byte equality. Static ARM64
release disassembly showed ordinary clips branching around path-vector growth
and an 80-byte smaller function stack reservation; these are not speed or RSS
measurements.

The bounded scheduling wait expired with foreign compute active, before any
benchmark launch. The candidate was removed from the renderer and preserved as a
[patch with frozen bindings and evidence](../../benchmarks/results/clip-viewport-experiment.md).
No numeric regression or accepted performance improvement is claimed. The shared
paired-run harness now binds its entry, helper and fixture modules explicitly and
requires the candidate source to be the checkout providing its imported tools.


After the user explicitly requested immediate performance testing, the coordinator
stopped the owned correctness check and ran the unchanged clip experiment serially,
recording background compute without aborting. The 400-output pilot and fresh
200-pair-per-cell confirmation completed separately. All 8,200 confirmation
outputs passed exact independent RGB, opaque alpha and same-mode byte parity.
Target medians were 5.035 to 4.532 ms for Exact (-10.0%) and 8.700 to 8.231 ms for
Competitive (-5.4%). The complete numeric gate failed: one of two target latency
cells, three of eighteen protected latency cells, and twelve of twenty memory
cells passed. Background contention also fails the environment gate. These are
observations under load, not an accepted speedup or established causal regression.
The candidate was removed again and the final restored-source check restarted.

Following the user's question about discarding partial wins, the experiment is
classified as promising but not yet admitted. The numerical results, failed gates,
and product state are unchanged. The patch remains a candidate for incremental
evaluation on the newer integration stack; a missing admission is not a finding
that an idea should be retired. The [stacking review](../../benchmarks/results/performance-stacking-review.md)
separates cache overlap, independent image/text work, branch ancestry, and the
measurements needed to establish a combined gain.

The final restored-source check002 passed with unchanged source identity in 985
seconds: 182 Python tests, 1,237 conformance cases, strict Rust checks and native/
WASM host parity. Its receipt and log hashes are bound in the clip experiment.
