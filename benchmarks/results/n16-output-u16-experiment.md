# Exact N16 output in 16-bit lanes

2026-10-21. Retained as an experimental partial win against `f98ad836`, keeping the glyph memo, clip work, shared opaque accuracy correction and background guard in both arms.

The only product change uses saturating u16 addition in the existing exact shift/subtract converter. Above the saturation boundary both formulas return 255; below it they are identical. It removes a lint exception and adds no storage, cache, dispatch, resource charge or accuracy-mode change.

The fixed confirmation ran 200 fresh pairs and five warmups for each of 46 cells. All 18,860 outputs passed independent RGB, opaque-alpha and same-mode byte checks. No pilot or historical samples enter these estimates.

| Target | Baseline median ms | Candidate median ms | Median ratio (upper) | p95 ratio (upper) | Peak RSS MiB baseline → candidate |
|---|---:|---:|---:|---:|---:|
| opaque64 / exact / full | 4.258333 | 3.797875 | 0.8919 (0.8988) | 0.8786 (0.9412) | 25.515625 → 24.234375 |
| opaque64 / competitive / full | 3.566167 | 3.115730 | 0.8737 (0.8882) | 0.8286 (0.9891) | 26.203125 → 24.765625 |

Target latency gates: 2/2; protected latency gates: 12/44; peak-RSS gates: 25/46. **Full admission remains false.** Every failed gate remains visible below and in the complete JSON.

Ratios are candidate/baseline. Upper bounds are nominal one-sided 95% paired-bootstrap bounds from the pinned harness. They do not establish simultaneous coverage across the matrix or guarantee p95 calibration. The independent auditor recomputes point estimates and verifies artifacts; it does not recompute bootstrap bounds.

Full-page latency covers render after open/compile and excludes input reads and output emission. Cold cells include open/compile but do not measure cold filesystem I/O. RSS covers the whole fresh process. No known competing jobs were observed; this is not controlled-runner certification.

## Accuracy and validation

Full repository validation passed: 876 Rust tests including 210 renderer tests, 196 Python tests, native/WASM and host checks, and 1,249/1,249 conformance cases. The native exhaustive 65,536-word converter test passes, including nonzero source offset, incomplete-pixel tails and excess destination capacity. Independent precision full/tile probes pass both modes under the stated coverage-byte composition model; default Exact host output matches native/WASI. Existing colored-background tests also pass. This does not prove arbitrary-PDF or continuous-geometry correctness.

The first focused build stopped on the obsolete lint expectation; its failure is retained. The corrected build and full check passed. This has no relation to selecting performance samples.

## WASM diagnostic

A separately frozen 12-pair, six-cell Node/WASI screen verified 168 outputs against independently reconstructed analytic pixels. The predeclared material-regression signal did not fire. Every per-cell linear-memory end size matched between sides. This establishes no WASM speed, warmed-browser, total-memory or RSS admission.

| Case / mode | Median ratio | p95 ratio | Linear memory bytes baseline / candidate |
|---|---:|---:|---:|
| opaque64 / exact | 0.9901 | 0.9348 | [19922944] / [19922944] |
| opaque64 / competitive | 0.9794 | 0.9640 | [20578304] / [20578304] |
| opaque-fill / exact | 0.9815 | 1.0207 | [3014656] / [3014656] |
| opaque-fill / competitive | 0.9987 | 0.9787 | [3014656] / [3014656] |
| default-knockout64 / exact | 1.0165 | 0.9901 | [23199744] / [23199744] |
| image-only / exact | 0.9926 | 0.9978 | [2818048] / [2818048] |

Explicit module compile/instantiate and Node startup are outside the internal render timer; lazy compilation and tiering may remain inside its first render. Module hashes, identical Cargo commands, runtime pins, raw samples and independent audit are retained. ARM disassembly shows saturating 16-bit vector addition and narrowing subtraction; that is instruction evidence, not a portable speed claim.

## Same-binary memory diagnostic

