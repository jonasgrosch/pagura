# Exact N16 conversion with corrected background admission

2026-10-21. Retained as an experimental partial win on the glyph-memo/shared-composition stack. The flat shift/subtract quantizer is exactly equivalent to `floor((word + 128) / 257)` for every u16 input and adds no buffer or retained state. Nonopaque colored backgrounds now use the existing F64 path to preserve premultiplied output.

The fresh frozen confirmation ran 200 pairs and five warmups per cell, verifying 18,860 outputs against independent RGB, opaque alpha and same-mode baseline bytes. Every cell remains in the report; no old or pilot samples were reused.

| Target | Baseline median ms | Candidate median ms | Median ratio (95% upper) | p95 ratio (95% upper) | Peak RSS MiB baseline → candidate |
|---|---:|---:|---:|---:|---:|
| opaque64 / exact / full | 4.859750 | 4.250125 | 0.8746 (0.8919) | 0.8418 (0.9396) | 26.078125 → 27.218750 |
| opaque64 / competitive / full | 4.141978 | 3.546771 | 0.8563 (0.8645) | 0.8470 (0.9792) | 24.250000 → 24.328125 |

Target latency gates: 2/2; protected latency gates: 15/44; peak-RSS gates: 25/46. Full admission is **false**. Known competing jobs observed during confirmation: False. Process observation can miss short jobs and is not controlled-runner certification.

The target medians are 12.5% lower in Exact and 14.4% lower in Competitive.
Both target RSS gates fail: sampled peaks rise 4.4% and 0.3%, respectively.
The default-knockout Exact control has a supported p95 regression: ratio
1.1729, with lower bound 1.0337 above the protected 1.02 threshold. Its median
ratio is 1.0036. The largest RSS ratio is outline2-knockout Exact, from
12.984375 to 20.625 MiB (1.5884). Unchanged converter storage does not erase
these observed failures or establish their cause. Retention is explicitly
experimental while protected tail and RSS costs are investigated.

For full-page targets, latency is rendering after open/compile; input storage reads and output emission are excluded. Cold cells include open and compile but are not cold-I/O measurements. Each sample runs in a fresh process. Peak RSS covers that whole process.

The five-pair pilot separately passed 920 outputs, 2/2 target latency gates, 16/44 protected latency gates and 36/46 RSS gates. It informed launching the already frozen confirmation; it was not pooled into it.

Full integrated validation passed: 210 renderer tests, 196 Python tests, all 1,249 conformance cases, native/WASM and host checks. The eight-output independent precision probe still returns 219 and 5 within strict rational bounds. Background tests cover alpha 0/128/254/255 in both modes with full and offset tiles. The original failing transparent-background reproducer remains retained.

The background fix uses F64 working pixels: 56 bytes versus N16 eight. It can hit existing resource limits sooner; no limits were increased. This opaque-background performance matrix does not measure that correction’s cost. The earlier shared opaque migration still has material costs versus its legacy baseline; these incremental results do not establish a net legacy-route win.

The previous quantizer confirmation was canceled for an independently reproduced accuracy defect after six complete cells. Its launch is consumed and no samples were reused. This corrected revision has its own source/binary bindings, plan, fixed seeds and single completed confirmation.

## Every measured cell

Ratios are candidate/baseline. Latency passes require both frozen median and p95 upper bounds. Bounds are nominal, marginal bootstrap estimates, without simultaneous-coverage or guaranteed p95-calibration claims. RSS passes require peak ratio ≤1. A bound miss alone is not proof of slowdown; lower bounds and all raw metrics remain in the JSON.

