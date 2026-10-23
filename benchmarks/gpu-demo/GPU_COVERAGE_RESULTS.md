# GPU area integration experiment

SVG export remains available, with its independent bounded artifact cache,
image reuse, clip proof and download control. This experiment adds actual
GPU coverage integration alongside the exact CPU-coverage GPU compositor.

## Architecture and precision

The optional `gpu-demo` feature exposes a read-only view of prepared shape
geometry. Preparation stays with the existing renderer: device transforms,
stroke outlining, general-cubic flattening, true quadratic glyph extraction,
winding certification, exceptional cell resolution and clip classification.
The new producer never invokes `BandRasterizer` and emits no ordinary
coverage spans. A GPU integrates straight edges and true monotone quadratics
against each pixel, quantizes coverage and composites the same N16 colours.

Source-ordered 32 by 8 pixel tile references and a four-row index for large
compound paths bound traversal. Exact CPU-prepared winding/joint-clip cells
and non-rectangular clip runs are explicit inputs. Counts are reported; this
is not a claim that all preparation or all coverage moved to the GPU.

Two modes isolate coordinate precision:

- **Absolute f32:** narrow prepared device coordinates directly.
- **Local f32:** subtract each shape/row part's integer origin in f64 before
  narrowing. GPU area integration and compensated accumulation use f32.

Neither mode is `Accuracy::Exact`. CPU geometry and compositing policies
remain unchanged. The experiment retains the opaque solid RGB N16 admission
contract. Sampled images/paints, transparency, groups, transfer/overprint and
required higher-precision buffers keep explicit CPU fallbacks. These are
prototype feature boundaries, not proof that those operations require a CPU.

The native executor lives under `benchmarks/gpu-demo/native/`, uses Swift
and the system Metal framework, and consumes the same packet. No GPU API,
FFI, unsafe Rust or new third-party dependency enters the product engine.
Its shader counterpart uses the same area formulas; both executors are
checked against the independent oracles and CPU reference.

## Native f64

