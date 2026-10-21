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

## Cross-integration on the combined performance stack

An isolated candidate on `202bda8d` retains the newer image/text/stroke work and
four-entry clip cache, and adds only conditional viewport construction. The
[experiment](../../benchmarks/results/stacked-clip-experiment.md) keeps numerical
results, independent pixel checks, execution conditions and promotion separate.
The earlier partial gains remain available; neither historical percentages nor
checks of the restored old renderer establish this candidate's result.

The replacement fixed confirmation completed with 200 fresh measured pairs and
five warmup pairs in each of 20 cells. All 8,200 outputs passed independent exact
RGB, opaque alpha and same-mode byte equality. The clip-churn full-render target
medians fell from 5.335 to 4.624 ms in Exact (13.3%) and 8.626 to 7.892 ms in
Competitive (8.5%). Both median gain bounds pass; the complete target latency
gates and some protected latency/RSS gates do not. No prohibited jobs were
observed during this run, but it is not a controlled-release-runner attestation.

The first confirmation stopped on an infrastructure error after eleven cells;
those partial timings remain excluded. An ownership-preserving process-group
repair passed 44 focused tests, and one separately frozen replacement run used
the unchanged product binaries. The complete combined-source check passed in
1,027.45 seconds, including native/WASM and host checks, 196 Python tests and
1,237 generated-PDF conformance cases. The completed receipt records unchanged
source SHA-256
`511edc3ebc862398defd7c795f2fffbb2d71094ebf2b21b722aa2fcb1da016b2`;
the experiment record binds the log and receipt hashes.
This candidate remains in the experimental branch, without performance promotion.
That baseline is now the control for the incremental memo experiment below.

## Bounded root glyph-hull memo on the retained stack

The user activated the [standing renderer goal](renderer-standing-goal.md) on
2026-10-21. It continues through independent accuracy and fair competitor
performance/memory evidence; useful partial results do not complete it.

The [memo experiment](../../benchmarks/results/glyph-memo-experiment.md) adds
only lazy reuse of existing finite root glyph-run hulls to `776fd4b9`. It keeps
the first 64 root-command ordinals, multiple-source admission, canonical device
transform, repeated internal-region requirement, existing charges and fallback.
It does not select commands or complete the general shared scene index.
The native memo occupies 2,064 bytes in each Context even when disabled;
this is separate from process RSS and decoded storage.

Thirteen focused tests passed, including actual Competitive fragment paths,
warm caches, off-tile regions, exact work boundaries and positioned errors.
The fixed 34-cell confirmation verified all 13,940 independent output checks.
The Exact and Competitive designated target median ratios are 0.8233 and 0.9096;
both target median, p95 and peak-RSS gates pass. Seven of 32 protected latency
cells and 19 of 34 peak-RSS cells pass their full bounds. Image-only Exact tail
and tiny Competitive cold-median lower bounds exceed the protected threshold;
these are recorded regression evidence, not dismissed as inconclusive noise.
Two-source controls do not establish a gain. Retain experimentally without
admission, and investigate the disabled paths in a separately frozen revision.

Full integrated validation passed in 772.15 seconds, including 196 Python tests,
native/WASM and host checks, and all 1,237 generated-PDF conformance cases.
The completed receipt records unchanged source
`0106c6da903589edfc3cba638702176d252cab80bef1582954895dfe8a894b12`;
148 observer scans found no competing jobs. The local full-check log hash is
`e4daefd3e8e9cfedb73e61387ba6d572457fc16dd2b61777cc87dc2e50b9aa5b`,
and receipt hash is
`9d31fd7a37eaac1d820bd0cfff0132fc051ba8b56c24d86d6a49eef3264607e5`.
Two earlier attempts exposed only a test iteration lint and its follow-up
dereference error; those logs are retained.
The test cleanup does not alter the measured product. The original measured
source hashes are reconstructed from its retained test preimage and the current
source closure, with both measured binary hashes unchanged.

Static review also confirms the separate
[plain opaque composition accuracy gap](opaque-composition-accuracy.md).
The memo's output equivalence did not fix or bless that defect; the subsequent
independent reproducer and shared-composition correction are recorded below.

## Shared opaque composition and exact output conversion

The [independent direct-root cases](../../benchmarks/results/opaque-precision-baseline.md)
reproduced the precision failures on the retained stack. Commit `4dfbaea9`
removed the legacy opaque traversal in favor of the bounded shared compositor,
preserving required off-region source errors and Competitive fragment masks.
Both modes now return 219 and 5 within the independent rational intervals.
The [46-cell pilot](../../benchmarks/results/opaque-composition-experiment.md)
measured material costs: target median ratios 2.731 and 3.003 versus the legacy
route. This remains an experimental accuracy correction, not a speed win.

The following exact flat N16 output converter addresses a profiled cost without
changing quantization or adding storage. An initial pilot found a useful gain,
but its confirmation was canceled after an independent review reproduced wrong
premultiplied pixels for nonopaque colored backgrounds. The canceled launch and
all evidence remain preserved; its samples are excluded from the replacement
experiment. The correction selects existing F64 storage for nonopaque
backgrounds, retaining its higher memory charges and possible `Limit` results.
The [background-guarded experiment](../../benchmarks/results/n16-output-guarded-plan.json)
uses a new frozen source/binary pair and fresh samples with unchanged gates.
The complete check passed: 210 renderer tests, 196 Python tests, native/WASM
and host checks, and all 1,249 conformance cases. Independent precision probes
also pass. Final performance evidence is recorded separately from those checks.