| Case / mode / operation | Median ratio | p95 ratio | Peak RSS ratio | Latency gate | RSS gate |
|---|---:|---:|---:|---|---|
| clip-churn / competitive / full | 0.9978 | 1.0027 | 0.9882 | pass | pass |
| clip-churn / exact / full | 0.9995 | 0.9988 | 0.8547 | pass | pass |
| default-knockout64 / competitive / full | 0.9991 | 0.9178 | 0.9282 | miss | pass |
| default-knockout64 / exact / full | 1.0036 | 1.1729 | 1.1725 | miss | miss |
| dense64 / competitive / full | 0.9977 | 0.9911 | 1.0101 | miss | miss |
| dense64 / exact / full | 1.0029 | 1.0267 | 1.0972 | miss | miss |
| image-only / competitive / full | 0.9919 | 0.9981 | 1.0000 | miss | pass |
| image-only / exact / full | 0.9945 | 0.9905 | 0.9493 | pass | pass |
| nested64 / competitive / full | 0.9974 | 0.9750 | 0.9890 | pass | pass |
| nested64 / exact / full | 1.0004 | 1.0300 | 1.1475 | miss | miss |
| one-source64 / competitive / full | 0.9961 | 1.0115 | 1.0048 | miss | miss |
| one-source64 / exact / full | 0.9978 | 1.0487 | 1.1734 | miss | miss |
| opaque-clip / competitive / full | 0.9936 | 0.9746 | 0.9880 | pass | pass |
| opaque-clip / exact / full | 0.9884 | 0.9822 | 0.9012 | pass | pass |
| opaque-fill / competitive / full | 0.8588 | 0.8638 | 0.9663 | pass | pass |
| opaque-fill / exact / full | 0.8579 | 0.8858 | 0.7350 | pass | pass |
| opaque-tiny / competitive / cold | 1.0156 | 1.0343 | 0.9702 | miss | pass |
| opaque-tiny / exact / cold | 1.0183 | 0.9862 | 0.8670 | miss | pass |
| opaque64 / competitive / cold | 0.8827 | 0.8969 | 1.0975 | miss | miss |
| opaque64 / competitive / full | 0.8563 | 0.8470 | 1.0032 | pass | miss |
| opaque64 / competitive / tile | 0.9457 | 0.9710 | 0.9873 | pass | pass |
| opaque64 / competitive / tiled | 0.9272 | 0.9631 | 0.9830 | miss | pass |
| opaque64 / exact / cold | 0.8967 | 0.9119 | 1.0249 | pass | miss |
| opaque64 / exact / full | 0.8746 | 0.8418 | 1.0437 | pass | miss |
| opaque64 / exact / tile | 0.9410 | 0.9258 | 1.0547 | pass | miss |
| opaque64 / exact / tiled | 0.9335 | 0.8203 | 0.9437 | pass | pass |
| outline2 / competitive / full | 1.0020 | 1.0055 | 0.9887 | miss | pass |
| outline2 / exact / full | 0.9997 | 0.9973 | 1.0105 | miss | miss |
| outline2-knockout / competitive / full | 0.9977 | 0.9063 | 1.1955 | miss | miss |
| outline2-knockout / exact / full | 0.9986 | 0.9913 | 1.5884 | miss | miss |
| outline2-late64 / competitive / full | 1.0093 | 0.9969 | 1.0652 | pass | miss |
| outline2-late64 / exact / full | 1.0025 | 1.0299 | 0.7726 | miss | pass |
| overcapacity128 / competitive / full | 0.9939 | 0.9485 | 1.0021 | miss | miss |
| overcapacity128 / exact / full | 0.9963 | 1.0175 | 1.1460 | miss | miss |
| sparse64 / competitive / cold | 0.9945 | 1.0007 | 0.9970 | miss | pass |
| sparse64 / competitive / full | 0.9987 | 0.9818 | 0.9956 | pass | pass |
| sparse64 / competitive / tile | 0.9971 | 0.9784 | 0.9465 | miss | pass |
| sparse64 / competitive / tiled | 1.0041 | 1.0362 | 1.0019 | miss | miss |
| sparse64 / exact / cold | 0.9945 | 1.0387 | 0.9968 | miss | pass |
| sparse64 / exact / full | 0.9995 | 0.9340 | 1.0814 | pass | miss |
| sparse64 / exact / tile | 1.0028 | 0.9956 | 1.0560 | miss | miss |
| sparse64 / exact / tiled | 0.9959 | 1.0132 | 0.8756 | miss | pass |
| tiny / competitive / cold | 1.0315 | 0.9976 | 0.9954 | miss | pass |
| tiny / competitive / full | 1.0062 | 0.9937 | 0.9900 | miss | pass |
| tiny / exact / cold | 1.0046 | 1.0870 | 1.0143 | miss | miss |
| tiny / exact / full | 0.9957 | 0.9927 | 0.9099 | miss | pass |

## Provenance and limits

[Complete audit and evidence hashes](n16-output-guarded-experiment.json), [frozen plan](n16-output-guarded-plan.json), [source/binary bindings](n16-output-guarded-bindings.json). The baseline is the exact previously built shared-composition binary whose product fingerprint matches `4dfbaea9`; it was not independently rebuilt. Candidate sources were measured dirty on `4097d66e`; the bindings preserve the complete product fingerprint and binary hash for the later commit.

The auditor independently rehashes and checks pixel artifacts, source identity, orders, stage timing fields, latency point estimates and sampled RSS. Bootstrap bounds use the pinned implementation and are not independently recomputed. This synthetic matrix is neither a release-runner attestation nor a competitor or holdout result. Historical percentages are not added together.