On the tested Apple M3 Pro, runtime compilation of a kernel with a `double`
storage buffer fails with **'double' is not supported in Metal**. No native
FP64 rendering timings are claimed. Apple's [Metal Shading Language 4.1
specification](https://developer.apple.com/metal/Metal-Shading-Language-Specification.pdf),
section 2.1, excludes `double`. This is a hardware/API boundary on this host;
changing from WGSL to a native Metal shader does not enable FP64.

[Vulkan exposes optional `shaderFloat64`](https://docs.vulkan.org/spec/latest/chapters/features.html#features-shaderFloat64).
A native Vulkan implementation on suitable hardware could use real shader
f64; its availability and throughput need an actual adapter test. That
hardware was not available for this experiment. Compensated f32 accumulation
here is not software f64 or a global exactness guarantee.

## Method

Same four local PDF inputs, first page, opaque white, 72/150/300 DPI, Apple
M3 Pro / macOS 15.6.1 / Chrome 155. No private PDF names or contents are
committed. The evidence records SHA-256 input identities, source/binary hashes,
raw samples, median and p95. Browser and native performance runs are sequential.

Each path has five warmups and 100 measured samples, rotating and reversing
order. The browser compares CPU, exact CPU-coverage GPU spans, local GPU
coverage and absolute GPU coverage. Native compares native CPU with native
Metal local/absolute coverage. Timings include fresh CPU preparation and
geometry export, upload, GPU completion and RGBA readback/copy. Worker/native
transport is reported separately. Pipeline initialization, first allocation
and browser packet validation are separate. Native buffer allocation is included
in each measured render; initial allocation is absorbed by warmup and its cold
cost is not separately retained. GPU timestamps are diagnostics and are
not substituted for complete-render time. No persistent prepared-scene or
GPU geometry cache is silently assumed.

SVG was not re-benchmarked: its code is unchanged and the previous retained
SVG results remain in [SVG_CLEANUP_RESULTS.md](SVG_CLEANUP_RESULTS.md). The
SVG pane and export control were exercised in the updated interactive demo.

## Results

Full-render time in milliseconds, **median / p95**. Browser CPU is SIMD WASM;
native CPU is a separate native build and must be compared with the native
Metal column. Fallback cells have no GPU timing.

### Browser

| Input | DPI | CPU | Exact CPU-coverage GPU | Local GPU coverage | Absolute GPU coverage |
| --- | ---: | ---: | ---: | ---: | ---: |
| Document 1 | 72 | 7.873 / 11.195 | CPU fallback | CPU fallback | CPU fallback |
| Document 1 | 150 | 14.478 / 15.475 | CPU fallback | CPU fallback | CPU fallback |
| Document 1 | 300 | 30.090 / 31.500 | CPU fallback | CPU fallback | CPU fallback |
| Document 2 | 72 | 1.630 / 1.790 | 3.023 / 6.450 | 3.913 / 7.460 | 3.893 / 6.535 |
| Document 2 | 150 | 2.988 / 3.165 | 5.692 / 6.970 | 6.488 / 10.390 | 6.842 / 10.970 |
| Document 2 | 300 | 7.072 / 7.570 | 13.660 / 14.730 | 15.925 / 22.650 | 16.230 / 23.460 |
| Document 3 | 72 | 9.670 / 10.315 | CPU fallback | CPU fallback | CPU fallback |
| Document 3 | 150 | 18.927 / 20.885 | CPU fallback | CPU fallback | CPU fallback |
| Document 3 | 300 | 38.822 / 41.625 | CPU fallback | CPU fallback | CPU fallback |
| Document 4 | 72 | 31.830 / 32.515 | 35.203 / 36.200 | 46.910 / 52.005 | 46.987 / 51.250 |
| Document 4 | 150 | 46.252 / 49.340 | 54.223 / 57.600 | 65.390 / 71.145 | 64.573 / 70.340 |
| Document 4 | 300 | 76.720 / 83.675 | 102.848 / 115.055 | 102.155 / 111.265 | 102.653 / 112.515 |

### Native Metal

| Input | DPI | Native CPU | Local GPU coverage | Absolute GPU coverage |
| --- | ---: | ---: | ---: | ---: |
| Document 1 | 72 | 5.850 / 6.126 | CPU fallback | CPU fallback |
| Document 1 | 150 | 10.801 / 11.150 | CPU fallback | CPU fallback |
| Document 1 | 300 | 23.733 / 24.034 | CPU fallback | CPU fallback |
| Document 2 | 72 | 1.137 / 1.500 | 2.980 / 4.178 | 2.650 / 3.799 |
| Document 2 | 150 | 2.360 / 2.594 | 5.467 / 7.335 | 5.250 / 7.175 |
| Document 2 | 300 | 6.894 / 7.862 | 15.883 / 22.163 | 15.671 / 22.797 |
| Document 3 | 72 | 6.454 / 6.619 | CPU fallback | CPU fallback |
| Document 3 | 150 | 12.867 / 13.525 | CPU fallback | CPU fallback |
| Document 3 | 300 | 27.904 / 28.461 | CPU fallback | CPU fallback |
| Document 4 | 72 | 25.663 / 28.114 | 47.215 / 51.805 | 47.690 / 52.022 |
| Document 4 | 150 | 38.069 / 40.839 | 62.764 / 71.170 | 62.787 / 68.810 |
| Document 4 | 300 | 62.005 / 77.690 | 99.704 / 125.845 | 103.690 / 131.628 |

Documents 1 and 3 contain sampled paints/images, so the prototype refuses GPU
admission and displays the complete CPU result. The same inputs and all three
resolutions were tested. No GPU path was faster than its CPU counterpart on
the admitted pages in this fresh-render/readback experiment.

At 150 DPI, the dense page's browser CPU preparation is 27.320 ms of its
46.252 ms complete render. The GPU coverage path keeps that preparation,
exports geometry in 24.358 ms, then completes upload/integration/readback in
13.372 ms (kernel 9.368 ms). Individual stage medians need not add to the
median complete-render time of 65.390 ms. The packet is 12,354,036 bytes,
with 358,572 edges and 13,320 CPU-prepared exceptional cell corrections.
The sparse page has 19 exceptional cells.

Thus GPU area integration does real work, but this representation expands
prepared geometry per placement and exports it afresh. Geometry export alone
costs more than the dense CPU path's coverage and composition (18.927 ms).
Native Metal removes neither that cost nor the existing CPU preparation.
Removing readback alone would not recover the dense page's overhead.

## Accuracy

The following are changed pixels against the corresponding CPU at **150 DPI**,
from the final completed readback of each measured cell. This does not assert
that all 100 approximate renders were checked for byte repeatability. Every
CPU and exact-span GPU sample is checked against its CPU reference.

| Input | Browser local | Browser absolute | Native local | Native absolute |
| --- | ---: | ---: | ---: | ---: |
| Document 2 | 29 | 52 | 30 | 53 |
| Document 4 | 7 | 632 | 7 | 632 |

Every changed channel in this table differs by **one of 255 levels**. Across
all admitted real-PDF cells at 72/150/300 DPI, local mode has a maximum
channel error of **1**, with **zero pixels beyond one level**, in both APIs.
Absolute mode reaches error **2** at one pixel of the sparse page at 300 DPI.
At 300 DPI on the dense page, browser changed pixels are 209 local versus
7,366 absolute; native is 198 versus 7,362. GPU APIs/compiler arithmetic can
produce slightly different tie decisions. Local coordinates do not
monotonically reduce changed-pixel counts at every DPI: at 72 DPI on the
dense page, both modes stay within one level but local changes more pixels.

An independent synthetic fractional rectangle translated to origin 2^20
has maximum errors of 1 local / 12 absolute. At origin 2^28 this is
1 local / 255 absolute. Integer pixel offsets are subtracted before shader
f32 conversion. These stress cases demonstrate a precision failure mode;
they are not representative error rates for the four PDF pages.

These are differential observations, not a certified global error bound.
Approximate coverage must remain opt-in; no Exact/conformance claim or
native/WASM pixel-parity guarantee is attached to these GPU floats.

## Validation and provenance

- 183 raster/paint unit tests pass; five existing tests remain ignored.
  The two new checks cover bounded export, unchanged CPU rendering,
  band-independent packets and opaque-background admission.
- Six SVG export host tests pass, with strict host/core Clippy, formatting,
  Swift warnings-as-errors compilation and JS syntax checks.
- 999 coverage fixture decisions: 639 GPU admissions / 360 explicit CPU
  fallbacks. The SIMD and scalar sweeps have zero failures and identical
  CPU/geometry SHA-256 identities for admissions; native agrees on those
  639 CPU/geometry identities. This includes 320 extra band/tile checks.
  Maximum GPU difference is one level; one existing geometry fixture has
  four differing pixels in WebGPU and six in native Metal.
- Four independent polygon oracles (2,048 pixels), one independently
  integrated true quadratic (512 pixels), large-coordinate stress, and
  16 malformed-packet refusals pass. No coarse supersampling is used.
- The original exact GPU span sweep still passes: 999 fixtures, 641
  admissions, 1,916 full-page/band/region checks and the integer kernel oracle.
- Real input and CPU pixel SHA-256 identities agree between native and SIMD
  for all 12 resolution cells; local and absolute packet identities agree
  for the six admitted cells. Scalar real-input parity is recorded separately
  in a one-sample run, which is not performance evidence.
- The actual demo controls, local GPU coverage, zoom, retained SVG source and
  SVG download availability pass the UI check. The generated UI zoom sample
  differs from CPU by at most one level; no new real-PDF zoom timing claim is
  made. Previous SVG zoom evidence remains in the SVG cleanup report.

The timed implementation is commit `61b8ad4bc33eb55207dcef5113f8337e705b8f04`.
Browser and native source/binary hashes are in their reports and were verified
unchanged during each run. These are local shared-desktop observations, not
release performance-gate results. Raw evidence and private PNGs remain ignored
under `target/gpu-demo/`:

- `coverage-browser.json`, `coverage-native.json`
- `coverage-fixtures.json`, `coverage-native-fixtures.json`,
  `coverage-scalar-fixtures.json`, `coverage-scalar-real-parity.json`
- `coverage-span-regression.json`, `coverage-ui-final.json`
- `coverage-browser-visuals/`, `coverage-demo.png`

Browser WASM SHA-256: `58f804f27846d0095da268f4393d7ff058fbacf667a8e7a9812f7003db0f7775`.

Native engine SHA-256: `14f67d1a8419f06feff2428384684c43f96e17b80937ce4754a2fcb02394c203`.

## Direction

Keep the SVG exporter as a useful independent feature, and keep this bounded
coverage experiment for reference. Local coordinates are a useful precision
choice; lowering precision further is not the performance lead in these data.
Before broadening GPU PDF feature support, the next useful experiment is a
shared geometry representation and retained GPU resources, reducing repeated
outline expansion/serialization and moving more binning/preparation work to
the GPU. A fair retained-resource comparison must give the CPU its equivalent
prepared-scene reuse as well. Actual on-screen presentation can be measured
separately from the readback needed for accuracy checks.