A separate fixed diagnostic ran the same hash-bound native binary for both labels: two warmup pairs and twelve measured pairs in three Exact/full cells. All 84 outputs, raw Darwin RSS values and summaries passed independent audit. The collector already uses per-child OS high-water RSS in bytes; it is not a periodic sampler or cumulative process counter. Three pure diagnostic tests pass.

| Case | Median RSS MiB A / B | Maximum RSS MiB A / B | Maximum A/B ratio |
|---|---:|---:|---:|
| outline2-knockout | 9.140625 / 9.140625 | 9.781250 / 9.203125 | 1.0628 |
| default-knockout64 | 23.921875 / 23.921875 | 45.812500 / 31.296875 | 1.4638 |
| opaque64 | 20.781250 / 20.781250 | 21.937500 / 21.375000 | 1.0263 |

Identical code produces appreciable peak variation, including a 46.4% difference between the default-knockout maxima despite equal medians. This small descriptive run neither establishes the cause of prior failures nor subtracts a noise allowance. Every prior gate remains unchanged. Its first audit attempt rejected the macOS platform string because the auditor incorrectly expected Darwin; the corrected auditor verifies the same retained samples. The failed audit and original source remain preserved; no renderer sample was repeated.

## Every native confirmation cell

| Case / mode / operation | Median ratio | p95 ratio | Peak RSS ratio | Latency gate | RSS gate |
|---|---:|---:|---:|---|---|
| clip-churn / competitive / full | 1.0010 | 0.9958 | 1.0015 | pass | miss |
| clip-churn / exact / full | 0.9990 | 1.0178 | 0.9261 | miss | pass |
| default-knockout64 / competitive / full | 1.0164 | 0.9111 | 1.0701 | miss | miss |
| default-knockout64 / exact / full | 0.9857 | 1.0773 | 1.3106 | miss | miss |
| dense64 / competitive / full | 1.0061 | 0.9870 | 1.0101 | miss | miss |
| dense64 / exact / full | 1.0066 | 1.0350 | 1.1583 | miss | miss |
| image-only / competitive / full | 1.0009 | 1.0262 | 1.0000 | miss | pass |
| image-only / exact / full | 0.9886 | 0.9856 | 0.9111 | miss | pass |
| nested64 / competitive / full | 0.9977 | 1.0122 | 0.9971 | miss | pass |
| nested64 / exact / full | 1.0013 | 0.9773 | 0.9614 | miss | pass |
| one-source64 / competitive / full | 1.0018 | 0.9948 | 1.0032 | miss | miss |
| one-source64 / exact / full | 0.9819 | 1.0048 | 1.1968 | miss | miss |
| opaque-clip / competitive / full | 0.9921 | 0.9956 | 1.0013 | miss | miss |
| opaque-clip / exact / full | 0.9830 | 1.0405 | 0.9226 | miss | pass |
| opaque-fill / competitive / full | 0.8063 | 0.7833 | 0.8662 | pass | pass |
| opaque-fill / exact / full | 0.8107 | 0.8778 | 1.2441 | miss | miss |
| opaque-tiny / competitive / cold | 0.9543 | 0.9345 | 0.9765 | pass | pass |
| opaque-tiny / exact / cold | 0.9321 | 0.9464 | 1.0062 | pass | miss |
| opaque64 / competitive / cold | 0.8932 | 0.9157 | 1.0635 | pass | miss |
| opaque64 / competitive / full | 0.8737 | 0.8286 | 0.9451 | pass | pass |
| opaque64 / competitive / tile | 0.9330 | 0.9569 | 1.0055 | miss | miss |
| opaque64 / competitive / tiled | 0.8993 | 0.8979 | 0.9995 | pass | pass |
| opaque64 / exact / cold | 0.9038 | 0.9238 | 0.9628 | pass | pass |
| opaque64 / exact / full | 0.8919 | 0.8786 | 0.9498 | pass | pass |
| opaque64 / exact / tile | 0.9123 | 0.9124 | 1.0207 | pass | miss |
| opaque64 / exact / tiled | 0.8957 | 0.8803 | 0.9747 | pass | pass |
| outline2 / competitive / full | 1.0046 | 1.0009 | 1.0000 | miss | pass |
| outline2 / exact / full | 0.9979 | 0.9911 | 1.2036 | pass | miss |
| outline2-knockout / competitive / full | 0.9985 | 0.9606 | 0.9805 | miss | pass |
| outline2-knockout / exact / full | 0.9974 | 1.0072 | 1.3522 | miss | miss |
| outline2-late64 / competitive / full | 1.0022 | 1.0028 | 1.0108 | miss | miss |
| outline2-late64 / exact / full | 0.9937 | 0.9900 | 1.0967 | miss | miss |
| overcapacity128 / competitive / full | 1.0005 | 0.9681 | 0.9842 | pass | pass |
| overcapacity128 / exact / full | 0.9970 | 1.0129 | 0.9275 | miss | pass |
| sparse64 / competitive / cold | 0.9976 | 1.0083 | 0.9919 | miss | pass |
| sparse64 / competitive / full | 0.9986 | 1.0231 | 0.9869 | miss | pass |
| sparse64 / competitive / tile | 0.9965 | 0.9899 | 1.0442 | miss | miss |
| sparse64 / competitive / tiled | 1.0065 | 1.1256 | 0.9314 | miss | pass |
| sparse64 / exact / cold | 0.9985 | 0.9678 | 1.0527 | miss | miss |
| sparse64 / exact / full | 1.0026 | 0.9795 | 1.1951 | miss | miss |
| sparse64 / exact / tile | 0.9980 | 1.0229 | 0.9633 | miss | pass |
| sparse64 / exact / tiled | 1.0095 | 0.9869 | 1.0443 | miss | miss |
| tiny / competitive / cold | 0.9799 | 0.9688 | 0.9807 | pass | pass |
| tiny / competitive / full | 0.9925 | 1.0370 | 1.0000 | miss | pass |
| tiny / exact / cold | 0.9574 | 1.1719 | 0.9957 | miss | pass |
| tiny / exact / full | 0.9774 | 0.9641 | 0.9770 | miss | pass |

