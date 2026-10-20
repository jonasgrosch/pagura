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
receipt. Current measurements discard temporary outputs and do not retain
durable per-sample artifacts. Therefore `gate.py` and `evaluate_cell` continue
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

Next, complete U1 artifact/fixture bindings and U2 runner coordination, then
start S0's private, initially all-command scene index. Follow S1/I1/C0 only
after their independent gates are executable. Do not infer permission to weaken
release admission from the existence of the standalone helper.
