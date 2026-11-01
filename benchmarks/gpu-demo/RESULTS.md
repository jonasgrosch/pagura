# GPU compositor experiment: local results

The CPU renderer wins the render-stage comparison on every admitted workload in this run. Moving exact N16 compositing to WebGPU while retaining CPU coverage costs more in packet construction and transfer than it saves in compositing. This is a result about this hybrid experiment, not a bound on a future GPU rasterizer.

Measured on 2026-10-22, Apple M3 Pro (12 CPU cores, 18 GPU cores, 18 GB), macOS 15.6.1, Chrome 155 / Metal hardware adapter. Rendering revision: `8e3a7aba9ed64419c506a0bbce25a78b0f6b5c1a`. Rust: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Shared desktop, with other rendering work active: these are exploratory timings, with no release-gate or cross-machine claim.

Each cell has 100 samples after five warmups. Browser CPU and hybrid alternate CPU/GPU/GPU/CPU. Native CPU is additional context from a separate run. Every measured output is checked against its CPU reference outside timing. Both browser paths use the same SIMD WASI build and document state.

## At 150 DPI

All times are median milliseconds. Hybrid includes CPU preparation, exact coverage, packet building, upload, completed GPU execution and RGBA readback. CPU includes its preparation, coverage and compositing. Worker output transport is excluded here and included in the frame table below.

| Workload | Native CPU | Browser CPU | Hybrid | Hybrid / browser CPU | GPU compute only |
|---|---:|---:|---:|---:|---:|
| Text page | 14.11 | 12.54 | 20.01 | 1.60× | 1.16 |
| Vector chart | 4.28 | 6.21 | 12.62 | 2.03× | 1.06 |
| Overlapping fills | 14.27 | 11.38 | 21.17 | 1.86× | 1.52 |
| Image (CPU fallback) | 4.63 | 14.04 | CPU fallback | — | — |
| Transparency (CPU fallback) | 4.46 | 11.07 | CPU fallback | — | — |
| Real text page | 11.01 | 15.07 | CPU fallback | — | — |
| Real vector page | 2.60 | 3.15 | 6.27 | 1.99× | 0.37 |
| Real mixed page with images | 13.20 | 19.44 | CPU fallback | — | — |
| Real mixed geometry page | 37.54 | 47.95 | 56.89 | 1.19× | 1.20 |

Images and dynamic opacity use the CPU throughout this experiment; the two real fallback pages include sampled paint. Fallbacks are explicit and have no GPU speed ratio. The native column is measured separately, so it should not be read as a paired native-versus-WASM ranking.

## Resolution sensitivity

Each cell is **browser CPU / hybrid**, median ms; only admitted workloads have a GPU comparison.

| Workload | 72 DPI | 150 DPI | 300 DPI |
|---|---:|---:|---:|
| Text page | 6.26 / 8.77 | 12.54 / 20.01 | 26.63 / 45.24 |
| Vector chart | 2.98 / 6.48 | 6.21 / 12.62 | 14.94 / 32.06 |
| Overlapping fills | 3.25 / 7.92 | 11.38 / 21.17 | 39.95 / 56.61 |
| Real vector page | 1.56 / 3.15 | 3.15 / 6.27 | 7.29 / 14.69 |
| Real mixed geometry page | 33.01 / 36.64 | 47.95 / 56.89 | 80.30 / 111.01 |

## Full frame latency at 150 DPI

These medians also include the worker/WASI transport. They exclude Canvas presentation and diagnostic timestamp-query readback.

| Workload | Browser CPU | Hybrid |
|---|---:|---:|
| Text page | 14.70 | 21.23 |
| Vector chart | 8.50 | 13.62 |
| Overlapping fills | 13.79 | 24.47 |
| Real vector page | 5.33 | 6.59 |
| Real mixed geometry page | 49.69 | 58.06 |

The real vector page at 300 DPI was near parity in frame latency (CPU 16.00 ms, hybrid 15.66 ms), despite a slower hybrid render stage. Its smaller program saves worker transport of a full CPU RGBA surface. The roughly 2% difference is too small for a speedup claim on this desktop.

## Where the time goes

At 150 DPI, the text program is 5.30 MiB. CPU preparation is 0.57 ms in either path; CPU band execution is 11.97 ms. Hybrid coverage and packet construction take 15.38 ms, and completed GPU upload/execution/readback takes 4.07 ms. The GPU compute portion is 1.16 ms. Stage medians do not add exactly to a median of totals.

First adapter/pipeline setup was 205.47 ms. Program validation and first GPU buffer allocation are reported separately for each case; for the 150-DPI text page they were 17.44 ms. Document opening, WASM compilation and this GPU setup are outside warm timings.

The next useful experiments are reducing or streaming the coverage packet and keeping output GPU-resident for a measured viewer workload. General GPU coverage, sampled paints and F32 composition need their own validated implementations; F64-dependent work retains its CPU execution. A prepared-program cache needs viewer reuse evidence before it is added.

## Verification and evidence

- 999 generated unpassworded render fixtures: 641 admitted, 358 explicit CPU fallbacks; zero failures.
- 1,916 hardware GPU checks of full pages, band heights and offset regions in each of SIMD and scalar WASI.
- Native/SIMD/scalar CPU and 641 packet SHA-256 comparisons agree.
- Independent shader oracle: 131,072 pixels covering all N16 output words and all coverage bytes; four malformed packets refused before dispatch.
- 2,700 browser timing samples, including 1,500 GPU renders; 2,700 native CPU timing samples. All repeated outputs verified.
- Workspace tests: 1,084 passed; feature-enabled paint tests: 91 passed; 1,246/1,246 conformance cases; four release reference tests; strict workspace/feature clippy, docs, fixtures, policy, Python lint/types and existing host/viewer checks passed.

Raw samples, p95s, input hashes, hardware information, source/binary hashes and screenshots remain local under ignored `target/gpu-demo/`: `browser-bench.json`, `native-bench.json`, `hardware-tests.json`, `scalar-hardware-tests.json`, `native-parity.json`, `simd-scalar-parity.json`, and `demo.png`. [`README.md`](README.md) reproduces the generated workloads and explains adding local documents with `--pdf`; source documents are not redistributed.