## Retention and limits

Retain only on the experimental renderer stack. Both designated target latency and peak-RSS gates pass, and the exact conversion adds no storage or accuracy change. The 32 protected latency misses and 21 RSS misses remain open; no protected median or p95 lower bound exceeds 1.02 in this confirmation. This absence does not establish non-regression. Largest observed RSS increases are outline2-knockout Exact (+35.2%, 18.5 to 25.015625 MiB), default-knockout64 Exact (+31.1%, 46.328125 to 60.71875 MiB), and opaque-fill Exact (+24.4%, 4.671875 to 5.8125 MiB). No memory-noise allowance, gate relaxation, release promotion or competitor-dominance claim is made. Earlier stack regressions and nonopaque-background costs remain open. Keep the useful measured target gains while profiling the next separately frozen slice.

The five-pair pilot is separate: 920 verified outputs, 1/2 target latency, 13/44 protected latency and 34/46 RSS gates passed. Both target RSS cells passed in that pilot. Its Competitive target upper median bound missed the 5% benefit threshold, and two protected tail lower bounds exceeded 1.02; those controls stayed in confirmation.

[Complete audited results and evidence hashes](n16-output-u16-experiment.json), [frozen native plan](n16-output-u16-plan.json), [source/binary bindings](n16-output-u16-bindings.json), [separate pilot](n16-output-u16-pilot.md). Native baseline reuses the prior guarded candidate binary whose product fingerprint matches committed `f98ad836`; the WASM baseline was rebuilt independently from its pristine control checkout.

Historical percentages are not added together. The earlier opaque accuracy correction still has measured costs versus its legacy route, and nonopaque backgrounds still use the corrected, more expensive F64 path. A direct net-stack and fair competitor comparison remains required. The standing goal remains active.
