# CPU, WebGPU and SVG: local comparison

CPU remains the best fresh-rendering path on the four real PDF pages measured
here. Retaining an SVG source reduces zoom work on two of those pages, but
the dense vector page produces long browser stalls. SVG is useful as an
optional presentation experiment; these results do not justify replacing the
exact renderer or promoting SVG into product crates.

The adapter consumes the existing public `Scene`, keeping PDF interpretation,
resources and geometry shared. It emits vector paths, outline placements,
clips and supported RGB operations, with source image samples embedded as PNG.
The existing WebGPU experiment still uses exact CPU coverage and integer N16
composition. It is a hybrid compositor, not a GPU rasterizer.

## Setup and timing boundaries

Measured on 2026-10-22 at 22:57 UTC (2026-10-23 locally): Apple M3 Pro,
12 CPU cores, 18 GPU cores, 18 GB; macOS 15.6.1; Chrome 155.0.8059.85,
hardware Metal adapter, no software GPU fallback; Rust 1.99.1 and Node 26.8.1.
Other rendering work was active on this desktop. This is an exploratory
comparison, not a release gate or a cross-machine result.

The timed revision is `68eb856200e36fb6c6ce9c3bc2c05e7311b8f7a8`.
The runner captured source hashes before the run and verified they were
unchanged at completion. WASM SHA-256:
`7ba5124ee6ff85a216d27f0a599f066b585be902f2f3bec6ab42ac5ead32a748`.
Subsequent changes make zoom heatmaps sample the retained image, add the same
untimed diagnostic to the zoom report, and close Chrome's inherited stderr
pipe after exit; they do not change the timed rendering implementations.

There are 27 resolution/workload cells: five generated controls and the first
page of four real PDFs, each at 72, 150 and 300 DPI. CPU has 100 samples after
five warmups in every cell; the 15 admitted hybrid cells also have 100.
SVG has 100 except the dense page, where the explicit slow-raster policy caps
each resolution at three samples after one warmup. This gives 2,700 CPU,
1,500 hybrid and 2,409 SVG timed samples. Backend order rotates and reverses.

PDF opening, compilation, adapter setup and first GPU allocation are outside
warm timings. Scene recording and resources are warm. CPU includes preparation
and execution. Hybrid includes packet construction, upload, completed GPU work
and RGBA readback. SVG includes export, XML validation/parse, image decode,
Canvas rasterization and completed RGBA readback. These render timings exclude
worker transport; the raw report records transport-inclusive timings too.
The browser chooses its SVG/Canvas rendering implementation; the SVG column
is not evidence that every operation ran on the GPU.

## Fresh rendering on real PDFs

Milliseconds, median (p95), at 150 DPI. The labels also appear in the local
demo. A fallback is never counted as a GPU render.

| Page | CPU | Hybrid WebGPU | SVG | SVG samples |
|---|---:|---:|---:|---:|
| Document 1: text and images | 14.59 (15.56) | CPU fallback | 120.04 (139.27) | 100 |
| Document 2: sparse vectors | 3.14 (3.39) | 6.16 (6.80) | 29.72 (32.70) | 100 |
| Document 3: text and images | 18.75 (21.21) | CPU fallback | 174.27 (209.91) | 100 |
| Document 4: dense vectors and text | 46.88 (58.78) | 55.21 (74.92) | 1,237.73 (1,590.33) | 3 |

SVG takes 8.2–26.4 times the CPU render time on these pages. At 300 DPI the
CPU/SVG pairs are 30.88/149.10, 7.19/64.84, 39.21/197.14 and
76.96/1,979.33 ms respectively. The dense page remains a three-sample result;
its p95 is the maximum observed sample, not a well-estimated tail percentile.

Image conversion/PNG export dominates Documents 1 and 3: median SVG export
costs 80.48 and 115.65 ms, versus browser costs of 38.55 and 58.00 ms.
Retaining the source avoids that repeated export work. Document 4 is the
opposite: export costs 6.55 ms and browser work 1,231.29 ms, with 165 paths,
5,778 glyph placements and no images. Stage medians need not sum to the
median of totals. No single browser rasterization cause was isolated.

The generated image control is a useful counterexample to a universal CPU
claim: at 300 DPI CPU takes 51.85 ms and SVG 40.50 ms. The workload and output
requirements matter. Hybrid rendering remains slower than CPU in all 15
admitted cells of this run; its compute-only kernel time is not an end-to-end
speedup.

## Viewport presentation and zoom

These measurements end after two animation-frame opportunities in a clipped
480 by 480 viewport, alternating the top and centre of the page. They do not
measure scanout or prove browser/GPU completion. The measured refresh interval
was about 16.665 ms, so results near 33 ms are limited by this timing method.

Fresh presentation rebuilds backend output from a warm scene and creates a
new SVG image, without forcing full-page SVG readback. Zoom retains one SVG
source and changes its viewport size at 150/225/300 DPI; CPU and hybrid
produce fresh full-page pixels and upload them to Canvas. Neither pixel path
has a viewer tile/bitmap cache. Reusing cached resolutions could reduce their
cost on revisited scales.

| Page | Fresh viewport CPU / hybrid / SVG, median ms | Zoom CPU / hybrid / SVG, median ms |
|---|---:|---:|
| Document 1 | 33.20 / fallback / 133.36 | 65.55 / fallback / 33.48 |
| Document 2 | 33.61 / 33.26 / 33.70 | 33.69 / 33.62 / 33.28 |
| Document 3 | 33.31 / fallback / 183.17 | 66.89 / fallback / 33.25 |
| Document 4 | 82.74 / 350.47 / 141.25 | 107.69 / 610.80 / 41.85 |

