# Rendering backend experiment

A comparison host branched from `render/one-pass` at
`87fa81b64162281fccbdcae5564e1ec431022c4c`, merged with
`f86eb5da130286b5c9f270e367a7e4daafa68a89` for the shared-buffer study.
It presents exact CPU output,
approximate GPU area coverage through WebGPU and native Metal, an exact integer
N16 GPU compositor using CPU coverage, and browser SVG exported from the shared
scene. The demo retains shared GPU geometry and compares it with CPU
prepared-scene reuse. A separate direct-display panel tests resolution-specific
and fixed world geometry with WebGPU canvas and native Metal presentation. GPU coverage currently admits opaque solid RGB pages.
This experiment does not change the production renderer or the scope of
ADR 0004.

## Native shared-buffer application

The Swift application owns Metal and a safe-Rust engine child directly. Two
private 64 MiB files carry frames and packets; the pipe carries bounded metadata
only. CPU prepared rendering streams the current base's `render_bands` into
the frame file. A page-aligned shared mapping is wrapped by a Metal
`bytesNoCopy` buffer, with its mapping lifetime owned by that buffer. Each draw
completes before another engine request can overwrite shared bytes. Rust uses
only safe standard-library file writes. There is no Rust FFI, mapping library,
new product dependency, or Node frame relay. Kernel file-write copies remain.

```sh
cargo build --manifest-path benchmarks/gpu-demo/engine/Cargo.toml \
  --release --target-dir target/gpu-demo
swiftc -D SHARED_CLIENT -O -warnings-as-errors \
  benchmarks/gpu-demo/native/Host.swift \
  benchmarks/gpu-demo/native/Presentation.swift \
  benchmarks/gpu-demo/native/World.swift \
  benchmarks/gpu-demo/native/Shared.swift \
  benchmarks/gpu-demo/native/Integrated.swift -o target/gpu-demo/shared-host
node benchmarks/gpu-demo/native/integrated.mjs \
  --samples 100 --warmups 5 --cold-samples 20 \
  --pdf /absolute/input.pdf --output target/gpu-demo/integrated-native.json
node benchmarks/gpu-demo/run.mjs integrated \
  --samples 100 --warmups 5 --cold-samples 20 \
  --pdf /absolute/input.pdf --output target/gpu-demo/integrated-browser.json
```

Repeat `--pdf` in the same order on both runners. Opening starts a new engine
process or browser worker, opens already-read bytes and waits for first display
at 150 DPI. Application/adapter/presentation initialization and file I/O are
reported separately. These are document-cold trials with a warm application.
Browser frame opportunities and native drawable callbacks are different clocks;
neither is display scanout.

Warm redraws render fresh full-page pixels at 72/150/300 DPI. Twelve new zoom
targets include all preparation and transfer. Scrolling lets **every** mode
retain its full-page bitmap and changes only the viewport, with zero engine
requests, coverage dispatches or page-content uploads. Diagnostic readbacks and
top/middle/bottom attachment captures run afterward. The browser demo includes
an opening/scrolling panel and live retained scroll control for page 1.

`--world-reciprocal` is a separate arithmetic experiment on both runners: one
clip reciprocal per mark/pixel replaces three divisions per edge. The original
shader remains the default control. The new option changes no admission rules
or CPU view preparation. Run `world-test --world-reciprocal` to check it against
the same fixtures and independent polygon oracle.

Preparation now follows the base's 512 MiB limit. The prepared LRU remains
256 MiB charged bytes. The experimental WASM worker has a 1 GiB aggregate
memory cap for recorded/prepared/exported data and allocator high-water use;
the production browser host keeps its 512 MiB default. Mappings/reservations
are not measurements of resident memory. See `policy.json` for all scopes and
`SHARED_HOST_RESULTS.md` for results.

