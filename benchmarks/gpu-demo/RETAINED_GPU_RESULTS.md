# Retained GPU geometry and equivalent CPU reuse

This follow-up compares the same four local documents used in
[the fresh-export coverage experiment](GPU_COVERAGE_RESULTS.md). Every path
builds new pixels on each redraw. CPU retains the immutable `PreparedScene`,
Scratch and output allocation. WebGPU and native Metal retain validated source
geometry and output/readback buffers. No result bitmap is cached.

The default demo now selects shared, retained geometry. The SVG exporter and
its resource reuse remain independent; its timings are recorded in
[the SVG cleanup report](SVG_CLEANUP_RESULTS.md).

## Architecture and scope

The optional geometry seam remains in `pagura-raster` and the bounded packet
producer in `pagura-paint`. GPU APIs live in the experimental JavaScript and
Swift hosts. No product dependency, FFI or unsafe Rust was added. The default
product renderer and Exact accuracy contract are unchanged.

Version 3 shares each certified, translation-free glyph source across its
placements. Fractional placement offsets are separate from integer pixel
origins; origin subtraction happens before f32 narrowing. Non-glyph row slices
reference an edge pool instead of duplicating edge payloads. Source pointers
are temporary lookup keys; serialized IDs follow deterministic first use.
Version 2 expanded geometry is retained as a control.

Each CPU/GPU cache keeps at most three resolution/region requests. The CPU
cache caps the existing `render_bytes` preparation reservation metric at
256 MiB. This is not physical memory or RSS: the separately bounded Store,
Scratch and one output Vec also consume memory. GPU cache limits cover nominal
source/output/readback buffer sizes, capped at 256 MiB. Buffer metadata,
driver allocations, transient validation copies and host-owned RGBA snapshots
are outside that nominal sum. The comparison retains CPU preparation as well
as GPU resources, so these columns are not alternative total memory footprints.
Eviction and page/document changes explicitly release retained resources.
The native host independently enforces three handles and its buffer cap.

This is still a hybrid, approximate f32 coverage experiment. Existing f64 CPU
preparation outlines strokes, certifies geometry and handles exceptional
winding/joint-clip cells. Unsupported sampled paints, groups and other PDF
semantics explicitly use the complete CPU result. Sharing does not broaden
admission or make this an Exact backend. Native Metal's double-precision probe
remains unsupported on this M3 Pro; no native FP64 rasterizer is claimed.

## Measurement

All documents use page 0, opaque white, at 72/150/300 DPI. Static redraws use
100 samples after five warmups, rotating and reversing CPU/expanded/shared
order. The initial setup order rotates by case. PDF opening, recorded scene,
Store and pipelines are warm; preparation/export, allocation/validation,
upload and the first completed draw are reported separately from retained
redraws. A setup row is one observation, not a percentile estimate.

CPU render time is engine execution; GPU render time includes dispatch,
completion and a new owned RGBA readback/copy. Transport/request latency is
reported separately. Kernel timestamps are diagnostic. GPU counters must show
one upload and 106 dispatches, and retained samples must make zero engine
requests. CPU samples must hit their prepared scene and match fresh CPU bytes.
Both expanded and shared GPU jobs are released before the zoom study.

Zoom uses 150/225/300 DPI. Twelve new-target samples clear prepared and GPU
caches before each mode/target; recorded scene, Store and pipelines remain
warm. Eighteen revisits after three warmups use the same three cached targets
on CPU and GPU. Geometry is device-grid dependent: an arbitrary new DPI still
rebuilds stroke outlines, clip corrections and the packet. This study does
not demonstrate continuous zoom without preparation.

Browser frame time includes completed pixels, canvas upload and two animation
frame opportunities in a clipped 480px top/centre viewport. It is not scanout.
Native frame time is completed RGBA plus pipe transport; it does not include
window presentation and is not directly comparable with browser frame time.
Readback is retained in both paths to keep CPU/GPU conversion results comparable;
a future viewer could present GPU output without this round trip.

These are exploratory shared-desktop measurements, separate from the dedicated
CPU release performance gate. Raw samples, source/binary hashes and private
input identifiers stay in ignored `target/gpu-demo/` evidence.

## Hardware and provenance

Apple M3 Pro (12 CPU / 18 GPU cores, 18 GB unified memory), macOS 15.6.1,
Chrome 155 hardware WebGPU over Metal 3, and a separate native Metal executor
with safe math. Browser adapter initialization was 219.26 ms; native Metal
initialization was 227.10 ms. Both are excluded from the retained samples.
Rust 1.99.1 and Node 26.8.1 were used.

