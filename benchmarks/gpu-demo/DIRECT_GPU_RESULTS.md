# Direct GPU presentation and fixed world geometry

Direct presentation is worth retaining. On the dense 300-DPI page, median
browser frame opportunities improve from 66.66 ms with CPU reuse to 33.32 ms
with retained direct GPU; native completion/presentation callbacks improve
from 74.69 to 41.70 ms. World geometry reduces resident resources and improves
some sparse-page updates, but does not consistently outperform resolution-
specific retained geometry. It still needs CPU preparation for a new view.
Its tested error reaches two channel levels, so it remains approximate.

This experiment adds direct WebGPU canvas and native CAMetalLayer display to
the retained coverage backend, and a separate fixed world-geometry packet.
The CPU control reuses the same prepared scene, Scratch and output allocation.
Every measured frame rasterizes fresh whole-page pixels; no path caches RGBA.
SVG export remains independent and available.

The code checkpoint is `15a12337d3645d55abe19bd6e6c00982053a5e9f`, branched
from `render/one-pass` at `87fa81b64162281fccbdcae5564e1ec431022c4c`.
The shared desktop uses an Apple M3 Pro (12 CPU / 18 GPU cores, 18 GB), macOS
15.6.1, Chrome 155 and hardware Metal 3. These are exploratory measurements,
not release-gate evidence. The original four local inputs keep their earlier
anonymous ordering. Only page 0 is tested, on opaque white, at 72/150/300 DPI.
Inputs and generated real-document images remain outside Git. A subsequent
change only hardens native view-size validation against integer overflow;
the shader and drawing path are unchanged, and native fixture parity was
rerun after that guard change.

## What changed

- Direct presentation reads the compute output buffer in a fullscreen render
  pass. Timed frames perform no RGBA copy/map and no timestamp-query mapping.
  Retained GPU with readback is kept as an otherwise equivalent control.
- The new world packet retains one canonical 300-DPI vectorization, shared
  glyph sources, source-local edge rows and conservative fixed tile bins.
  Geometry is uploaded once per current document/page. One maximum-size GPU
  output allocation is overwritten at each integer resolution from 72–300 DPI.
  At most three view-parameter buffers remain resident; they contain no edges.
- New views still run existing CPU preparation and export sparse exceptional
  coverage cells and placement metadata. Revisited views make no engine call.
  Thus GPU geometry is independent of zoom, but CPU certification is still
  resolution dependent. General cubics retain the existing fixed flattening;
  the experiment does not add arbitrary-zoom adaptive subdivision.
- World admission explicitly refuses device hairlines, adjusted strokes,
  fixed-size annotations, zero-area fills/clips/shading bounds and active
  nonrectangular clips. Sampled paints and existing precision/compositing
  restrictions retain explicit CPU fallback. Refusals never show a partial page.
- All new Rust is safe and remains behind the optional `gpu-demo` feature.
  GPU APIs stay in JavaScript and an isolated Swift host using system frameworks.
  There is no Rust FFI, new product dependency or change to Exact rendering.
  Swift's ordinary Metal buffer access does not introduce unsafe Rust.

## Measurement

Each static comparison has 100 samples after five warmups, rotating and
reversing CPU, GPU readback, retained direct GPU and world direct GPU order.
First preparation/export/upload/pipeline/first draw costs are reported
separately. Source and binary hashes must remain unchanged throughout a run.

All paths display the same 480×480 top or centre PDF viewport. CPU and readback
controls crop and upload fresh viewport pixels, rather than a full-page display
texture. GPU paths still compute the entire page, then sample that viewport.
The browser ends timing after GPU completion and two animation-frame
opportunities. Native uses an actual AppKit window/CAMetalLayer with an 8-pixel
alternating footer outside the PDF viewport. It records completion and
presentation-or-discard callbacks plus IPC, keeps every timed attempt, and
reports presented-only drawable duration separately. These frame clocks are
not directly comparable across hosts and do not measure display scanout.

Native drawable `presentedTime == 0` means a drawable was not presented or was
dropped ([Apple documentation](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime)).
Initial warmup handles window initialization, records those discards, and is
separate from samples. Timed discards are never retried or filtered. A
per-request autorelease pool releases Metal drawables promptly.

