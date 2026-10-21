# N16 route reach R2 — separate post-discovery diagnostic

This package is source-only and unexecuted. It supersedes neither the original
R1 sources nor R1's failed reach gate. R1's six-row opaque64 Competitive
hypothesis failed after its independent RGBA comparison passed. R2 records that
discovery explicitly, corrects only that old interval expectation from the
source proof below, and adds three fresh inputs fixed before their first run.
There is no change to renderer accuracy, benchmark cells, tolerances or timing
thresholds. The instrumented binaries cannot be used for performance claims.

`base/` contains four copied production files from the isolated interval
candidate. `overlay/` changes only diagnostic statements and the example's
temporary public-background argument. `reach.patch` is their exact diff;
`source-sha256.txt` binds both versions. No active checkout was edited. The
original R1 package and its failed report must remain retained unchanged.

## Actual observation and source proof

The marker is `PAGURA_N16_REACH_V2 `. It observes the selected plan after the
nonopaque-background guard; every actual chunk and allocated surface; each
validated N16 path span or fragment row; and actual output interval, converted
pixels and calls. A second plan, fallback, bypass, wrong format, missing chunk
or unexpected writer blocks acceptance. No type field, allocation, resource
charge, error branch, source evaluation, geometry or store arithmetic changes.

The new writer field `nonzero` counts nonzero bytes in the actual submitted
coverage slice, after existing validation and before the unchanged store.
This read-only scan and stderr logging make timing unusable. A zero row still
widens the existing interval; the diagnostic does not optimize or skip it.

Source anchors in the copied candidate:

- `benchmarks/scene_index.py:53–80`: opaque64 is 2048 square; each of64 runs
  contains64 adjacent integer4×6 glyphs, making one256×6 stripe per256 square
  chunk. Its device ink occupies local rows234 through239.
- `fragments/glyphs.rs:299–301` adds one phase row/column to the integer bounds;
  `607–609` pads a5-column mask to8, retaining7 rows for a4×6 glyph.
- `glyphs.rs:741–771` resets all accumulator cells to zero. `span` at650–663
  traverses the half-open vertical interval0..6 at phase0. The final phase row
  is zero; the adjacent quad term in849–857 contributes exactly zero there.
  `finish_into` at939–970 nevertheless emits all7 rows.
- `compositor/narrow.rs`'s validated fragment-row callback widens the interval
  before the zero-coverage store skips arithmetic. Thus Competitive writes
  submitted rows234..240, yielding `[59904,61696)`,1792 converted pixels and7
  calls perchunk, with1536 positive samples and a verified zero row240.
- Exact's path coverage emits the6 nonzero rows, preserving R1's
  `[59904,61440)`,1536 pixels/6 calls. Path rectangle coverage and span traversal
  do not acquire the glyph-mask phase row; other original expectations stay.

These are submitted interval/output counts, not measured speed or allocations.
Opaque64 may submit additional zero-only horizontal tails from adjacent glyphs;
R2 therefore fixes positive coverage and the observed zero halo, not its total
writer count. Fresh single-glyph controls additionally fix every submitted
sample and writer count.

## Fixed scope and independent expectations

`request_plan.py` is the frozen request source:12 inputs/requests, each Exact
then Competitive, full72dpi,24 outputs. The first9 requests are the exact R1
inventory: opaque64, opaque-empty, opaque-distant, opaque-fill, opaque-tiny,
sparse64, default-knockout64, image-only, and nonopaque-empty. All original
pixels and route expectations remain, except the proved opaque64 Competitive
extent correction. Sparse64 and image-only remain F32; default-knockout64 and
nonopaque-empty remain F64. Image-only has quarter alpha, so no opaque-image
bypass is accepted. Nonopaque empty requires literal RGBA[9,43,85,128].

The three added PDFs use one encoded glyph33→ID1,1000 units/em,Tf10, an ordinary
four-point orthogonal glyph, opaque black, `/TK false`, and no complex clip.
`fresh_fixtures.py` authors their font/PDF bytes but derives RGB references from
separately fixed literal device boxes, not from a renderer or PDF parser.

| Request | PDF page/origin; glyph | Independent device ink box |
| --- | --- | --- |
| fresh-h2-position |256²;(12,17);4×2 |[12,16)×[237,239) |
| fresh-h5-right-clip |256²;(252,241);4×5 |[252,256)×[10,15) |
| fresh-h9-chunk-crossing |512²;(254,252);4×9 |[254,258)×[251,260) |