The completed [confirmation and audit](../../benchmarks/results/n16-output-guarded-experiment.md)
verify 18,860 outputs. Target medians fall 12.5% in Exact and 14.4% in
Competitive; both target latency gates pass. Only 15/44 protected latency and
25/46 peak-RSS gates pass. Target RSS increases and a supported default-knockout
Exact tail regression keep this an experimental partial win. The run lasted
1,839.25 seconds; 353 observer scans recorded no competing jobs. No samples
from the canceled confirmation or the pilot enter the confirmation analysis.

[Next slices](n16-next-slices.md) are an exact 16-bit converter, conservative
untouched-output skipping and command-local paint resolution, followed by a
direct net-stack comparison. They remain hypotheses until separately measured.

## Exact output conversion in 16-bit lanes

The [u16 follow-up](../../benchmarks/results/n16-output-u16-experiment.md) replaces
widened addition with saturating u16 addition in the same exact converter. It
adds no storage and removes the obsolete lint exception. Exhaustive 65,536-word
equivalence, source-offset/tail checks, both-mode background/precision cases,
and native/WASI host parity pass. Full validation passes 876 Rust tests,
196 Python tests and 1,249 conformance cases.

The fixed 46-cell confirmation verified all 18,860 outputs with 200 fresh pairs
and five warmups per cell. Exact opaque64/full falls from 4.258334 to 3.797875 ms
(10.8%); Competitive falls from 3.566167 to 3.115730 ms (12.6%). Both target
latency and peak-RSS gates pass. Twelve of 44 protected latency cells and 25 of
46 RSS cells pass. No protected median/p95 lower bound exceeds 1.02, but missing
upper bounds and substantial RSS increases remain open. The explicit decision
is experimental retention, without full admission.

The run lasted 2,001.45 seconds including quiet preflight; 384 observer scans
recorded no known competing jobs. The separate WASI screen checked 168 outputs
and equal linear-memory sizes with no material-regression triage signal.
The same-binary RSS diagnostic checked 84 outputs and found differing maxima
even with equal medians; no threshold or earlier failure is changed. Its first
audit stopped on a platform-string mismatch, then the corrected auditor checked
the same retained samples successfully. No render or performance run was retried.

Fresh profiling of this retained product precedes the next structural slice.
The prepared-span and untouched-output proposals remain unapplied hypotheses.

## Current competitor gap and translucent composition

The resource/stroke integration at `c70b4678` is the latest measured product
baseline. The [analytic19 competitor pilot](../../benchmarks/results/analytic19-conversion.md)
finds a median Pagura/Hayro ratio of 1.203 across 13 independently qualifying
cases. Reaching the 0.80 target needs a median 33.49% time reduction; individual
case needs range from 25.36% to 66.30%. This small native pilot is not the release
matrix or a worldwide ranking. Memory and broader accuracy gates remain open.

The [many-marks profile](../../benchmarks/results/many-marks-profile.md) directs
the next work toward allocation and compilation costs. The
[color-reuse preflight](../../benchmarks/results/color-reuse-preflight.md)
preserved an allocation candidate without timing it: both arms reproduced an
independent Competitive translucent-text error. Its patch remains archived and
was absent from the following experiment.

The [F32 correction and cost diagnostic](../../benchmarks/results/f32-preservation-cost.md)
preserve planner-admitted composition precision in Competitive mode. An F32
fragment-mask sink keeps glyph-cache reuse, pre-visibility source errors and
once-per-run color conversion. The independent text answer is now 64 instead
of 63. This does not unify the remaining geometry, image/font or shading policies.

All 1,406 candidate outputs pass the frozen independent references. The fixed
trial contains 76 preflight outputs and 2,736 warmup/measured outputs, with all
38 cells retained. Median case time ratios are 0.9942 Exact and 0.9951 Competitive;
these small variations do not establish a speed win or confidence-bound admission.
The full single-job check passes, including 1,249 conformance cases. All 226
native renderer tests, 104 WASI compositor tests and the WASI pixel-budget test
pass. The independent auditor verifies raw outputs, measurements and final
coordination bindings. Wide dynamic/shading performance and WASI speed remain
unmeasured; F32 chunks can change traversal, memory and resource-limit outcomes.

Retain this accuracy improvement separately from performance claims. The next
small performance slice reserves the five rectangle segments once, then measures
its incremental effect. Color reuse, prepared spans and untouched-output skipping
remain separate candidates; historical percentages must not be added together.


## F32 output interval and requested pause

The F32 interval continuation and independent audit completed with every original
complete pair preserved. Both sparse target latency gates pass: median time falls
50.0% Exact and 55.1% Competitive versus the retained N16 interval. Protected
latency passes 17/54 and sampled RSS passes 30/56, with supported control
regressions and Exact target RSS growth. Retain experimentally, without promotion.

[The pause status](unified-renderer-status.md) records the full result, method
amendment, validation, unresolved goal gaps and restart point. Work is paused
after this commit at the user's request; no next optimization has started.