“Work” below is CPU engine preparation/execution time or GPU submission through
completion, including any new packet/view preparation needed by that sample.
“Frame” adds transport, crop/upload where needed, and the host's display wait.
Native CPU frames cross the isolated process pipe with full-page RGBA; a future
in-process native API would have different transport costs. Work and frame
columns should therefore be read together.

Novel zoom measures twelve previously unrequested targets:
151, 163, 177, 191, 207, 221, 239, 253, 269, 281, 293 and 299 DPI.
Prepared CPU state is cleared independently before every mode/target; recorded
scene, Store and pipelines stay warm. World geometry stays resident. New view
preparation, patch export/upload and display all count. The revisit study
warms 150/225/300 DPI, then measures 18 updates after three warmups. Each mode
retains the same three views. Differential readbacks and presentation captures
occur only after timing.

## Dense page at 300 DPI

Document 4, milliseconds as median / p95. Native and browser frame clocks differ.

| Host | Path | Work | Frame |
|---|---|---:|---:|
| Browser | CPU prepared | 47.07 / 56.96 | 66.66 / 83.31 |
| Browser | GPU readback | 20.68 / 23.22 | 33.32 / 33.50 |
| Browser | Retained direct | 15.98 / 18.40 | 33.32 / 33.60 |
| Browser | World direct | 21.07 / 23.50 | 33.31 / 33.90 |
| Native | CPU prepared | 41.56 / 57.23 | 74.69 / 91.81 |
| Native | GPU readback | 22.37 / 33.19 | 58.57 / 74.53 |
| Native | Retained direct | 18.61 / 29.69 | 41.70 / 50.24 |
| Native | World direct | 50.99 / 84.10 | 74.96 / 107.76 |

Avoiding readback reduces 300-DPI dense GPU work from 20.68 to 15.98 ms in the
browser and 22.37 to 18.61 ms natively. The native world shader is slower than
the retained per-resolution shader, even though it uploads less geometry.
The three-resolution warm zoom study below remains a different workload.

## Full static results

Documents 1 and 3 contain sampled paints and retain CPU fallback at every DPI.
Document 2 is sparse vectors; Document 4 is dense vectors/text. Values are
milliseconds as median / p95. The dense 150-DPI full-run timing drift is
preserved in these tables and investigated immediately below them.

### Browser work

| Document | DPI | CPU prepared | GPU readback | Retained direct | World direct |
|---|---:|---:|---:|---:|---:|
| 1 | 72 | 9.54 / 10.31 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 150 | 14.62 / 14.96 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 300 | 30.21 / 31.57 | CPU fallback | CPU fallback | CPU fallback |
| 2 | 72 | 2.51 / 3.72 | 3.75 / 5.59 | 3.17 / 5.06 | 2.50 / 4.12 |
| 2 | 150 | 5.42 / 8.05 | 7.99 / 10.03 | 5.37 / 6.54 | 3.54 / 5.06 |
| 2 | 300 | 8.19 / 11.72 | 14.48 / 19.09 | 8.41 / 14.74 | 3.83 / 6.83 |
| 3 | 72 | 9.19 / 11.91 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 150 | 17.33 / 18.15 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 300 | 37.93 / 39.94 | CPU fallback | CPU fallback | CPU fallback |
| 4 | 72 | 15.54 / 24.82 | 6.33 / 8.23 | 5.93 / 7.31 | 11.80 / 20.00 |
| 4 | 150 | 26.69 / 42.57 | 29.77 / 33.12 | 27.69 / 31.89 | 41.69 / 48.20 |
| 4 | 300 | 47.07 / 56.96 | 20.68 / 23.22 | 15.98 / 18.40 | 21.07 / 23.50 |

### Browser frame

| Document | DPI | CPU prepared | GPU readback | Retained direct | World direct |
|---|---:|---:|---:|---:|---:|
| 1 | 72 | 33.32 / 33.88 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 150 | 33.32 / 34.10 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 300 | 66.67 / 67.17 | CPU fallback | CPU fallback | CPU fallback |
| 2 | 72 | 33.29 / 33.98 | 33.31 / 34.13 | 33.27 / 34.19 | 33.26 / 33.96 |
| 2 | 150 | 33.30 / 33.47 | 33.31 / 33.57 | 33.29 / 33.48 | 33.30 / 33.49 |
| 2 | 300 | 33.34 / 34.00 | 33.33 / 34.03 | 33.32 / 34.08 | 33.27 / 34.05 |
| 3 | 72 | 33.32 / 34.12 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 150 | 33.33 / 34.16 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 300 | 66.66 / 67.53 | CPU fallback | CPU fallback | CPU fallback |
| 4 | 72 | 33.29 / 33.83 | 33.32 / 34.10 | 33.29 / 34.01 | 33.32 / 34.21 |
| 4 | 150 | 33.83 / 66.87 | 33.29 / 66.84 | 33.29 / 50.12 | 66.25 / 67.48 |
| 4 | 300 | 66.66 / 83.31 | 33.32 / 33.50 | 33.32 / 33.60 | 33.31 / 33.90 |