Both full runs used code checkpoint `168a9dfd19893ad4e0ddadb632342400ca5b3ab9`.
All audited source and binary hashes stayed unchanged during each run. Later
commits only document these measurements. The scalar run verifies parity; its
one-sample timings are not performance evidence.

## Static redraws

Milliseconds, **median / p95**. CPU includes prepared execution into a reused
output allocation; both GPU formats include fresh rasterization and completed
readback. Documents 1 and 3 retain CPU fallback for sampled paints at every DPI.

### Browser: SIMD WASM CPU versus WebGPU

| Document | DPI | Prepared CPU | Retained expanded GPU | Retained shared GPU |
|---|---:|---:|---:|---:|
| Document 1 | 72 | 7.82 / 8.19 | CPU fallback | CPU fallback |
| Document 1 | 150 | 14.80 / 18.70 | CPU fallback | CPU fallback |
| Document 1 | 300 | 30.43 / 31.13 | CPU fallback | CPU fallback |
| Document 2 | 72 | 0.82 / 2.10 | 2.27 / 5.07 | 2.10 / 4.48 |
| Document 2 | 150 | 2.06 / 2.17 | 4.16 / 8.08 | 4.21 / 8.49 |
| Document 2 | 300 | 5.77 / 6.05 | 12.59 / 19.70 | 12.36 / 18.62 |
| Document 3 | 72 | 8.43 / 9.09 | CPU fallback | CPU fallback |
| Document 3 | 150 | 18.04 / 18.41 | CPU fallback | CPU fallback |
| Document 3 | 300 | 38.43 / 39.57 | CPU fallback | CPU fallback |
| Document 4 | 72 | 10.95 / 11.50 | 6.35 / 9.01 | 6.71 / 9.77 |
| Document 4 | 150 | 19.59 / 19.99 | 7.84 / 11.08 | 9.20 / 13.23 |
| Document 4 | 300 | 39.14 / 39.88 | 15.72 / 27.85 | 20.01 / 35.46 |

### Native: CPU versus Metal

| Document | DPI | Prepared CPU | Retained expanded Metal | Retained shared Metal |
|---|---:|---:|---:|---:|
| Document 1 | 72 | 5.58 / 5.77 | CPU fallback | CPU fallback |
| Document 1 | 150 | 10.67 / 10.95 | CPU fallback | CPU fallback |
| Document 1 | 300 | 22.18 / 22.58 | CPU fallback | CPU fallback |
| Document 2 | 72 | 0.52 / 0.59 | 1.56 / 4.81 | 1.77 / 2.93 |
| Document 2 | 150 | 1.60 / 1.70 | 3.47 / 6.97 | 3.80 / 7.16 |
| Document 2 | 300 | 4.44 / 4.63 | 11.80 / 17.88 | 11.36 / 17.47 |
| Document 3 | 72 | 5.30 / 5.55 | CPU fallback | CPU fallback |
| Document 3 | 150 | 11.75 / 11.99 | CPU fallback | CPU fallback |
| Document 3 | 300 | 27.51 / 31.30 | CPU fallback | CPU fallback |
| Document 4 | 72 | 7.87 / 8.17 | 5.12 / 7.59 | 5.21 / 9.10 |
| Document 4 | 150 | 14.14 / 15.13 | 7.68 / 10.52 | 8.93 / 12.84 |
| Document 4 | 300 | 28.72 / 29.39 | 17.14 / 26.32 | 20.86 / 33.89 |

At 150 DPI on Document 4, shared WebGPU is **2.13x** faster than prepared
CPU execution, and shared Metal **1.58x** faster. With request/transport latency
included, medians are CPU/WebGPU **21.21 / 9.32 ms** and native CPU/Metal
**16.53 / 11.62 ms**. Document 2 does not benefit: CPU render medians are
**2.06 / 1.60 ms** in browser/native, against shared GPU **4.21 / 3.80 ms**.

Retaining geometry is the main redraw improvement. Sharing adds indirection
and scans a complete shared glyph source per placement, whereas expanded
geometry already has row slices. On the dense 150-DPI page, shared geometry
is 17% slower than retained expanded geometry in WebGPU, and 16% slower in
Metal. Its benefit is packet size and setup work, not a faster warm kernel.

## First target setup

One observation per mode after clearing CPU preparation. Scene, Store and
pipelines stay warm. Values include preparation/export, resource setup, upload,
the first completed draw and request transport. These single observations
should not be read as median or p95 estimates.

