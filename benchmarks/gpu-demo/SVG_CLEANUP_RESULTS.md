# SVG cleanup: retained exports and conservative clip elision

This follow-up retains the SVG export as a reusable feature experiment. It
keeps geometry and resource encoding separate from the browser and transport,
and measures the remaining costs of using SVG for presentation.

The host retains one resolution-independent SVG artifact, including its
embedded source images, for the current recorded page. A page or document
change invalidates it; the artifact is bounded by 64 MiB. The browser retains
one validated SVG and decoded image per comparison. Each measured render
still creates a new Canvas and completes RGBA readback; the application does
not cache page pixels. The demo's **Export SVG** button saves that same
self-contained artifact.

A separate bounded proof removes a fill/glyph clip application only when the
complete clip chain consists of rectangles and strictly contains the mark's
conservative control hull. Bounds include the affine transform, generous
antialiasing margin and finite-precision padding. Near-edge marks, uncertain
or empty chains, extreme coordinates, strokes, images and group clips stay
clipped. Placements, opacity and source order are preserved. No adjacent
marks or clip groups are merged.

The measurements below use the same four real first pages and the five
generated controls as the [initial comparison](SVG_RESULTS.md). The initial
implementation rebuilt SVG and encoded images for every sample. This one
retains those resources. Initial export/setup and steady rendering are
reported separately; the changed reuse policy is part of the experiment.
CPU/hybrid still produce fresh pixels from warm scenes, with no viewer tile
or bitmap cache. These results do not compare equally cached bitmap viewers.

The completed measurements and validation are recorded below.

## Method and provenance

Measured on 2026-10-23 locally, Apple M3 Pro (12 CPU / 18 GPU cores, 18 GB),
macOS 15.6.1, Chrome 155 / Metal hardware adapter. This shared desktop also
runs other rendering work. Results are exploratory and not a release gate.

Timed source: `5d12e86b3d1757a952a604faf749974745ec3bc0`; all recorded source hashes stayed
unchanged during the run. WASM SHA-256:
`1b6fd7ab750a313665769df158add2049d89da2bbff0b37ac475583a2f8718d4`.
The later SVG card caption change only clarifies the retained-render metric.

There are 27 cells: five generated controls and four real first pages at
72/150/300 DPI. Each CPU and admitted hybrid cell has 100 samples after five
warmups. SVG has 100 except Document 4, whose initial raster exceeded 250 ms
and triggers the existing three-sample cap. Totals are 2,700 CPU, 1,500 hybrid
and 2,409 SVG samples. Slow results are retained with their actual counts.
Source input hashes, adapter information, phase timings and raw samples are
in the local report.

SVG setup is outside its warm samples. CPU/hybrid render times include fresh
preparation/execution, with hybrid upload, GPU completion and RGBA readback.
CPU/hybrid frame times additionally include worker transport. SVG warm
samples reuse the exported/decoded artifact, create a fresh Canvas, draw and
complete RGBA readback. No new worker export request is made for that sample.
This comparison measures the requested SVG reuse policy; it does not prove
that SVG beats a CPU viewer caching completed tiles or bitmaps.

## Completed RGBA at 150 DPI

Median milliseconds. Previous SVG numbers include repeated export, validation
and decode; retained SVG numbers move those costs to setup.

| Page | CPU now | Hybrid now | Previous SVG | Retained SVG now | SVG samples |
|---|---:|---:|---:|---:|---:|
| Document 1 | 15.17 | CPU fallback | 120.04 | 13.56 | 100 |
| Document 2 | 3.40 | 6.49 | 29.72 | 14.54 | 100 |
| Document 3 | 19.63 | CPU fallback | 174.27 | 16.53 | 100 |
| Document 4 | 47.90 | 55.98 | 1,237.73 | 201.61 | 3 |

Retained SVG is competitive with fresh CPU pixels on the two image pages.
CPU remains substantially faster on the sparse and dense vector pages.
Document 4 improves by about 6.1 times relative to the initial SVG path,
while remaining roughly 4.2 times the CPU render time. Its SVG p95 is 203.43 ms;
with only three samples this is the maximum observed value, not a reliable
tail percentile. Other SVG p95s are 14.28, 15.68 and 18.00 ms respectively.

The dense page elides 4,459 clip applications and retains 1,845. The sparse
page elides 93 and retains 44. Image pages preserve their existing clip
applications. Geometry checks do not merge marks or alter their opacity.

At 300 DPI, the CPU / retained SVG medians are:

| Page | CPU | Retained SVG |
|---|---:|---:|
| Document 1 | 31.71 | 42.46 |
| Document 2 | 7.64 | 46.42 |
| Document 3 | 40.19 | 45.88 |
| Document 4 | 76.86 | 319.40 |

## Initial export and setup

The first 72-DPI request creates the resolution-independent SVG and embedded
images; subsequent DPI requests reuse that host artifact. First exports are
single observations, not medians. Initial completed setup includes export,
transport, validation, decode and first raster/readback at that resolution.

| Page | First export | Initial completed SVG setup at 72 DPI |
|---|---:|---:|
| Document 1 | 99.66 | 152.59 |
| Document 2 | 1.02 | 20.72 |
| Document 3 | 120.93 | 181.93 |
| Document 4 | 6.98 | 325.80 |

Resource reuse removes repeated image conversion/PNG encoding; it does not
remove the first-export cost. No JPEG bypass, new decoder, third-party crate
or cross-page resource cache is introduced.