### Native work

| Document | DPI | CPU prepared | GPU readback | Retained direct | World direct |
|---|---:|---:|---:|---:|---:|
| 1 | 72 | 7.49 / 10.54 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 150 | 11.35 / 12.18 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 300 | 23.63 / 25.39 | CPU fallback | CPU fallback | CPU fallback |
| 2 | 72 | 0.69 / 0.74 | 2.48 / 3.09 | 3.15 / 3.68 | 3.11 / 3.68 |
| 2 | 150 | 1.85 / 3.28 | 5.79 / 6.59 | 5.52 / 6.31 | 6.05 / 7.44 |
| 2 | 300 | 7.44 / 10.06 | 15.31 / 20.75 | 10.28 / 16.53 | 9.83 / 16.05 |
| 3 | 72 | 7.79 / 8.42 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 150 | 13.57 / 14.17 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 300 | 26.67 / 27.63 | CPU fallback | CPU fallback | CPU fallback |
| 4 | 72 | 15.71 / 21.24 | 5.39 / 6.17 | 5.70 / 6.49 | 14.41 / 19.04 |
| 4 | 150 | 20.61 / 32.63 | 32.40 / 36.18 | 31.06 / 35.61 | 89.18 / 100.28 |
| 4 | 300 | 41.56 / 57.23 | 22.37 / 33.19 | 18.61 / 29.69 | 50.99 / 84.10 |

### Native frame

| Document | DPI | CPU prepared | GPU readback | Retained direct | World direct |
|---|---:|---:|---:|---:|---:|
| 1 | 72 | 33.22 / 40.79 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 150 | 33.51 / 41.53 | CPU fallback | CPU fallback | CPU fallback |
| 1 | 300 | 58.33 / 59.37 | CPU fallback | CPU fallback | CPU fallback |
| 2 | 72 | 24.94 / 25.72 | 24.93 / 25.86 | 25.00 / 25.85 | 25.02 / 25.87 |
| 2 | 150 | 25.22 / 33.54 | 33.07 / 34.19 | 25.00 / 25.80 | 25.14 / 25.91 |
| 2 | 300 | 49.62 / 50.62 | 57.39 / 59.20 | 33.17 / 34.28 | 33.13 / 34.10 |
| 3 | 72 | 33.33 / 34.23 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 150 | 41.58 / 42.88 | CPU fallback | CPU fallback | CPU fallback |
| 3 | 300 | 58.36 / 59.36 | CPU fallback | CPU fallback | CPU fallback |
| 4 | 72 | 41.16 / 49.50 | 33.22 / 34.21 | 24.92 / 25.72 | 33.59 / 42.07 |
| 4 | 150 | 49.46 / 59.08 | 58.01 / 67.11 | 49.53 / 58.61 | 108.58 / 121.31 |
| 4 | 300 | 74.69 / 91.81 | 58.57 / 74.53 | 41.70 / 50.24 | 74.96 / 107.76 |

### Dense 150-DPI timing drift and bounded repeat

The full run is not stationary: browser retained-direct work changes from a
7.85 ms median in its first ten samples to 29.46 in its last ten. Native
retained-direct changes from 8.70 to 32.39 ms. World work also drifts. No
samples are removed. One additional 100-sample/five-warmup run per host, with
the same code and dense input at 150 DPI, is reported separately. It is stable
across its first/last ten samples; this experiment does not establish the
cause of the full-run drift. Do not infer a universal API speedup or use only
the favorable repeat. A dedicated performance host is needed for that claim.

