# Untimed session-start/current accuracy probes

Unexecuted off-tree draft. Root owns serial builds, tests and execution. This
does not alter either product checkout or its existing profile example. Build
the two external Cargo packages using the same pinned 1.99.1 toolchain, release
profile, environment and separate target directories. Both package manifests
point to the identical `../accuracy_probe.rs`; only path dependencies differ.
Generate and freeze each external Cargo.lock offline before locked builds.
Capture raw Cargo output, exact commands/env, source identities before/after,
common Rust/manifest/lock bytes, selected binary paths/hashes, and final build
receipt/observer evidence. This directory is not inside either source digest.

The Rust adapter uses existing public APIs in both revisions: strict document
open, full `compile_page`, page zero, `DisplayList::render` or `render_tile`,
scale 1, explicit background and mode. It writes raw premultiplied RGBA without
normalization, plus one JSON invocation/diagnostic record. Some earlier
transparency/background tests constructed private display lists; these probes
express the same literal geometry through small PDFs and add parser/compiler
coverage. They do not assert identical private dispatch or bypass compilation.

## Fixed scope and independently computed references

There are 27 requests per mode and arm, hence **108 outputs**:

- Two existing direct-root PDFs, full and 1×1 tile: 16 outputs. Accept only
  integers within one level of 44617/204 and 255×(254/255)^1000, respectively:
  RGB 218–219 and 5–6, alpha 255. These duplicate the separately collected
  16 untimed root-precision outputs; no timing or performance data is pooled.
- Six retained subbyte PDFs, full: 24 outputs. Use the existing declared
  coverage-byte composition reference: 255 for the three individually
  subbyte cases, zero for compound/full coverage, and 16129/255 for disjoint
  halves. The original absolute-one-level allowance is unchanged. Continuous
  scene coverage is a separate model, so disjoint-tiny white is not called
  continuously correct or perfect.
- Two full-coverage transparency PDFs, full: eight outputs. Two half-alpha
  red marks on white give [255,64,64,255]. Eight black marks at alpha 1/1024
  give [253,253,253,255]. Values are exact Fraction expressions rounded once.
- Straight colored background [200,100,50,a], a∈{0,128,254,255}, with blank,
  opaque half-width black, and full-width half-opacity black: 48 outputs.
  Premultiply the background once to output bytes, then apply coverage128/255
  or opacity1/2 and round once. Preserve the distinction between coverage and
  opacity, including the blue 12 versus13 result at background alpha128.
  These common probes are full-page only; the retained unit tests cover their
  one-pixel tiles separately. Do not imply both revisions ran those unit tests.
- A 259×3 opaque colored page, two half-alpha red rectangles at PDF(255,0,3,2):
  full, tile(254,0,5,3), tile(256,1,2,2), both modes/arms: 12 outputs. Painted
  device pixels x255..257,y1..2 are [241,25,13,255]; other pixels equal the
  [200,100,52,255] background. This checks x256 and nonzero origins.

All exact references include alpha; no RGB flattening is performed. Existing
root/subbyte PDFs are loaded using literal retained hashes, without regeneration.
New tiny PDFs have independently specified literal geometry and exact numbers;
their bytes and reference records are retained before rendering. No engine
output supplies any reference or tolerance.

## Collector binding and invocation

`collect.py` takes `--root ABS --bindings ABS --bindings-sha256 HEX --output NEW`.
Run from the candidate root under the existing serial runner and strict
observer. The frozen binding has:

```
schema: 1
experiment: start-current-accuracy
plan: references.PLAN (fixed 108; no parameters)
identities:
  baseline/candidate: {root, commit, source_sha256, binary, binary_sha256}
harness_sha256: {absolute file path: SHA256, ...}
completed_evidence: [{path: absolute completed build evidence, sha256: SHA256}, ...]
```

Required harness pins are collect.py, references.py, accuracy_probe.rs, both
external Cargo.toml files, and the candidate's benchmarks/paired_render.py.
Add both generated locks, build driver and runtime/toolchain closure. Product
`source_identity` includes all tools, so it separately binds imported benchkit
helpers. The coordinator authenticates build evidence and exact old/new source
identities; the collector preserves pinned evidence but does not infer that an
arbitrary JSON file proves a successful build.

Record order is request, then Exact/Competitive, then baseline/candidate. All
ordinary process/output/quality failures are retained through the 108 requested
invocations. Cancellation, lifecycle uncertainty, source/hash changes, retention
failure or unexpected Python exceptions abort and leave an incomplete report.
No retry, filtering, normalization or tolerance adjustment occurs. The retained
process object contains incidental elapsed fields; these are not benchmark
measurements and no performance summary is produced.

The report includes plan/identities/binding/harness hashes, requests and exact
reference files, raw commands/logs/RGBA files and their hashes, observed
invocation metadata, all baseline failures, and independently measured candidate
quality. Completed collection is distinct from candidate correctness. The
collector exits nonzero if any process fails or any candidate reference fails.
The live receipt snapshot is explicitly not terminal evidence.

An independent final audit must rederive expectations, check the full fixed
matrix, bytes and inputs, source/binary/build bindings, exact invocation chain,
and externally pinned final receipt/guard. Its admission interface for the
separate cost collector is `status: verified-accuracy-probes`,
`candidate_quality_verified: true`, `report_sha256`, and `identities`. It is an
accuracy prerequisite, never release admission or equivalent-output speed proof.

`audit.py` is the separate standard-library auditor. It imports neither the
collector nor reference module, rederives rational intervals, uses the literal
independently reviewed background table, reconstructs new PDF bytes, checks
original PDF hashes, and rechecks every raw channel and full/tile crop. Six
component tests cover arithmetic/schema edges; they are not an executed complete
synthetic-run test. The coordinator must execute and retain their result.

Run `audit.py --pins ABS --pins-sha256 SHA --output NEW_JSON`, outside the retained
run directory. The external pins object contains `root`; `report`, `bindings`,
`receipt`, `guard` descriptors `{path,sha256}`; `reviewed_build_evidence` equal to
the frozen binding's completed_evidence; `command` equal to the direct absolute
Python `-B collect.py` invocation; `producer_argv` equal to its observed arguments;
`runtime_files_sha256` containing the resolved requested launcher and observed
Python executable; and `coordination_sources` for tools/runner.py and the strict
analytic19-conversion/load_observer.py. The runtime entries must match the
pre-execution binding's harness entries. Arguments after Python argv0 remain
identical, allowing only an explicitly pinned launcher/app handoff. This does
not assert complete operating-system/runtime provenance. The coordinator's
reviewed completed build records are trusted and hash-checked, not recompiled.

## Correction lineage

The original Exact root opaque byte route rounded after each mark. Fresh root
comparison confirms original Exact217/127 → current219/5 for the two rational
cases, full and tile; original Competitive219/127 →219/5. The F32 correction
fixes original Competitive half-alpha63→64; original Exact already retained
F32 and64 in that scene. Background lineage is mixed: original Exact blank and
opaque pages already premultiplied correctly through legacy Pixmap. Migrating
those scenes to narrow storage exposed an Exact regression, repaired by the
background guard. Original general/Competitive narrow paths lacked that guard,
so background changes must be reported per actual mode/scene, not all credited
as improvements over original Exact or all dismissed as introduced regressions.