[`DIRECT_GPU_RESULTS.md`](DIRECT_GPU_RESULTS.md) records the latest direct-display
and world-geometry experiment. [`RETAINED_GPU_RESULTS.md`](RETAINED_GPU_RESULTS.md) records the shared-geometry,
resource-retention and equivalent CPU reuse comparison, including new and
revisited zoom targets. [`SVG_CLEANUP_RESULTS.md`](SVG_CLEANUP_RESULTS.md) records
the retained SVG export cleanup. [`SVG_RESULTS.md`](SVG_RESULTS.md) preserves
the first three-backend experiment, and [`RESULTS.md`](RESULTS.md) preserves
the original CPU/GPU measurements. Raw evidence stays under ignored
`target/gpu-demo/`.

```sh
cargo build --manifest-path benchmarks/gpu-demo/engine/Cargo.toml --locked \
  --release --target wasm32-wasip1 --target-dir target/gpu-demo
node benchmarks/gpu-demo/run.mjs serve
```

Open `http://127.0.0.1:8097/` in a WebGPU-capable Chrome. Choose a generated
workload or open a PDF, then compare the previews at 72, 150 or 300 DPI.
PDFs stay in the browser. The server binds only to loopback and exposes
the demo, its WASM module and generated development fixtures. No packages
need installing. The existing bundled font notices accompany this host in
[`data/README.md`](../../data/README.md) and its linked license files.

## Exact-span architecture

`pagura-paint`'s optional `gpu-demo` feature adds export methods on
`PreparedScene`. Default product builds do not include them. The original
`gpu_program` exact-span export is an additional bounded preparation
step: it uses the same active-item schedule, prepared shapes, joint clips,
glyph-run batching and exact coverage bytes as CPU execution. It preserves
the existing N16 source encodings and source order.

Only RGB N16 page buffers with opaque solid Normal marks are admitted. This
covers fills, strokes, clipped paths and outline text. Sampled paints,
dynamic opacity, groups, masks, transfers, overprint, F32 and F64 keep the
CPU renderer, with a visible reason. No GPU precision substitutes for F64.
Packet allocation and bin references have additional byte and work meters;
the CPU preparation's charges are unchanged.

The host validates the packet before allocation, checks device limits, then
dispatches one GPU invocation per pixel. A row is binned in 32-pixel runs,
with source-ordered span references. Integer composition is exactly
`(c*s + (255-c)*d + 127) / 255`; all values fit in u32. The last fully opaque
Normal store safely discards preceding stores at its pixel (ADR 0034 §4),
then remaining stores execute forward. Final conversion is
`(word + 128) / 257`. No GPU floating-point math or normalized texture
conversion enters these pixels.

The small development host is a separate Cargo workspace
with only local engine dependencies. Native and WASI builds use the same
code. The browser reuses the viewer's WASI shim, bounded mailbox and extracted
`web/host_runtime.mjs`. WebGPU and shader code stay in the host.

## GPU coverage experiment

Measured performance, accuracy, native FP64 capability and limitations are in
[GPU_COVERAGE_RESULTS.md](GPU_COVERAGE_RESULTS.md).

The GPU stage selector also offers **local f32 coverage** and **absolute f32
coverage**. These are approximate experiments, distinct from the exact N16
span compositor. SVG export, its one-page cache, reusable resources and the
Export SVG control remain independent of either GPU path.

A second optional method, `PreparedScene::gpu_raster_program`, borrows the
existing prepared shapes through the `pagura-raster/gpu-demo` geometry seam.
It exports straight edges and true monotone quadratic glyph boundaries;
it never calls the band rasterizer or generates ordinary coverage spans.
The GPU integrates pixel areas, quantizes coverage once, then composites
source-ordered N16 colours. Existing preparation still outlines strokes,
flattens general cubics at its existing tolerance, certifies winding and
resolves exceptional winding/joint-clip cells. Those sparse exact corrections
and non-rectangular clip runs are explicit packet inputs and reported counts.
This experiment does not move all of preparation to the GPU.

Local mode subtracts a shape's integer origin in f64 **before** conversion to
f32. Absolute mode narrows device coordinates directly. Both use the same
compensated GPU sum and true-curve integrals; neither is `Accuracy::Exact`.
Large compound shapes have a bounded row index. Tile references preserve
source order; export bytes and conservative pixel-edge evaluations have
separate limits. The existing solid opaque RGB N16 admission remains.
Unsupported pages show a CPU result with an explicit reason.