| Host, repeat | Path | Work | Frame |
|---|---|---:|---:|
| Browser | CPU prepared | 26.57 / 35.90 | 33.42 / 66.77 |
| Browser | GPU readback | 9.85 / 10.84 | 33.33 / 34.00 |
| Browser | Retained direct | 8.84 / 9.83 | 33.31 / 33.96 |
| Browser | World direct | 13.07 / 21.45 | 33.30 / 33.86 |
| Native | CPU prepared | 24.50 / 31.57 | 50.20 / 58.89 |
| Native | GPU readback | 8.61 / 9.59 | 33.38 / 41.42 |
| Native | Retained direct | 8.44 / 9.25 | 32.72 / 33.98 |
| Native | World direct | 23.45 / 26.18 | 41.96 / 50.49 |

## Zoom responsiveness

Milliseconds as median / p95 of the host frame clock. New targets include CPU
preparation and view uploads; revisits reuse the same three target DPIs.
Only direct paths appear here, avoiding shared setup between a readback and
direct control at a novel target. There is no application bitmap cache.

| Host | Document | Targets | CPU prepared | Retained direct | World direct |
|---|---:|---|---:|---:|---:|
| Browser | 1 | 12 new | 39.19 / 67.53 | CPU fallback | CPU fallback |
| Browser | 1 | 18 revisits | 33.68 / 67.43 | CPU fallback | CPU fallback |
| Browser | 2 | 12 new | 32.97 / 49.92 | 49.58 / 49.89 | 33.07 / 33.31 |
| Browser | 2 | 18 revisits | 33.30 / 66.63 | 33.30 / 33.50 | 33.33 / 33.56 |
| Browser | 3 | 12 new | 66.16 / 66.92 | CPU fallback | CPU fallback |
| Browser | 3 | 18 revisits | 34.26 / 66.95 | CPU fallback | CPU fallback |
| Browser | 4 | 12 new | 83.14 / 100.46 | 91.62 / 116.59 | 91.38 / 116.44 |
| Browser | 4 | 18 revisits | 66.41 / 67.30 | 33.34 / 33.94 | 33.29 / 33.92 |
| Native | 1 | 12 new | 49.74 / 58.97 | CPU fallback | CPU fallback |
| Native | 1 | 18 revisits | 42.17 / 65.87 | CPU fallback | CPU fallback |
| Native | 2 | 12 new | 34.04 / 42.40 | 40.78 / 41.94 | 33.00 / 33.98 |
| Native | 2 | 18 revisits | 41.42 / 58.13 | 32.76 / 34.22 | 26.09 / 34.33 |
| Native | 3 | 12 new | 50.20 / 66.41 | CPU fallback | CPU fallback |
| Native | 3 | 18 revisits | 50.17 / 59.20 | CPU fallback | CPU fallback |
| Native | 4 | 12 new | 83.50 / 100.08 | 83.45 / 99.43 | 91.97 / 116.86 |
| Native | 4 | 18 revisits | 62.61 / 84.04 | 33.63 / 42.50 | 58.29 / 75.69 |

Dense new-target GPU medians remain slower than, or effectively tied with,
CPU reuse. Dense browser revisits improve from 66.41 ms CPU to 33.34 ms
retained direct and 33.29 ms world direct. Native revisits favor retained
direct (33.63 ms); world direct reaches 58.29 ms versus 62.61 ms CPU. Sparse
native new-view world updates reach 33.00 ms versus 34.04 ms CPU and 40.78 ms
resolution-specific GPU. Small differences at browser frame floors should
not be interpreted as precise scanout improvements.

## Retained resources and setup

The world packet is 1,109,892 bytes on Document 2 and 4,754,212 on Document 4.
It contains respectively 133/5,943 marks, 18,001/91,791 edges and 119/1,074
shared sources. Across all novel and revisited DPIs, counters remain at one
geometry upload per page. The twelve new views upload 81,184 and 7,217,728
bytes of parameters/patches respectively. Warm revisits upload neither.
Each static world block adds 105 dispatches with no new view upload or RGBA
readback. Retained direct similarly adds no RGBA readback; only its explicit
readback control does. Diagnostic accuracy work increases counters afterward.

Nominal GPU bytes before the revisit phase, with three views retained:

| Host | Document | Resolution-specific retained bytes | World bytes |
|---|---:|---:|---:|
| Browser | 2 | 128,678,208 | 35,925,408 |
| Browser | 4 | 138,526,776 | 41,349,572 |
| Native | 2 | 65,595,984 | 35,925,376 |
| Native | 4 | 75,421,264 | 41,349,540 |

