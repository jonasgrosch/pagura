# Plain opaque precision: independent baseline

2026-10-21. The committed glyph-memo stack retains its measured partial wins;
this separate probe confirms a pre-existing composition defect. No product
code changed, no accuracy fix is claimed, and no performance or memory result
is inferred from these diagnostic renders.

The independent generator is `benchmarks/opaque_precision_fixtures.py`. Both
one-pixel PDFs contain only solid opaque path marks directly in the page stream.
Their coverage bytes are independently fixed by rectangle area at 72 DPI.
ISO 32000-2, 11.3.3 supplies the Normal recurrence; ADR 0004 supplies the
discrete-coverage one-level target. Neither baseline pixels nor a competitor
sets the expected values.

| Case | Exact rational result | Admissible bytes | Exact output | Competitive output |
| --- | --- | --- | --- | --- |
| Gray background and two partial gray marks | 44617/204 = 218.710784… | 218–219 | **217: fail** | 219: pass |
| White background and 1,000 black slivers | 255·(254/255)^1000 = 5.012754… | 5–6 | **127: fail** | **127: fail** |

All RGB channels have the listed value and alpha is 255. Each mode/case was
rendered through both full and tile entry points; their bytes agree. A
one-pixel tile does not test multi-tile seams. The one-level target is applied
to both modes here; this does not change Competitive's existing policy or
invent a per-fixture allowance. The failures remain visible.

The first eight-output diagnostic reused immutable candidate binary
`bcf7603da254a9561d2b31015bb42f09bd77a2bc7fe0107b2092d348aa25b0c6`.
Its original source identity and build receipt are bound in
`glyph-memo-bindings.json`; the test-only cleanup relating it to product commit
`d16c07f8` is recorded in `glyph-memo-post-measurement-bindings.json`.
The [baseline record](opaque-precision-baseline.json) retains the initial plan,
raw RGBA values, file hashes, runner receipt and observation guard.

Review then tightened the probe's integrity checks: compare final PDF/PPM
hashes to their original values, hash the bytes actually verified, and reject
changes to verified output. The initial artifacts independently pass these
checks. One additional eight-output execution validated the hardened helper;
every output and contract outcome is identical. Both attempts and the original
helper are retained. The [audit](opaque-precision-audit.json) pins both attempts,
the exact helper diff, external build evidence and source correspondence.
Neither run is a performance confirmation or a retry for a favorable result.

Both coordinated runs completed with unchanged sources, exit zero and no
observed competing job (three process scans each). Exit zero means the
diagnostic executed successfully, **not** that accuracy passed. The driver uses
default open options and emits neither repair diagnostics nor a dispatch
trace; selection of the legacy opaque Exact route is a static inference from
the content and bound source. Strict-open diagnostics and actual compiled
eligibility still belong in the eventual integration regression.

To reproduce, build the existing `scene_index_profile` example from the bound
product source, then run `benchmarks/opaque_precision_probe.py --binary PATH
--output NEW_DIRECTORY` through the coordinated runner in a quiet slot. Output
directories must be new. Inspect `within_contract` and `all_within_contract`;
do not use process success as the accuracy gate. The generator and driver are
independent of the renderer's writer; the full numerator/denominator and
admissible interval are frozen before the first render.

## Correction boundary

Use the existing N16 kernel for opaque coverage accumulation; combining the
two byte roundings alone cannot fix the long-stack stagnation. The
[implementation plan](../../docs/plans/opaque-composition-accuracy.md) requires
source validation before culling, explicit resource/diagnostic migration,
preserved fragment producers and a separately frozen incremental speed/RSS
experiment. No weaker oracle or larger tolerance is accepted as a fix.