`native/Host.swift` is an isolated Metal host using system frameworks, with
an independent shader counterpart and the same wire format. It probes
`double` compilation, uses safe math, waits for command completion and copies
the RGBA output before reporting time. No FFI, unsafe Rust or external GPU
dependency enters the engine. On a platform without Metal this optional host
is unavailable; the portable Rust and browser paths are unchanged.

```sh
node benchmarks/gpu-demo/run.mjs coverage-test --output target/gpu-demo/coverage-fixtures.json
node benchmarks/gpu-demo/run.mjs coverage-bench --samples 100 --warmups 5 \
  --pdf /absolute/path.pdf --output target/gpu-demo/coverage-browser.json
swiftc -O benchmarks/gpu-demo/native/Host.swift \
  benchmarks/gpu-demo/native/Presentation.swift benchmarks/gpu-demo/native/World.swift \
  -o target/gpu-demo/metal-host
node benchmarks/gpu-demo/native/bench.mjs --samples 100 --warmups 5 \
  --pdf /absolute/path.pdf --output target/gpu-demo/coverage-native.json
node benchmarks/gpu-demo/native/bench.mjs --fixtures \
  --browser-report target/gpu-demo/coverage-fixtures.json
```

Coverage checks include independent clipped-polygon and numerical true-curve
oracles, large-coordinate precision stress, malformed packets, and fixture
comparisons against CPU. Error distributions are evidence about this sample,
not a global error bound or a replacement for conformance. Native/WASM packet
parity is checked separately from GPU arithmetic differences.

## Earlier fresh-export measurement

[`policy.json`](policy.json) defines exploratory reporting, separately from
the release gate. The earlier `bench` runner uses the same SIMD WASI engine, cached scene and
warm resources. CPU and integer-span GPU results are checked byte-for-byte
outside timing; approximate coverage and SVG report differential error. Samples
rotate and reverse the CPU, GPU and SVG order. The retained experiment below
has its own CPU reuse control and separate setup/redraw/zoom measurements.

The earlier fresh-export comparison includes CPU preparation and coverage, packet
construction, program upload on every render, GPU completion and RGBA
readback. It reports full worker-transport latency separately. GPU timestamp
queries are diagnostic kernel timings, never the end-to-end result. PDF
opening, WASM compilation, first adapter/pipeline initialization and initial
GPU buffer allocation are outside warm timings; first-render and GPU setup
costs are reported separately. No persistent prepared-GPU-scene cache is
assumed. Native CPU timings can provide additional context, measured in a
separate run; they must not be substituted for the browser CPU column.

```sh
node benchmarks/gpu-demo/run.mjs bench --samples 100 --warmups 5 \
  --output target/gpu-demo/browser-bench.json
# Repeat --pdf /absolute/path.pdf to add local input documents (page 0).

cargo build --manifest-path benchmarks/gpu-demo/engine/Cargo.toml --locked \
  --release --target-dir target/gpu-demo
node benchmarks/gpu-demo/native.mjs bench --samples 100 --warmups 5
```