These are buffer-accounting values, not RSS. The browser retained control
still allocates readback capacity, whereas world diagnostic staging is
transient. The difference includes allocation policy and one shared maximum
output versus three outputs, not only source sharing. CPU uses the same
three-entry prepared LRU; its charged reservation metric is capped at
256 MiB and excludes separately bounded Store/output storage. Native and
WASM reservation sizes differ with pointer-sized storage. At 150 DPI the
CPU control reserves roughly 20–24 MB per prepared scene plus about 8.7 MB
output capacity. These metrics must not be equated with GPU nominal bytes.

First world setup request, with prior CPU/retained setup already run:

| Host | Document | CPU canonical prepare | World export | GPU validate/retain | Full setup request |
|---|---:|---:|---:|---:|---:|
| Browser | 2 | 2.85 | 5.19 | 6.59 | 19.90 |
| Browser | 4 | 43.56 | 9.43 | 1.97 | 73.24 |
| Native | 2 | 1.77 | 3.60 | 4.17 | 18.08 |
| Native | 4 | 38.94 | 13.98 | 115.10 | 224.31 |

Setup order is CPU, retained GPU, then world GPU; those one-time numbers are
context, not an order-controlled cold-start comparison. Pipeline initialization,
first presentation, first view/patch preparation and all raw samples remain
in JSON. The native shader host initializes in the same process for the run;
the first window/drawable warmup is separate from timed samples.

## Accuracy

Fresh and prepared CPU pixels match byte-for-byte. CPU pixels, immutable world
packets and sparse view packets match across native, SIMD-WASM and scalar-WASM
for all twelve real-page/resolution decisions. GPU arithmetic can still differ
between Metal and WebGPU. The maximum tested retained-coverage difference is
one channel level; the world maximum is two across static and zoom views.
There is no global error bound and no Exact conformance claim for either.

World output against CPU, changed-pixel percentage and maximum channel error:

| Host | Document | DPI | Changed pixels % | Beyond one % | Maximum error | RMSE |
|---|---:|---:|---:|---:|---:|---:|
| Browser | 2 | 72 | 0.076159 | 0.001196 | 2 | 0.020871 |
| Browser | 2 | 150 | 0.043654 | 0.000276 | 2 | 0.015665 |
| Browser | 2 | 300 | 0.001379 | 0.000000 | 1 | 0.002588 |
| Browser | 4 | 72 | 0.850683 | 0.000399 | 2 | 0.079457 |
| Browser | 4 | 150 | 0.277482 | 0.000000 | 1 | 0.045250 |
| Browser | 4 | 300 | 0.002666 | 0.000000 | 1 | 0.004318 |
| Native | 2 | 72 | 0.076159 | 0.001196 | 2 | 0.020871 |
| Native | 2 | 150 | 0.043746 | 0.000276 | 2 | 0.015676 |
| Native | 2 | 300 | 0.001357 | 0.000000 | 1 | 0.002565 |
| Native | 4 | 72 | 0.850484 | 0.000399 | 2 | 0.079447 |
| Native | 4 | 150 | 0.277482 | 0.000000 | 1 | 0.045250 |
| Native | 4 | 300 | 0.002528 | 0.000000 | 1 | 0.004196 |

The dense 150-DPI world result changes about 0.2775% of pixels, predominantly
by one level; the 72-DPI result changes about 0.8507%. Resolution-specific
retained geometry is closer to CPU. Untimed top/centre attachment captures
match the corresponding GPU viewport bytes exactly on both hosts, and CPU
viewport uploads also match exactly. These captures validate the attachment
that is submitted, not a screen/color-profile/scanout round-trip.

An initial raw-scaling probe that omitted per-view CPU exceptional cells
produced errors up to 44 levels on the sparse real page and 130 on the dense
page. Those results remain local as diagnostic evidence; that mode is not
exposed as the world backend. Keeping sparse corrections is material to
accuracy, and is why this step does not eliminate all CPU preparation.

## Validation and limits

- 185 paint/raster tests pass with the optional feature, five ignored; default
  feature tests also pass. Eleven host tests cover lifetime, invalidation,
  refusal, exact CPU reuse and the independent SVG exporter.
- Strict feature-enabled core and host Clippy, formatting, 32 JavaScript module
  syntax checks and Swift warnings-as-errors pass. Product policy verifies
  24 workspace-only packages, unsafe prohibition and 4,566 fixture cases.