| Host | Document, 150 DPI | CPU first request ms | Expanded GPU first request ms | Shared GPU first request ms |
|---|---|---:|---:|---:|
| Browser | Document 2 | 5.51 | 15.16 | 11.21 |
| Browser | Document 4 | 52.12 | 86.93 | 59.28 |
| Native | Document 2 | 6.57 | 17.88 | 14.85 |
| Native | Document 4 | 43.97 | 92.32 | 52.11 |

For Document 4 at 150 DPI, the geometry changes are identical in native and
WASM exports:

| Format | Packet bytes | Unique edge records | Edge references | Shared sources | Instanced placements |
|---|---:|---:|---:|---:|---:|
| Expanded | 12,354,036 | 358,572 | 358,572 | 0 | 0 |
| Shared | 3,390,196 | 68,981 | 79,849 | 511 | 5,380 |

This is a **72.6% packet reduction** and 80.8% fewer unique edge records.
The same 13,320 exceptional CPU-prepared cells remain. Sharing does not help
every document: Document 2 grows from **573,396 to 613,212 bytes** at 150 DPI
(6.9%) because its reusable geometry does not offset the indirection metadata.

## Zoom: new targets versus retained revisits

Milliseconds, **median / p95 frame time**, aggregated equally across
150/225/300 DPI and top/centre viewports. New-target rows have 12 samples;
retained rows have 18 after three warmups. A p95 from this small zoom sample
is sensitive to individual scheduling delays. Browser rows include canvas
presentation opportunities; native rows include pipe transport only.

| Host | Document | Target state | Prepared CPU | Shared GPU |
|---|---|---|---:|---:|
| Browser | Document 1 | New target | 54.10 / 67.91 | CPU fallback |
| Browser | Document 1 | Retained revisit | 34.27 / 62.51 | CPU fallback |
| Browser | Document 2 | New target | 26.32 / 50.26 | 29.56 / 55.72 |
| Browser | Document 2 | Retained revisit | 25.27 / 29.72 | 23.24 / 46.09 |
| Browser | Document 3 | New target | 56.64 / 77.18 | CPU fallback |
| Browser | Document 3 | Retained revisit | 57.63 / 70.61 | CPU fallback |
| Browser | Document 4 | New target | 76.37 / 102.20 | 83.83 / 122.75 |
| Browser | Document 4 | Retained revisit | 57.25 / 62.35 | 25.14 / 62.18 |
| Native | Document 1 | New target | 22.17 / 35.00 | CPU fallback |
| Native | Document 1 | Retained revisit | 21.95 / 32.70 | CPU fallback |
| Native | Document 2 | New target | 9.51 / 17.20 | 19.05 / 36.37 |
| Native | Document 2 | Retained revisit | 8.55 / 16.53 | 13.95 / 27.76 |
| Native | Document 3 | New target | 25.20 / 41.70 | CPU fallback |
| Native | Document 3 | Retained revisit | 24.18 / 35.58 | CPU fallback |
| Native | Document 4 | New target | 55.96 / 70.77 | 67.15 / 91.46 |
| Native | Document 4 | Retained revisit | 27.23 / 38.81 | 19.65 / 32.18 |

On Document 4, retained browser revisits improve the median from **57.25 to
25.14 ms**, but p95 is essentially unchanged (**62.35 / 62.18 ms**). Native
revisits improve from **27.23 to 19.65 ms**, with p95 **38.81 / 32.18 ms**.
New-target GPU updates still cost more than CPU in both hosts. Document 2
has no consistent GPU responsiveness advantage; its browser revisit median
is slightly lower while its p95 is worse, and native CPU is faster.

The corresponding dense-page render-only zoom medians are new-target CPU/GPU
**64.27 / 71.09 ms** in the browser and **50.15 / 55.01 ms** natively; retained
revisit medians are **29.81 / 14.98 ms** and **21.76 / 14.11 ms**, respectively.
This distinguishes raster work from transport and presentation scheduling.

Three retained zoom resolutions reserve the following memory. CPU values are
the preparation charge metric, not total RAM; GPU values are nominal buffers.
The CPU output allocation is retained alongside them.

| Host | Document | CPU preparation charges MiB | CPU output capacity MiB | Shared GPU buffers MiB |
|---|---|---:|---:|---:|
| Browser | Document 2 | 85.76 | 33.18 | 122.72 |
| Browser | Document 4 | 95.74 | 33.20 | 132.11 |
| Native | Document 2 | 85.81 | 33.18 | 62.56 |
| Native | Document 4 | 97.31 | 33.20 | 71.93 |

Every static GPU job uploads once and executes 106 draws: one setup draw,
five warmups and 100 timed draws. Each warmed zoom job uploads once and
executes seven draws. Retained redraws make no geometry export/engine request.
The CPU hits its prepared cache on all retained samples.