Fresh viewport samples are 12 per path except Document 4 hybrid, which stops
at three after a slow frame (p95 982.37 ms). Zoom has 18 samples per path on
Documents 1–3. Document 4 stops after 13 CPU, three hybrid and six SVG samples;
their p95s are 738.20, 635.45 and 538.44 ms. Once a frame exceeds 250 ms, a
path stops when it has at least three samples; already collected samples are
retained. These unstable tails prevent a reliable zoom speedup claim for that
page.

A separate 60-frame CSS gesture scales existing content for all paths.
Documents 1–3 record median intervals of 16.665 ms and no long frames for
any available backend. Document 4's CPU and hybrid bitmap previews also do,
but SVG encounters a 1,083.29 ms interval and stops after six frames. Bitmap
scaling is a responsive preview, not newly rendered detail. Retained vectors
also cannot guarantee smooth interaction on an arbitrarily complex page.

## Visual differences

CPU/hybrid outputs remain byte-identical for every admitted measured render.
SVG uses browser antialiasing, stroke placement, blending and image sampling;
image resources are quantized to eight-bit straight RGBA. PDF zero-area fill
behaviour also differs. The following values compare completed SVG snapshots
with CPU at 150 DPI, including the white page area. A differing pixel has at
least one RGBA channel whose absolute difference exceeds one on the 0–255
scale. This is not a percentage of incorrect content or an independent
accuracy oracle.

| Page | Pixels differing by >1 | Mean RGBA channel error | Maximum channel error |
|---|---:|---:|---:|
| Document 1 | 7.699% | 1.325 | 223 |
| Document 2 | 1.087% | 0.103 | 102 |
| Document 3 | 9.323% | 1.570 | 219 |
| Document 4 | 11.733% | 1.906 | 127 |

Inspection of page images and amplified differences preserves the visible
text, layout and charts on these four pages, with prominent differences along
glyph and shape edges. This small sample does not establish general PDF
fidelity. Native browser pixels are not an exact-mode implementation.

An additional diagnostic at revision
`7bb49348650041f3fd1ca233a25083b13ecf9c32` samples the actual retained image
at zoom, keeping its source URL unchanged. Source hashes stayed unchanged.
This uses the normal benchmark runner with three timing samples; the fidelity
readbacks themselves are untimed. Its pixels differing from CPU by >1 at
150 / 225 / 300 DPI are:

| Page | Retained SVG differences against CPU |
|---|---:|
| Document 1 | 7.699% / 5.289% / 4.074% |
| Document 2 | 1.087% / 0.733% / 0.547% |
| Document 3 | 9.323% / 6.248% / 4.814% |
| Document 4 | 11.733% / 7.993% / 6.014% |

Retained and freshly serialized SVG match exactly at 150 DPI in that check.
At other zooms, up to 0.030% of pixels differ by >1 between them, with a
maximum local channel difference of 213. They are not interchangeable fidelity
measurements. The demo's zoom heatmap and subsequent zoom reports now use
retained-source readback and record the fresh-versus-retained difference too.

Export bytes repeat exactly. Browser pixels sometimes vary even for identical
SVG bytes: one sample each in the generated vector chart and Document 3 at
72 DPI changed by up to seven channel levels. The report records these
variations and accuracy ranges instead of treating browser output as
deterministic. CPU/hybrid byte checks remain strict.

## Coverage, validation and evidence

The SVG sweep visits all 999 generated, unpassworded rendering fixtures:
706 exports are admitted and 293 refuse with explicit semantic or budget
reasons, with zero harness failures. Admission is not a fidelity pass. Native,
SIMD WASI and scalar WASI agree on all decisions, all 706 SVG export hashes
and CPU reference hashes. Independent colour/isolated-page blend checks and
active/external SVG resource refusal checks pass. The interactive controls
preserve aspect ratio and the retained source and refresh pixel resolution.

The existing GPU sweep still passes 1,916 full-page, band and region variants
over 641 admitted fixtures. Its independent integer oracle covers all N16
output words and all coverage bytes, with malformed packets refused before
dispatch. Rust SVG host tests, strict host clippy and formatting pass. This
extension changes no production rendering path and adds no third-party crate.

Shadings, tiling paints, non-RGB groups, soft-mask groups, transfers, unsupported
knockout/overprint, fixed-view annotations and several image-mask/alpha cases
remain explicit whole-page SVG refusals. Outlined text is not selectable
browser text. The exact CPU renderer remains available for full semantics.

Local ignored evidence under `target/gpu-demo/`: `three-backends.json` contains
raw samples, hashes, phase timings, counts and p95s; `three-backends.png`
captures the demo; `three-backends-visuals/` contains CPU/SVG PNGs, amplified
differences and SVG exports. `retained-accuracy.json` records the separate
zoom fidelity probe, reproduced by `retained-zoom-check.json` through the
maintained runner. SVG fixture, parity, UI and GPU verification reports
are stored alongside them. Real PDFs and their output images are not
redistributed. See [`README.md`](README.md) for reproduction commands and
[`policy.json`](policy.json) for timing policy.

For a short retained-source diagnostic, run `bench --real-only --dpis 150
--samples 3 --warmups 1 --zoom-samples 3 --gesture-frames 3` with the desired
`--pdf` inputs and an `--output` path. Use the full sample counts in the README
for a performance comparison; this short diagnostic is for fidelity checks.