- World fixture sweep: 999 decisions, 569 admissions and 430 explicit refusals.
  Among admissions, 558 are pixel-identical and 11 differ by at most one level.
  Another 240 zoom variants have maximum error one. Native and scalar-WASM
  admission/CPU/world/patch parity pass. An independent polygon/placement oracle
  is exact at six DPIs; ten malformed world packets and two oversized native
  view requests are refused, including maximal-u32 dimensions without overflow.
- Existing retained coverage regression: 639 admissions, 360 refusals and
  320 band/region variants, plus native Metal parity. Original exact integer
  compositor regression also passes, including its independent N16 oracle.
  Real demo controls, live 173-DPI world preview, novel/revisited zoom and
  retained SVG/download availability were exercised and visually inspected.
- Every main native timed attempt was presented: 3,240/3,240, zero discarded.
  The bounded repeat adds 400/400 presented, also zero discarded. Initial
  window warmup discards are recorded separately; none is silently retried
  inside timed sampling.
- `python3 tools/check.py` passes policy, signature vectors, fixture regeneration
  and bundled-data verification, then stops because `uv` is unavailable for
  Python lint. It is not a full product-check pass.
- This draft remains pinned to the original one-pass checkpoint. The PR base
  has since advanced substantially; its Linux merge-result feature build
  exposes changed internal geometry types ([earlier Linux run](https://github.com/jonasgrosch/pagura/actions/runs/39886568579/job/120196005490)).
  Porting the optional adapters to
  that base and rerunning CI is required before integration. Local optional-
  feature checks pass on the measured branch. No release-gate claim is made.

Direct presentation is the stronger result to carry forward. World geometry
provides cleaner lifetime ownership and less repeated geometry traffic, but
its CPU view certification, narrower semantic support, fixed vectorization
range, setup cost and native shader cost still need work. Profiling source
row traversal/clip transforms, handling device-dependent marks explicitly,
and eventually moving view certification or subdivision to a bounded GPU
stage are separate experiments. The measurements do not justify replacing
CPU Exact or the better-performing resolution-specific retained path.

## Local evidence

Raw JSON, timings, binaries and screenshots stay in ignored `target/gpu-demo/`.
The audit checks frozen code/binaries, sample counts, transfer counters,
viewport bytes and cross-platform packet/pixel parity. Scalar real-page runs
are parity checks with one sample, not additional performance evidence.

| Evidence file | SHA-256 |
|---|---|
| `display-browser.json` | `596a41c0f88b763eb9f5ee46d0b0ec8ca0403b7221f696ac6d8e40ff7fd71e53` |
| `display-native.json` | `ffc50855651d9e6b84c0f7b6ff5629f6ab7fe45986355c0ce9e7e9f2dd543f2f` |
| `display-browser-repeat.json` | `e239599989c030a67a756ee702774df4c7d1bc3ff5b9167768e0cb12221eca88` |
| `display-native-repeat.json` | `68cf3c33397a06f0a07eb69bedb2a8c65930d224741d6ccf8f45b2a12530e51f` |
| `display-scalar-real.json` | `31ad4c6a83a5fb6395afbb1ddb6a48002e21d9528608176cf61331bdf1a85d73` |
| `display-fixtures.json` | `23def849601fcf1caa9a2ae361b287e3e2496e99515232352c592e0718826a83` |
| `display-scalar-fixtures.json` | `1f0eb76a516792acd543bcf3ccafb029dba6f873b79664be16eb7f07e434c617` |
| `display-native-fixtures.json` | `a976bf83abb26fbdcedf36bd747b128cfb57d7c090c6920b7cb19cc38fe80aad` |
| `display-retained-regression.json` | `e268c3ba3032293f83c57244ae08db79c5422a587321b4241f1a14dca6070bf4` |
| `display-integer-regression.json` | `298b225f8903db832681fce1df6eaf3daae436cf2ced6f13528135cc65391922` |
| `display-native-retained-regression.json` | `cd63920c2bad035b924a51e2aaf799cf2814b343a329153f4b56db87c5a2be2c` |
| `display-ui.json` | `fa17a60c388264f8291617dc56273b708d9dd21507ca930a5d1b0d1422598d8f` |
| `display-audit.json` | `e74f4ecbaf0946658b754a5b8c22db92f14c0a8a121fb1b67b253eef72750e67` |
