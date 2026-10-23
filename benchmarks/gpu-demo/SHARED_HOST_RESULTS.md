# Shared-buffer native presentation and WebGPU comparison

GPU presentation remains useful for repeated dense rasterization after removing
RGBA pipe copies. It does not win every interaction: CPU is competitive at
opening and faster for new dense zoom targets; cached scrolling is essentially
a tie. World geometry is now a credible retained path, with a substantial
native packet-writing cost removed, but still pays for CPU view preparation
and remains approximate.

The current measurements use `ffb2352ed33552f8d9b98ec36e0e727ad17c2cda`,
after merging `render/one-pass` at
`f86eb5da130286b5c9f270e367a7e4daafa68a89`. Later changes document the
channel scopes and results; rendering and timing code are unchanged. This is
an exploratory study on one shared Apple M3 Pro desktop (12 CPU / 18 GPU cores,
18 GB), macOS 15.6.1 and Chrome 155, not release-gate evidence. Earlier results
on the old base cannot isolate the benefit of removing pipes, because geometry
and CPU preparation have also changed.

The four real inputs retain their original anonymous order, page 0 and white
opaque background. Documents 1 and 3 contain sampled paints and use complete
CPU fallback. Document 2 is sparse vectors; Document 4 is dense vectors/text.
Private inputs, rendered pages and raw evidence remain outside Git.

## Host boundary and safety

A Swift application owns the Rust engine child, Metal resources and AppKit
window. Two private 64 MiB shared files carry RGBA and geometry/view packets.
Rust uses safe `std::fs`/`std::io::Write`: prepared rendering streams the base's
`render_bands` consumer into the frame file, reusing one band allocation.
The Swift host maps a page-aligned VM region and wraps it as a Metal
`bytesNoCopy` shared buffer. The Metal buffer owns the mapping's lifetime.
Every GPU command completes before the next mutating engine request.

The PDF is uploaded once on the input pipe. Subsequent draw requests are small
control messages, and engine responses contain JSON metadata only, capped at
16 KiB. RGBA and geometry never cross the response pipe. Node launches the
application and saves its final report; it does not relay frames. Standard
file writes still copy into kernel memory. This is a shared-buffer process
host, not a fully in-process Rust/Metal API or zero-copy rendering.

No unsafe Rust, Rust FFI, native Rust dependency or third-party product crate
was added. Native mapping and Metal pointer APIs stay in the separate Swift
host; their extents and lifetimes are checked there. The default CPU renderer
and independent SVG exporter remain available.