Exact submits the ink. Competitive submits a mask8×(height+1), clipped to the
current chunk. Each interval is `first_y*256+first_x` through
`(last_y-1)*256+last_x`; output calls count nonempty row intersections, including
holes inside the interval. These are fixed literal checks:

| Request/chunk | Exact interval; pixels/calls | Competitive interval; pixels/calls | Positive samples |
| --- | --- | --- | --- |
| h2 |60684..60944;260/2 |60684..61204;520/3 |8 |
| h5 |2812..3840;1028/5 |2812..4096;1284/6 |20 |
| h9 top-left |64510..65536;1026/5 |64510..65536;1026/5 |10 |
| h9 top-right |64256..65282;1026/5 |64256..65286;1030/5 |10 |
| h9 bottom-left |254..1024;770/4 |254..1280;1026/5 |8 |
| h9 bottom-right |0..770;770/4 |0..1030;1030/5 |8 |

Competitive submitted sample totals are24,24, then10/30/10/30 respectively;
Exact totals equal positive samples. Competitive has one observed all-zero
writer on h2 row239, h5 row15 and each lower h9 chunk's row4; the upper h9
chunks clip the phase row away. Every fresh request must use path writers in
Exact and fragment writers in Competitive. One-glyph eligibility follows
`interpreter/text.rs:442–481` (no minimum run length), explicit TKfalse excluding
its group wrapper, and `compositor.rs:572–585`/`fragment_paint`'s solid-color
dispatch. SceneIndex's separate two-source eligibility does not govern this
fragment route. Actual trace evidence is still required before claiming reach.

Raw outputs require exact full RGBA payload/dimensions, all alpha255, and exact
RGB agreement with the independent P6 references. The colored nonopaque empty
control compares all four literal premultiplied bytes; it is never normalized.
Every input/oracle/log/output is retained and hashed. Missing or wrong evidence
stops this diagnostic without rewriting a prediction or retrying.

## Coordinator preparation, execution and restoration

Root alone applies/removes patches, builds and executes. Archive this package
with all three sibling Python sources together. Before any child, externally
pin the package, original failed R1 report, production source snapshot, all four
original files, fixture/helper/runtime closure, exact patch and preparation.
Apply only the reviewed diagnostic delta; build an archived scene_index_profile
with a fresh diagnostic-specific target directory. Never share targets across
arms or use this binary for timing. Require a successful final build receipt,
strict observer and fresh Cargo-selected/hash-pinned example artifact.

The preparation JSON is schema2/revisionR2, with:

```text
instrumented_source: {root,commit,source_sha256}
pre_probe_source: {root,commit,source_sha256}
binary: {path,sha256}
patch: {path,sha256}
prior_failed_reach: {path,sha256}   # original failed R1 report, unchanged
build_evidence: [{path,sha256}, ...]  # final build report/receipt/guard/logs
```

Run the nine pure parser tests first, under the ordinary owned runner/observer:

```sh
/ABS/PYTHON -B /ABS/PACKAGE/test_run.py
/ABS/PYTHON -B /ABS/PACKAGE/run.py --binary /ABS/ARCHIVED/scene_index_profile \
  --preparation /ABS/preparation.json --preparation-sha256 FROZEN_SHA256 \
  --output /ABS/NEW/reach-r2
```

No fixture generation or renderer is invoked by the parser tests. The collector
uses the existing owned `run_process`,120-second per-child bounds and16MiB logs.
It runs exactly the fixed24 requests without retries. Its full request-plan JSON
is retained; source, binary, helper/runtime closure, inputs and raw artifacts
are rehashed at the end. Cancellation/failure checkpoints retain existing raw
output and explicitly mark unavailable logs, never inventing a successful row.

Report schema2/revisionR2 retains the R1 record structure and adds
`post_discovery_revision:true`, `prior_failed_reach`, `retained_request_plan`,
and per-chunk `coverage:{submitted,nonzero,all_zero_writers,halo_rows}`. Only24
strict rows permit `status:verified-route-reach`, `completed_outputs:24`, and
both success booleans. `trace(log,request,mode,background)`, `oracle_pixels` and
`verify_output` remain importable pure validators. An external consumer must
place the package directory on its import path and pin all imported sources.

The collector is not the final authority for build success, outer receipt or
guard. The coordinator must authenticate those separately, then remove exactly
the patch and compare all four originals plus production source identity with
the pre-probe snapshot. Archive this restoration record. Native24 full renders
do not prove WASI reach, tile/tiled reach, all52 timed cells or release admission.
The original R1 miss remains in the decision ledger; existing quality,104-output
uninstrumented preflight and frozen timing gates are unchanged.
