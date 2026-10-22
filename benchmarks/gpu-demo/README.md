# Rendering backend experiment

A WebGPU host for the existing prepared scene, branched from
`render/one-pass` at `87fa81b64162281fccbdcae5564e1ec431022c4c`.
The CPU's exact coverage feeds an integer N16 GPU compositor. There is no
general GPU rasterizer here. This experiment does not change the production
renderer or the scope of ADR 0004.

[`RESULTS.md`](RESULTS.md) records the measured CPU/GPU comparison and its
limits; raw evidence remains local under `target/gpu-demo/`.

```sh
cargo build --manifest-path benchmarks/gpu-demo/engine/Cargo.toml --locked \
  --release --target wasm32-wasip1 --target-dir target/gpu-demo
node benchmarks/gpu-demo/run.mjs serve
```

Open `http://127.0.0.1:8097/` in a WebGPU-capable Chrome. Choose a generated
workload or open a PDF, then compare both previews at 72, 150 or 300 DPI.
PDFs stay in the browser. The server binds only to loopback and exposes
the demo, its WASM module and generated development fixtures. No packages
need installing. The existing bundled font notices accompany this host in
[`data/README.md`](../../data/README.md) and its linked license files.

## Architecture

`pagura-paint`'s optional `gpu-demo` feature adds two methods on
`PreparedScene`: `gpu_fallback_reason` and `gpu_program`. Default product
builds do not include them. The latter is an additional bounded preparation
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

The small development host is a separate, dependency-free Cargo workspace
with only local engine dependencies. Native and WASI builds use the same
code. The browser reuses the viewer's WASI shim, bounded mailbox and extracted
`web/host_runtime.mjs`. WebGPU and shader code stay in the host.

## Measurement

[`policy.json`](policy.json) defines exploratory reporting, separately from
the release gate. Both browser paths use the same SIMD WASI engine, cached
scene and warm resources. Every measured result is checked byte-for-byte
outside timing. Adjacent paired samples alternate CPU/GPU/GPU/CPU.

The main comparison includes CPU preparation and coverage, packet
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
true glyph quadratics and joint clipping. Keeping the result GPU-resident
would suit a viewer, while conversion still needs readback. Prepared-program
reuse needs a measured viewer workload before adding a cache, as ADR 0034
requires. Sampled paints and F32 groups need separate validated kernels;
F64-dependent operations retain their CPU execution.

## SVG presentation experiment

The same host also accepts operation 4, a whole-page SVG export of the public
`pagura_interpret::Scene`. The adapter lives in `engine/src/svg.rs` and its
image resource module, outside product crates. It never consumes CPU coverage
spans and adds no third-party dependency or GPU API to the Rust engine.

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
Zoom resizes that vector image; CPU and hybrid paths rebuild their pixel
buffers. There is no selectable browser text in the outlined SVG presentation.

An unimplemented shading, tiling paint, soft-mask group, transfer function,
non-RGB group, non-isolated blend group, unsupported knockout, print overprint,
fixed-view annotation, image matte/embedded alpha or resampled image mask
refuses the whole SVG export with a reason. The demo labels it unavailable;
it never displays a partial SVG page or silently calls a CPU image SVG.
Export bytes, tags, traversal/function work and image bytes are bounded.

The comparison distinguishes three costs:

- **Completed RGBA:** export, validation/parse, decode, browser rasterization
  and Canvas readback. Both other paths already produce completed RGBA.
- **Fresh viewport display:** rebuild backend output from a warm recorded
  scene, then show a top or centre 480-pixel viewport. This does not force
  full-page SVG readback. Timings end after two animation-frame opportunities;
  they are not display scanout or GPU-completion timestamps.
- **Zoom:** reuse the SVG vector source at 1×, 1.5× and 2× while CPU/hybrid
  produce fresh pixels. A separate CSS gesture scales existing content for
  every backend; bitmap preview scaling is not fresh-resolution rendering.

Slow SVG raster cases remain in the report, with an explicit sample cap from
`policy.json`. SVG differences against CPU include changed pixels, pixels
whose largest channel difference exceeds one, maximum error, mean error and
RMSE. These are differential measurements, not independent accuracy oracles
or a pass of ADR 0004. CPU/hybrid byte equality remains required.

```sh
node benchmarks/gpu-demo/run.mjs svg-test
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