## Accuracy

CPU retained pixels match fresh CPU exactly on all real cells and all
999 generated fixtures. CPU pixel hashes and both GPU packet hashes agree
across native, SIMD WASM and scalar WASM for all real cells. GPU pixels can
differ between native and browser arithmetic despite identical packets.

Below are changed pixels for the shared format against Exact CPU. All rows
have maximum channel error **1/255**, no pixels beyond one level, and no
observed repeat variation across 100 redraws. The maximum also stays at one
level for the tested zoom targets. These measurements are not a global f32
error bound or an independent proof of complete PDF accuracy.

| Document | DPI | Page pixels | WebGPU changed pixels | Metal changed pixels |
|---|---:|---:|---:|---:|
| Document 2 | 72 | 501,585 | 62 | 62 |
| Document 2 | 150 | 2,176,200 | 30 | 32 |
| Document 2 | 300 | 8,698,811 | 120 | 118 |
| Document 4 | 72 | 501,832 | 168 | 168 |
| Document 4 | 150 | 2,176,714 | 10 | 11 |
| Document 4 | 300 | 8,703,348 | 233 | 220 |

At 150 DPI the dense page changes 10 pixels in WebGPU and 11 in Metal,
out of 2,176,714. The existing retained expanded control changes seven.
The two sampled-paint documents show the Exact CPU fallback result.

## Validation and evidence

- 184 raster/paint tests passed; five existing slow tests remain ignored in
  that targeted run. All nine demo-host tests passed, including SVG export,
  prepared CPU byte identity, allocation reuse and invalidation/eviction.
- Strict raster/paint, all-features paint and demo-host Clippy, formatting,
  JavaScript syntax and Swift warnings-as-errors compilation passed.
- The shared sweep visits all 999 unpassworded render fixtures: 639 admissions,
  360 explicit fallbacks, 320 extra band/tile checks. Native agrees on all
  admission decisions and the 639 admitted CPU/packet hashes. Scalar WASM
  agrees with SIMD on all 999 CPU hashes and 639 packet hashes. Native and
  browser admitted errors stay within one channel level in this sweep.
- Independent clipped-polygon, true-quadratic and fractional-instance oracles
  pass. Six malformed shared packets, uninitialized/destroyed draws, eviction
  and document invalidation are checked. Large-origin stress still confirms
  the need for subtraction before f32 narrowing.
- The original exact integer span compositor regression passes all 999 fixture
  decisions, with 641 admissions and 1,916 admitted GPU variants. Its pixels
  remain byte-identical to CPU.
- Actual demo controls, shared GPU zoom, retained SVG source, aspect ratio and
  export availability pass. SVG performance was not rerun in this follow-up.
- The repository-wide `tools/check.py` passed policy, crypto vectors, fixture
  regeneration checks and bundled-data verification, then stopped because
  `uv` is not installed locally. Its full completion is not claimed.

The draft PR's earlier Linux merge-result CI also exposes an integration
limitation: the moving `render/one-pass` base changed internal geometry types
after this experiment's pinned base. The optional adapter compiles and passes
its checks on the measured branch; porting it to that newer preparation layout
is separate from the measurements above. This remains a draft, not a merge or
release-gate claim.

Ignored evidence:

- `target/gpu-demo/retained-browser.json`, `retained-native.json`: complete raw
  timings, setup, geometry, counters, differential errors and source hashes.
- `retained-scalar-real.json`: real-input CPU/packet parity, not performance.
- `retained-fixtures.json`, `retained-native-fixtures.json`,
  `retained-scalar-fixtures.json`: fixture decisions and parity.
- `retained-exact-regression.json`, `retained-ui.json`, `retained-ui.png`:
  exact-span regression and demo validation.
- `retained-audit.py`: verifies sample counts, cache hits, uploads/dispatches,
  cross-platform real-input hashes, repeatability and zoom accuracy.

## Decision this experiment supports

Retained GPU rasterization is worth continuing for repeated dense vector/text
workloads: it now beats equivalent CPU preparation reuse. Keep shared geometry
as a bounded representation option because it substantially reduces dense
packet/export costs, while recognizing that expanded retained geometry is the
faster warm kernel in this implementation. Sparse pages and the currently
unsupported sampled paints still favour CPU.

The next useful experiments are GPU-resident presentation, reducing shared
source scan/indirection work, and separating resolution-independent geometry
from grid-dependent stroke/clip preparation so a new zoom target requires less
CPU work. None of those gains are assumed in this report. SVG export remains
a separate valuable feature; this result does not depend on SVG rendering.