The runner uses an isolated temporary Chrome profile and records the actual
hardware adapter. It rejects software GPU fallbacks. `--chrome PATH` selects
a browser binary. A developer-only flag exposes accurate GPU timestamp
queries and adapter backend information; the interactive demo needs no flag.
See [WebGPU developer features](https://developer.chrome.com/docs/web-platform/webgpu/developer-features)
and the [WGSL specification](https://www.w3.org/TR/WGSL/). Shared-desktop
timings are exploratory, with no inference to the dedicated CPU release gate.

## Verification

```sh
cargo test -p pagura-paint --features gpu-demo --lib --locked
cargo clippy -p pagura-paint --features gpu-demo --all-targets --locked -- -D warnings
node benchmarks/gpu-demo/run.mjs test
node benchmarks/gpu-demo/native.mjs parity

RUSTFLAGS='-C target-feature=-simd128' cargo build \
  --manifest-path benchmarks/gpu-demo/engine/Cargo.toml --locked --release \
  --target wasm32-wasip1 --target-dir target/gpu-demo-scalar
node benchmarks/gpu-demo/run.mjs test \
  --wasm target/gpu-demo-scalar/wasm32-wasip1/release/pagura-gpu-demo.wasm \
  --output target/gpu-demo/scalar-hardware-tests.json
```

The hardware sweep visits every generated, unpassworded render fixture.
Admitted cases compare GPU and CPU bytes, band heights and an offset region;
fallbacks retain their reasons. An independent integer shader oracle checks
all N16 output words and all 256 coverage values; malformed packet tests
run before dispatch. Native parity compares the CPU output and admitted
GPU packets with the browser's SHA-256 evidence. Rust tests also cover
transparent backgrounds, clipping, allocation/work refusal and F64
admission. JSON, screenshots, binaries and logs stay under ignored `target/`.

## Next work this experiment can inform

Packet construction and readback are real costs. Moving exact coverage or
source evaluation to the GPU needs its own accuracy proof, especially for
true glyph quadratics and joint clipping. Keeping rendered pixels GPU-resident would suit a viewer, while conversion
still needs readback. This experiment now measures bounded geometry/program
reuse alongside CPU preparation reuse; it does not introduce a production
viewer cache. Sampled paints and F32 groups need separate validated kernels;
F64-dependent operations retain their CPU execution.

## SVG presentation experiment

The same host also accepts operation 4, a whole-page SVG export of the public
`pagura_interpret::Scene`. The adapter lives in `engine/src/svg.rs` and its
image resource module, outside product crates. It never consumes CPU coverage
spans and adds no third-party dependency or GPU API to the Rust engine.

The exporter produces a resolution-independent owned artifact from `Scene`
and `Document`. It is separate from the native/WASI transport and browser
presentation. The host retains one artifact (at most 64 MiB) for the current
recorded page, invalidating it on page or document changes. Image encodings
are deduplicated inside that artifact and reused with it. There is no separate
cross-page image cache or cache keyed by scene-local resource IDs.

Paths and cubic curves remain vectors. Glyph outlines are shared definitions
with ordered placements, so browser font matching and text shaping do not
change their geometry. Clips intersect through SVG clip paths; pen transforms,
dashes, ordinary opacity, RGB groups and representable blends are emitted as
SVG operations. Image source samples use the existing decoders, colour
converters and eight-bit RGBA PNG encoder. They become reusable image resources, not a raster
substitute for the page. Native SVG antialiasing, stroke placement and image
sampling and PDF zero-area fill behaviour can differ from Pagura's exact renderer.
Zero-length strokes that cannot be represented by this adapter are refused.

The browser validates the generated XML's element/attribute vocabulary and
resources. SVG is displayed as an image with a retained vector source, keeping
its definitions private and avoiding a large SVG tree in the live HTML DOM.
Each comparison retains one parsed SVG and decoded image for its samples;
zoom resizes that vector image. CPU and hybrid paths rebuild their pixel
buffers. The **Export SVG** button saves the self-contained vector document
with its embedded image resources. There is no selectable browser text in the outlined SVG presentation.

An unimplemented shading, tiling paint, soft-mask group, transfer function,
non-RGB group, non-isolated blend group, unsupported knockout, print overprint,
fixed-view annotation, image matte/embedded alpha or resampled image mask
refuses the whole SVG export with a reason. The demo labels it unavailable;
it never displays a partial SVG page or silently calls a CPU image SVG.
Export bytes, tags, traversal/function work and image bytes are bounded.

The adapter removes a fill/glyph clip application only when its complete
rectangular clip chain strictly contains a conservative control-hull bound.
The proof includes affine transforms, a generous margin in natural SVG units
and finite-precision padding. Empty/uncertain chains, extreme transforms,
near-edge marks, strokes, images and group clips remain unchanged. Glyph
placements and their opacity stay in source order; no clip groups are merged.
`engine/src/svg/clips.rs` contains the bounded proof independently of XML output.

The comparison distinguishes three costs:

- **Completed RGBA:** fresh Canvas rasterization and readback from the retained
  SVG/image artifact. Initial export, transport, validation/parse, decode and
  first rasterization are reported separately. CPU/hybrid still build fresh
  pixels; neither path has an application raster cache.
- **Viewport display:** rebuild CPU/hybrid pixels or reuse the compiled SVG,
  then show a top or centre 480-pixel viewport. This does not force full-page
  SVG readback. Timings end after two animation-frame opportunities;
  they are not display scanout or GPU-completion timestamps.
- **Zoom:** reuse the SVG vector source at 1×, 1.5× and 2× while CPU/hybrid
  produce fresh pixels. A separate CSS gesture scales existing content for
  every backend; bitmap preview scaling is not fresh-resolution rendering.

Zoom differences read back the retained SVG image actually shown by the
browser. A fresh serialization at the same DPI is also compared separately;
changing an SVG image's intrinsic dimensions can change browser rasterization.

Slow SVG raster cases remain in the report, with an explicit sample cap from
`policy.json`. SVG differences against CPU include changed pixels, pixels
whose largest channel difference exceeds one, maximum error, mean error and
RMSE. These are differential measurements, not independent accuracy oracles
or a pass of ADR 0004. CPU/hybrid byte equality remains required. SVG export bytes must repeat exactly;
browser pixel variation across identical SVG inputs is reported separately.

```sh
node benchmarks/gpu-demo/run.mjs svg-test --output target/gpu-demo/svg-tests.json
node benchmarks/gpu-demo/run.mjs ui-test
node benchmarks/gpu-demo/native.mjs svg-parity
node benchmarks/gpu-demo/run.mjs bench --samples 100 --warmups 5 \
  --output target/gpu-demo/three-backends.json --pdf /absolute/path/to/input.pdf
```

Repeat `--pdf` for additional local documents; their labels are generic.
The same arguments with `serve --port 8097` make them available in the local
workload selector. PNG comparisons, amplified differences and SVG files are
saved beside a benchmark report under its `-visuals` directory. No real PDF
or generated real-document output is committed. `--real-only` limits a run to
explicit inputs; `--skip-zoom` skips the presentation/zoom studies.

## Shared geometry and retained resources

The default demo option retains a version 3 packet: certified glyph placements
reference one translation-free source; large path rows reference a single edge
pool. Fractional placement offsets remain separate from integer device origins.
No pointer/address enters the wire format. The version 2 expanded packet remains
available as a retained benchmark control.

The demo host adds a three-entry prepared-scene LRU for CPU and GPU setup.
Operation 7 renders the existing immutable `PreparedScene` into a reused output
Vec; operation 8 exports shared geometry from it, 10 exports expanded geometry,
and 9 clears prepared entries. Open/page changes invalidate entries. Existing
operations 2 through 6 keep their fresh-render behavior. No product renderer
policy or third-party dependency changes.

GPU setup validates, allocates and uploads once; every warm redraw dispatches
coverage again and completes a fresh RGBA readback/copy. No pixel bitmap cache
is used. A three-entry GPU LRU explicitly destroys evicted resources. WebGPU
combines pixel and timestamp readback; Metal uses immutable handle-owned buffers
and reports actual command completion. Cold setup, upload/dispatch counts, byte
reservations and evictions are recorded independently from warm timings.

Prepared geometry is tied to its DPI and region: clip corrections and stroke
outlines use that device grid. A new resolution rebuilds those values. Zoom
studies distinguish new targets from revisiting the same three retained targets,
giving CPU the same prepared-scene reuse. The demo's preview panels share CPU
preparation; the dedicated zoom study clears caches separately for each cold
mode to measure its complete cost.

```sh
node benchmarks/gpu-demo/run.mjs retained-test --output target/gpu-demo/retained-fixtures.json
node benchmarks/gpu-demo/run.mjs retained-bench --samples 100 --warmups 5 \
  --pdf /absolute/path.pdf --output target/gpu-demo/retained-browser.json
swiftc -warnings-as-errors -O benchmarks/gpu-demo/native/Host.swift -o target/gpu-demo/metal-host
node benchmarks/gpu-demo/native/retained.mjs --samples 100 --warmups 5 \
  --pdf /absolute/path.pdf --output target/gpu-demo/retained-native.json
node benchmarks/gpu-demo/native/retained.mjs --fixtures \
  --browser-report target/gpu-demo/retained-fixtures.json
```

CPU cache eviction uses at most 256 MiB of the existing preparation reservation
metric, with the bounded Store and one output Vec reported separately; this is
not total physical memory. GPU cache limits cover nominal buffer sizes, including
readback, with at most three resolutions. The retained expanded control and
shared path each hold one job during static comparisons and release both before
zoom. Native independently enforces three handles and a 256 MiB buffer cap.


## Direct display and fixed world geometry

The additional panel compares CPU prepared-scene reuse, retained GPU with
readback as a control, retained GPU with direct display, and fixed world
geometry with direct display. Every mode rasterizes fresh whole-page pixels,
then displays the same 480×480 top or centre viewport. Direct GPU frames use
neither RGBA readback nor timestamp-query mapping. Explicit diagnostic
readbacks and attachment captures occur outside timed samples. CPU and
readback controls upload a fresh cropped viewport, rather than a full-page
texture, to avoid charging unnecessary display traffic.

`present.mjs`/`present.wgsl` draw the existing compute output directly into a
WebGPU canvas. The native host uses a real `CAMetalLayer` in an AppKit window,
not an offscreen timing substitute. An alternating eight-pixel footer outside
the measured PDF viewport marks distinct native frames. Native samples keep
presentation/discard counts and presented-only drawable timings. No discarded
sample is silently retried or removed. Browser measurements end at two
animation-frame opportunities, while native measurements include completion,
presentation-or-discard callbacks and pipe transport. These clocks are not
interchangeable, and neither measures photons at the display.

`PreparedScene::gpu_world_program` exports a single 300-DPI vectorization for
views at integer DPIs from 72 through 300. Source-local edge indices and fixed
canonical tile bins are conservative over that interval. Geometry is uploaded
once per document/page; one maximum-size output allocation is overwritten on
every draw. At most three small view buffers remain resident. Per-view data
contains placement transforms and sparse exceptional coverage cells, never
edges or ordinary coverage rows. Native Metal independently validates the
wire format and uses the same ownership model. Buffer budgets count nominal
allocations, not process RSS or total driver memory.

**New resolutions still run existing CPU preparation** to derive those
corrections. Revisited views need no engine request, edge upload or pixel
cache. Removing the view corrections causes visible errors; keeping them
preserves the existing CPU certification boundary. Fixed vectorization is
approximate and must not be called Exact. Hairlines, stroke adjustment,
fixed-size annotations, zero-area device fills/clips/shading bounds and active
nonrectangular clips explicitly refuse world admission. Other GPU paths and
the CPU remain available. SVG export stays independent and reusable.

The formal comparison uses 100 samples after five warmups per static DPI.
Zoom measures twelve genuinely new DPIs (151–299), clearing prepared CPU state
separately before each mode/target, followed by eighteen revisits to three
retained views after three warmups. Store/recorded-scene/pipeline state stays
warm. Geometry and view upload counters, readbacks, accuracy and cold costs
are reported separately. `DIRECT_GPU_RESULTS.md` records outcomes and limits.

```sh
node benchmarks/gpu-demo/run.mjs world-test
node benchmarks/gpu-demo/run.mjs world-ui-test
node benchmarks/gpu-demo/native/world-checks.mjs \
  --browser-report target/gpu-demo/display-fixtures.json
node benchmarks/gpu-demo/run.mjs world-bench --samples 100 --warmups 5 \
  --pdf /absolute/input.pdf --output target/gpu-demo/display-browser.json
node benchmarks/gpu-demo/native/display.mjs --samples 100 --warmups 5 \
  --pdf /absolute/input.pdf --output target/gpu-demo/display-native.json
```