This follows Apple's [buffer wrapping contract](https://developer.apple.com/documentation/metal/mtldevice/makebuffer(bytesnocopy:length:options:deallocator:))
and [shared-memory synchronization contract](https://developer.apple.com/documentation/metal/mtlstoragemode/shared).

## Workloads and clocks

- Opening: 20 rotating/reversing document-cold trials per mode at 150 DPI,
  including a fresh native engine process/shared files or browser worker/WASM
  compilation, document open and first display. The application, adapter and
  presentation surface stay warm. File read/fetch is separate. Initial
  application/presentation setup is separate; these are not application-cold
  first-launch measurements.
- Redraw: 100 fresh whole-page rasterizations after five warmups at
  72/150/300 DPI, rotating/reversing CPU prepared reuse, per-DPI retained GPU
  and world GPU. CPU, GPU output storage and preparation/resources are reused.
- New zoom: twelve different targets, 151–299 DPI as listed in `policy.json`.
  CPU prepared reuse is cleared per mode/target; the recorded scene, Store
  and pipelines stay warm. All view preparation, serialization, transfer,
  allocation/upload, rasterization and presentation count.
- Scrolling: 100 viewport changes after five warmups over a retained 300-DPI
  full-page bitmap, rotating/reversing all modes. **CPU receives the same
  bitmap reuse.** Counter snapshots verify zero engine requests, coverage
  dispatches and page-content uploads. This is cached pan within one page;
  uncached tiles, newly opened pages and document-wide cache pressure are not
  measured.
- Accuracy: fresh CPU, prepared CPU and GPU readbacks plus top/middle/bottom
  attachment captures run outside timing at all three static and twelve novel
  DPIs. Native and WASM CPU SHA-256 agree in all 180 paired views.

Native frames end at actual drawable presentation-or-discard callbacks.
Browser frames end after completed work and two animation-frame opportunities.
These are different clocks and neither measures display scanout. Both final
world modes use the explicit `--world-reciprocal` variant; the original
normalization shader remains available as the default control. The browser
CPU also avoids an extra RGBA typed-array copy before Canvas upload.

All tables give milliseconds as **median / p95**. Small differences at a
frame-clock floor should be treated as ties.

## Document opening before rendering

The engine's open stage includes document construction and page-tree counting;
interpretation, render preparation and first display follow it. These values
pool all 60 equivalent opens per document (20 per selected backend). Request
time also includes the one-time PDF input transfer. Native process startup can
continue into that first request, while the browser worker is already ready;
the full first-display measurements remain the appropriate end-to-end values.

| Document | Native engine open | Native open request | WASM engine open | Browser open request |
|---|---|---|---|---|
| 1 | 0.13 / 0.17 | 2.57 / 4.21 | 0.11 / 0.19 | 0.75 / 1.04 |
| 2 | 0.49 / 0.95 | 3.91 / 5.53 | 0.32 / 0.38 | 0.95 / 1.19 |
| 3 | 0.09 / 0.11 | 2.50 / 3.09 | 0.07 / 0.10 | 0.68 / 1.00 |
| 4 | 0.43 / 0.47 | 2.72 / 3.37 | 0.36 / 0.43 | 0.94 / 1.14 |

One application/presentation initialization costs 247.85 ms
in the final native run; browser adapter/pipeline initialization is
246.22 ms. Their scopes differ and they are
reported separately, without adding medians into a synthetic first-launch
number.

## Dense-page interaction results

| Host | Operation | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| Native Metal | First display, 150 DPI | 97.70 / 102.92 | 94.57 / 101.77 | 176.95 / 211.13 |
| Native Metal | Fresh redraw, 300 DPI | 58.93 / 75.16 | 41.88 / 58.25 | 41.47 / 50.46 |
| Native Metal | 12 new zoom targets | 99.76 / 124.36 | 127.69 / 166.94 | 124.94 / 158.05 |
| Native Metal | Cached scrolling | 24.97 / 25.67 | 25.03 / 25.91 | 25.10 / 25.87 |
| Browser WebGPU | First display, 150 DPI | 116.58 / 123.80 | 133.23 / 134.57 | 216.60 / 233.26 |
| Browser WebGPU | Fresh redraw, 300 DPI | 66.66 / 83.35 | 33.32 / 33.40 | 33.33 / 33.51 |
| Browser WebGPU | 12 new zoom targets | 117.53 / 215.60 | 140.48 / 183.05 | 116.61 / 149.74 |
| Browser WebGPU | Cached scrolling | 33.30 / 33.44 | 33.32 / 33.48 | 33.32 / 33.47 |

In the final full run, dense native redraws improve from 58.93 ms CPU to
41.88 ms per-DPI GPU. Browser redraws improve from 66.66 to 33.32 ms. Native
first display is effectively tied for CPU/per-DPI GPU, while world setup costs
more. CPU wins new native zoom targets; browser world zoom is effectively tied
with CPU. Cached scrolling needs no new rasterization and ties on both hosts.

Rendering work excludes the display wait. CPU work includes its engine
request/shared writes or worker transfer and Canvas upload; GPU work includes
any needed setup and completed GPU execution. End-to-end frames above are the
primary comparison.

| Host | 300-DPI work | CPU | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| Native Metal | Completed rendering | 40.02 / 50.52 | 21.88 / 34.19 | 18.41 / 29.31 |
| Browser WebGPU | Completed rendering | 53.35 / 68.12 | 16.96 / 21.61 | 13.18 / 17.57 |

## Native Metal: full redraw frames

| Document | DPI | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| 1 | 72 | 33.33 / 34.22 | CPU fallback | CPU fallback |
| 1 | 150 | 33.30 / 34.47 | CPU fallback | CPU fallback |
| 1 | 300 | 49.60 / 50.82 | CPU fallback | CPU fallback |
| 2 | 72 | 24.97 / 25.85 | 24.93 / 25.88 | 24.99 / 25.83 |
| 2 | 150 | 25.15 / 33.43 | 24.99 / 25.86 | 24.98 / 25.84 |
| 2 | 300 | 33.58 / 41.87 | 25.12 / 33.29 | 25.01 / 26.14 |
| 3 | 72 | 25.43 / 33.66 | CPU fallback | CPU fallback |
| 3 | 150 | 33.35 / 41.01 | CPU fallback | CPU fallback |
| 3 | 300 | 50.74 / 66.22 | CPU fallback | CPU fallback |
| 4 | 72 | 33.49 / 41.60 | 25.03 / 25.79 | 25.05 / 25.81 |
| 4 | 150 | 41.78 / 50.59 | 29.22 / 34.00 | 25.73 / 33.95 |
| 4 | 300 | 58.93 / 75.16 | 41.88 / 58.25 | 41.47 / 50.46 |

## Browser WebGPU: full redraw frames

| Document | DPI | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| 1 | 72 | 33.31 / 33.83 | CPU fallback | CPU fallback |
| 1 | 150 | 33.33 / 33.79 | CPU fallback | CPU fallback |
| 1 | 300 | 66.67 / 83.30 | CPU fallback | CPU fallback |
| 2 | 72 | 33.36 / 34.41 | 33.31 / 34.67 | 33.30 / 34.24 |
| 2 | 150 | 33.34 / 34.46 | 33.32 / 34.78 | 33.31 / 34.81 |
| 2 | 300 | 33.34 / 33.40 | 33.31 / 33.38 | 33.32 / 33.37 |
| 3 | 72 | 33.33 / 33.38 | CPU fallback | CPU fallback |
| 3 | 150 | 33.33 / 66.24 | CPU fallback | CPU fallback |
| 3 | 300 | 66.76 / 84.08 | CPU fallback | CPU fallback |
| 4 | 72 | 33.32 / 33.80 | 33.32 / 33.66 | 33.33 / 33.86 |
| 4 | 150 | 33.32 / 33.45 | 33.33 / 33.42 | 33.32 / 33.42 |
| 4 | 300 | 66.66 / 83.35 | 33.32 / 33.40 | 33.33 / 33.51 |

## Opening and first display: all documents

| Host | Document | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| Native Metal | 1 | 64.59 / 69.41 | 64.94 / 77.00 (CPU fallback) | 77.32 / 85.54 (CPU fallback) |
| Native Metal | 2 | 43.54 / 44.63 | 36.00 / 44.56 | 44.61 / 53.05 |
| Native Metal | 3 | 69.03 / 77.52 | 69.11 / 71.31 (CPU fallback) | 68.63 / 70.41 (CPU fallback) |
| Native Metal | 4 | 97.70 / 102.92 | 94.57 / 101.77 | 176.95 / 211.13 |
| Browser WebGPU | 1 | 83.25 / 83.34 | 83.27 / 99.51 (CPU fallback) | 83.24 / 99.88 (CPU fallback) |
| Browser WebGPU | 2 | 49.97 / 60.25 | 49.90 / 67.57 | 66.04 / 68.44 |
| Browser WebGPU | 3 | 83.25 / 83.34 | 83.25 / 99.83 (CPU fallback) | 83.26 / 99.96 (CPU fallback) |
| Browser WebGPU | 4 | 116.58 / 123.80 | 133.23 / 134.57 | 216.60 / 233.26 |

## New zoom targets: all documents

| Host | Document | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| Native Metal | 1 | 50.33 / 101.97 | 50.40 / 174.49 (CPU fallback) | 49.92 / 75.53 (CPU fallback) |
| Native Metal | 2 | 33.23 / 33.83 | 33.31 / 41.61 | 24.73 / 25.30 |
| Native Metal | 3 | 57.71 / 67.18 | 58.31 / 66.97 (CPU fallback) | 57.76 / 75.01 (CPU fallback) |
| Native Metal | 4 | 99.76 / 124.36 | 127.69 / 166.94 | 124.94 / 158.05 |
| Browser WebGPU | 1 | 66.52 / 99.13 | 66.96 / 99.27 (CPU fallback) | 66.94 / 102.79 (CPU fallback) |
| Browser WebGPU | 2 | 33.15 / 33.21 | 33.12 / 33.19 | 33.12 / 33.19 |
| Browser WebGPU | 3 | 66.90 / 84.02 | 66.39 / 84.62 (CPU fallback) | 67.00 / 83.47 (CPU fallback) |
| Browser WebGPU | 4 | 117.53 / 215.60 | 140.48 / 183.05 | 116.61 / 149.74 |

## Cached scrolling: all documents

| Host | Document | CPU prepared | Per-DPI GPU | World GPU |
|---|---|---|---|---|
| Native Metal | 1 | 24.99 / 26.01 | 25.01 / 25.81 (CPU fallback) | 25.00 / 25.82 (CPU fallback) |
| Native Metal | 2 | 25.05 / 25.86 | 25.01 / 25.87 | 24.95 / 25.85 |
| Native Metal | 3 | 25.06 / 25.88 | 25.03 / 25.82 (CPU fallback) | 25.00 / 25.76 (CPU fallback) |
| Native Metal | 4 | 24.97 / 25.67 | 25.03 / 25.91 | 25.10 / 25.87 |
| Browser WebGPU | 1 | 33.24 / 35.47 | 33.11 / 35.30 (CPU fallback) | 33.21 / 35.31 (CPU fallback) |
| Browser WebGPU | 2 | 33.32 / 33.38 | 33.32 / 33.37 | 33.32 / 33.36 |
| Browser WebGPU | 3 | 33.41 / 34.51 | 33.15 / 34.41 (CPU fallback) | 33.32 / 34.57 (CPU fallback) |
| Browser WebGPU | 4 | 33.30 / 33.44 | 33.32 / 33.48 | 33.32 / 33.47 |

## Timing variability and retained controls

Native GPU work varies across runs on this desktop. No samples are removed.
The original division control and reciprocal variant were run with the same
pre-cleanup binary; the final full run and a separate dense-only repeat use the
same final binary and bulk serializer. The repeat's input hash identifies the
same Document 4 even though its one-input report labels it Document 1.

| Native run | Mode | 300-DPI work | 300-DPI frame |
|---|---|---|---|
| Division, before bulk copy | cpu | 34.75 / 45.01 | 57.99 / 67.31 |
| Division, before bulk copy | retained | 9.45 / 16.48 | 33.08 / 34.16 |
| Division, before bulk copy | world | 12.92 / 19.34 | 33.43 / 42.08 |
| Reciprocal, before bulk copy | cpu | 37.97 / 45.96 | 58.47 / 67.17 |
| Reciprocal, before bulk copy | retained | 10.63 / 16.70 | 33.33 / 41.61 |
| Reciprocal, before bulk copy | world | 9.67 / 16.02 | 32.58 / 34.34 |
| Final full run | cpu | 40.02 / 50.52 | 58.93 / 75.16 |
| Final full run | retained | 21.88 / 34.19 | 41.88 / 58.25 |
| Final full run | world | 18.41 / 29.31 | 41.47 / 50.46 |
| Final dense-only repeat | cpu | 37.88 / 46.80 | 58.52 / 67.23 |
| Final dense-only repeat | retained | 10.67 / 16.92 | 33.33 / 40.67 |
| Final dense-only repeat | world | 9.51 / 17.39 | 32.88 / 41.47 |

The final dense-only repeat gives 58.52 ms CPU versus 33.33 ms per-DPI GPU,
compared with 58.93 versus 41.88 in the full run. Both show a dense redraw
benefit; they do not establish one stable universal speedup. GPU work also
varies in the browser runs. The cause is not established, and cross-host wall
times should not be interpreted as an intrinsic WebGPU-versus-Metal ranking.

The reciprocal experiment computes clip reciprocals once per mark/pixel and
multiplies edges instead of repeating divisions. In the pre-cleanup native
A/B, dense 300-DPI world work changes from 12.92 to 9.67 ms, while the per-DPI
control changes from 9.45 to 10.63. It is promising but remains an opt-in
arithmetic experiment, with unchanged approximate accuracy and admission.

The larger native new-zoom issue was host serialization. Before the bulk copy,
median world view preparation minus engine time was 63.59 ms on the dense
page; afterward it is 10.44 ms. New-target world frames change from
178.10 / 240.56 to 124.94 / 158.05 ms. CPU controls change from 107.49 to
99.76 ms and per-DPI GPU controls from 124.74 to 127.69. All 180 native output
hashes are identical before/after the bulk serializer change. Host overhead
was a real implementation cost; the remaining CPU view preparation is still
material.

## Accuracy and resource observations

| Host | Document | CPU maximum error | Per-DPI GPU maximum | World GPU maximum |
|---|---|---|---|---|
| Native Metal | 2 | 0 | 1 | 2 |
| Native Metal | 4 | 0 | 1 | 1 |
| Browser WebGPU | 2 | 0 | 1 | 2 |
| Browser WebGPU | 4 | 0 | 1 | 1 |

Per-DPI GPU stays within one channel level; world geometry reaches two on
Document 2 and one on Document 4. These are differential observations against
Exact CPU, not an Exact claim or a general precision bound. CPU prepared reuse
and fallback output remain byte-identical. All 540 final attachment captures
per host equal their source viewport bytes exactly. Native final timed draws
report 5,184/5,184 presented attempts, zero discarded; initial warmup and
untimed diagnostic frames are separate. Browser opportunities are not an
actual-presented counter.

Native transport counters report zero RGBA pipe bytes and zero Swift RGBA
upload bytes. The dense CPU stream reuses a 317,568-byte band rather than
collecting a 34,813,392-byte page. Standard file-write copying remains. Warm
GPU redraws request no CPU geometry, and cached scrolling changes neither
engine nor GPU coverage/upload counters.

World geometry uploads once per page, keeps one maximum-size output and at
most three view buffers. The dense canonical packet is 3,808,252 bytes;
16 views through the static/novel/reset sequence upload 132,342,496 bytes of
view data. Sparse-page equivalents are 1,109,532 and 788,960 bytes. The dense
view traffic is still substantial because resolved rows are currently exported
as per-cell corrections. Nominal source/output/view buffers are bounded at
256 MiB per GPU cache; native frame and packet mappings are separately fixed
at 64 MiB each. These are reservations/mappings, not RSS measurements.

The prepared CPU LRU remains three entries and 256 MiB charged bytes; Store is
64 MiB and preparation uses the base's 512 MiB limit. The experimental WASM
worker has a 1 GiB aggregate limit for recorded/prepared/exported data and
allocator high-water use. The production browser host retains its 512 MiB
default.

## Path to fewer world-geometry compromises

The prototype's limitations are not inherent requirements of GPU rendering.
The next clean step is to retain source geometry and separate view preparation
from pixel execution at the existing scene/preparation boundary:

1. Export the base's resolved boundary lines for exceptional rows instead of
   CPU-quantized coverage cells. That can reduce view serialization and CPU
   quantization while keeping the same source order and clipping semantics.
   This path is identified, not implemented or timed here.
2. Move view transformation, adaptive curve work, binning and exceptional-row
   coverage toward GPU preparation. Retain source outlines independently of
   the current fixed 300-DPI vectorization. The current shader arithmetic
   experiment does not remove CPU topology/certification at a new DPI.
3. Represent hairlines, point cells, adjusted strokes and fixed-size
   annotations as explicit device-dependent commands. Scaling their geometry
   alone cannot preserve those semantics. Local f64 rebasing before f32 and
   selective precision refinement can preserve more accuracy than globally
   narrowing every coordinate; a general Exact proof remains open.
4. Compare visible-region/tile preparation and a bounded document cache for
   both CPU and GPU before drawing conclusions about a production viewer.
   Whole-page cached pans do not measure newly exposed uncached content.

For architectural context, [Vello's current architecture](https://github.com/linebender/vello/blob/main/ARCHITECTURE.md)
separates a shared sparse-strip CPU/GPU family from its compute-oriented
research renderer. That supports exploring different preparation/execution
splits; it is not evidence for Pagura's accuracy or performance, and no code
from that project is used. [WGSL's floating-point contract](https://gpuweb.github.io/gpuweb/wgsl/#floating-point-evaluation)
remains relevant to any attempt at an Exact GPU backend.

GPU is therefore a useful optional display path for admitted dense pages.
These measurements do not justify making it the universal native or WASM
release default. CPU remains the complete Exact path; two of four real inputs
still require it. SVG export stays independent and valuable as an export
feature.

## Validation and evidence

- The pinned full `tools/check.py` passes, including 1,246 conformance cases,
  independent-reference comparisons and band/partition/cache invariance.
- Feature-enabled paint/raster tests, strict Clippy, twelve engine tests and
  Swift warnings-as-errors pass. Shared output tests compare streamed bands
  with collected bytes across regions/reuse and enforce output bounds.
- World sweeps: 999 decisions, 569 admissions, 430 explicit refusals and
  240 zoom variants, passing for division and reciprocal shaders, native
  Metal, SIMD WASM and scalar WASM. Observed admitted fixture errors stay
  within one level; native/WASM CPU, geometry and patch hashes agree.
- Exact compositor regression: 999 decisions, 641 admissions and 1,916
  variants; integer composition remains byte-identical.
- Retained regression: 999 decisions, 639 admissions, 360 refusals and
  320 variants. Independent area/placement oracles, malformed packets/views,
  nine additional native shared-view refusals and the independent endian
  fixture pass. Opening/scroll controls and live scroll pass actual browser
  interaction tests. The independent SVG export sweep completes all 999
  fixture decisions without failures, including retained artifact identity.

Reproduce using the commands in `README.md`, with the same four `--pdf`
arguments in their original order and `--world-reciprocal` on both final
runners. Raw reports carry source and binary hashes, all samples, input hashes,
resource counters, accuracy and unchanged-source checks. Final runs and the
repeat all report unchanged sources/binaries and zero failures. Evidence is
local and ignored; the code checkpoints remain reproducible in Git.

| Raw report | SHA-256 |
|---|---|
| target/gpu-demo/integrated-native.json | b764d2c7da5ace70bfac59f428bc4da51039d76c617f1e1a1888540ed84ab5fb |
| target/gpu-demo/integrated-native-reciprocal.json | 0fe75491eed3e56243c1e2aaf1f2a600538a57c694a92ac6144db6b7d21c7e39 |
| target/gpu-demo/integrated-browser.json | ea51d190a4473024998979b50271891bdd1dc224be43ce0492b9faa18a01b638 |
| target/gpu-demo/shared-native.json | 3dab5a3f0b1250a4db14d3b6ac13c359eb3a6047fa1621a39a6ca55c32b38586 |
| target/gpu-demo/shared-browser.json | 5ea92974cb7811fafd851b77377c365a7b14c69e1984d7838fb174c9ce94cfc2 |
| target/gpu-demo/shared-native-dense-repeat.json | 7562103bf017491f7c796c9c6bcb2152da404a9bfc57a9dc1f877275f4d682f6 |