## Viewport and zoom response

These probes end after two animation-frame opportunities in a clipped
480 by 480 viewport. They are scheduling/presentation proxies, not scanout or
proof that new-resolution SVG pixels have completed. Completed accuracy
readbacks are separate. The measured frame interval is about 16.665 ms, so
roughly 33 ms is the method's floor.

Each viewport path has 12 samples after two warmups. Zoom uses 18 samples
after three warmups at 150/225/300 DPI. CPU/hybrid rebuild and upload pixels;
SVG retains its source and changes viewport size. Median milliseconds:

| Page | Viewport CPU / hybrid / SVG | Zoom CPU / hybrid / SVG |
|---|---:|---:|
| Document 1 | 33.35 / fallback / 33.35 | 33.67 / fallback / 33.35 |
| Document 2 | 32.87 / 33.04 / 33.25 | 33.34 / 33.32 / 33.25 |
| Document 3 | 33.17 / fallback / 33.05 | 53.07 / fallback / 33.17 |
| Document 4 | 66.65 / 83.45 / 33.34 | 83.69 / 100.14 / 33.35 |

All SVG zoom probes complete 18 samples, with p95s 33.38, 34.08, 34.04 and
34.15 ms. Document 4's CPU/hybrid zoom p95s are 101.01/134.00 ms. Those results
show a much cheaper SVG viewport update, not a verified 33-ms crisp redraw.

The separate 60-frame CSS gesture scales existing content. Documents 1–3
complete every requested frame with no long intervals for any available path.
Document 4's SVG gesture reaches 42 frames before an interval above 250 ms
stops it; the report estimates 16 missed refresh intervals. The initial SVG
experiment stopped after six frames with a 1,083-ms interval. CPU/hybrid
bitmap previews complete all 60 frames in this run. SVG still has a long-frame
risk even after cleanup; a low median or p95 alone would hide it.

## Accuracy and repeatability

The exact CPU renderer and admitted hybrid outputs remain byte-identical in
every measured sample. SVG is still an approximate browser presentation.
At 150 DPI, pixels with any RGBA channel differing from CPU by more than one
on the 0–255 scale are:

| Page | Before | After | Mean channel error after | Maximum error after |
|---|---:|---:|---:|---:|
| Document 1 | 7.698656% | 7.698656% | 1.325 | 223 |
| Document 2 | 1.087032% | 1.087768% | 0.103 | 102 |
| Document 3 | 9.322722% | 9.322722% | 1.570 | 219 |
| Document 4 | 11.732869% | 11.736085% | 1.907 | 127 |

A separate same-browser comparison renders saved pre-cleanup SVGs and current
exports, with both fresh and retained sources, at 72/150/225/300 DPI. The two
image pages produce identical before/after pixels in that diagnostic. At
150 DPI, sparse/dense SVGs differ from their old SVG output by more than one
at 0.000460% / 0.025038% of pixels, with maximum local channel changes 18/48.
The bounds proof preserves the continuous geometric clip result; removing
clip operations can still change browser rasterization. No SVG pixel-identity
or content-conformance claim is made. These percentages include white page
area and are differential measurements, not percentages of incorrect content.

The SVG wire artifact is stable across native, SIMD WASI and scalar WASI.
Browser pixels remain less stable: Document 3 at 150 DPI has up to nine pixels
varying from the initial sample, with maximum channel delta 16. Other recorded
small variations remain in the raw report. Resource reuse does not establish
deterministic browser pixels. Untimed zoom readbacks report retained-source
accuracy and compare it with newly serialized SVG at each DPI.

## Validation and scope

- Six Rust tests cover geometry, conservative clip retention, limits, cached
  resolution reuse and page/document invalidation; strict host clippy and
  formatting pass.
- All 999 generated fixtures retain the same admission decisions: 706 SVG
  exports and 293 explicit refusals. Native/SIMD/scalar SVG and CPU hashes
  agree; CPU fixture hashes also match the pre-cleanup report.
- All four real page SVG export hashes agree between native, SIMD WASI and
  scalar WASI at 72/150/300 DPI, including pages exercising clip elision.
- The hardware GPU sweep passes 1,916 full-page/band/region variants across
  641 admitted fixtures, with an independent integer oracle and four malformed
  packets refused. Native CPU/packet parity also passes.
- Interactive checks preserve aspect ratio, retain the source URL, refresh
  zoom resolution/differences and expose the SVG export control.

The adapter remains outside production crates. Its resolution-independent
`export(Scene, Document)` produces owned SVG bytes and resource/clip statistics;
the host owns the one-page cache, and the browser owns presentation. This is
a useful boundary for carrying SVG export forward without adopting browser
SVG as the exact renderer. Existing unsupported PDF semantics remain explicit
whole-page refusals, and outlined text is not selectable browser text.

The exporter is worth retaining. The renderer experiment now has a practical
warm viewport path and competitive image-page readback, but slow dense-page
RGBA, occasional gesture stalls, browser-dependent pixels and incomplete PDF
semantics still prevent a default-backend recommendation.

Local ignored evidence under `target/gpu-demo/` includes
`svg-cleanup-bench.json`, its `-visuals/` directory and `svg-cleanup-demo.png`;
`svg-cleanup-accuracy.json` contains the before/after pixel diagnostic;
fixture, native/scalar parity, UI and GPU reports are stored alongside them.
Scalar one-sample real-PDF runs are parity checks, not performance comparisons.
No real PDF, SVG output or document image is redistributed in the repository.
