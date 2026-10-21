# Shared opaque composition: validated accuracy correction, failed speed/RSS pilot

2026-10-21. Results for the experimental renderer stack; this is not production, release, or competitor admission. All paths below are relative to `benchmarks/results/`, unless stated otherwise. The reviewed implementation is the dirty candidate based on `0560506ca7c0529f3a3058a8244e0f04d044ef24`; that commit alone does not identify the change.

The shared compositor corrects the two independently derived direct-root composition failures. It also imposes substantial measured costs on ordinary opaque rendering. Preserve the validated accuracy correction as explicitly experimental work while revising those costs; do not describe it as an accepted speed optimization or an all-cell win. The existing glyph memo and clip cleanup remain in both comparison arms. No cumulative speed percentage follows from the earlier experiments.

## Independent accuracy and validation

| Direct-root fixture | Exact same-coverage-byte answer | Allowed RGB bytes | Earlier Exact / Competitive | Candidate Exact / Competitive |
| --- | --- | --- | --- | --- |
| Two partial gray marks | 44617/204 = 218.710784… | 218–219 | 217 / 219 | 219 / 219 |
| 1,000 black slivers | 255·(254/255)^1000 = 5.012754… | 5–6 | 127 / 127 | 5 / 5 |

Candidate release-driver results cover both full and tile entry points for both modes: all eight outputs meet the strict unrounded rational bounds, agree between entry points, and have alpha 255. These one-pixel fixtures establish composition accuracy for independently specified coverage bytes; they do not prove continuous-scene geometry accuracy or tile seams. A fresh probe of the frozen pilot baseline binary reconfirmed these earlier failures in all full/tile cells; see `baseline-accuracy-002` in the machine-readable results. Baseline pixels are not a correctness oracle. The separate accuracy-changing cases are not included in a baseline-byte-equivalent speed claim.

The strict host probe independently uses the newly generated corpus PDFs: default Exact produces 219 and 5 with exact raw native/Node-WASI response parity, successful strict open, and empty repair and typed diagnostic snapshots. This host protocol does not select Competitive; native Rust tests and the release driver cover that mode. The host and standalone long-stack PDFs have different encoded bytes, retained separately, with the same declared recurrence.

`full-check-001` completed on unchanged source with exit 0. Its log records the complete `tools/check.py --jobs 1` contract: policy; 20 fixture areas/13,513 files; 107 bundled-data files; formatting and clippy; workspace tests and docs; default and bundled native/WASI builds; classic host/browser checks; 196 Python tests; and **1,249/1,249 conformance cases**. The renderer suite has **208 passing tests**, including strict direct-root precision, off-region ICC/Type4 error context, stroke overflow, shared resource counters, transfer migration, mixed fragment masks, and tile partition checks. The four prior opaque-sliver Competitive allowances of 33–127 levels were removed. No complete-PDF conformance or universal one-level guarantee is inferred.

The full-check wrapper then built the release profile driver and bound its artifact. The separate accuracy wrapper bound both reports and the native/WASI host binaries. Their source fingerprints match before and after. Their process observers saw no known competing jobs in 151 and 3 scans respectively; this is observation with cooperative locks, not OS-enforced exclusivity.

## Pilot performance and memory

The predeclared pilot has 46 cells (2 targets, 44 protected), 5 measured pairs plus 5 warmup pairs per cell, seed 20261026. The evaluator rechecked **920 outputs**, including warmups, against independent fixture pixels and same-mode baseline bytes; it rehashed 13 distinct blobs. Frozen candidate/baseline ratios are lower-is-better. `cold` measures open+compile+render; other operations use retained-list render time. Five pairs cannot establish calibrated p95 or simultaneous confidence coverage. Reported paired-bootstrap bounds are retained pilot diagnostics, not a confirmation; the evaluator checks raw records and point ratios but does not independently recompute bootstrap bounds.

The frozen thresholds remain target median upper ≤0.95 and p95 upper ≤1.00; protected median and p95 uppers ≤1.02; maximum sampled candidate/baseline peak RSS ≤1.00 in every cell. **0/2 target latency gates, 5/44 protected latency gates, and 11/46 memory gates pass.** Thus 39 protected latency gates and 35 memory gates miss. Only `nested64/exact/full` and `sparse64/exact/tiled` meet all three numerical conditions; even these cannot be admitted from a pilot. The analysis correctly sets `admission: false` and `numeric_gates_pass: false`.

| Target: opaque64/full | Baseline → candidate median ms | Median ratio [lower, upper] | p95 upper | Peak RSS MiB baseline → candidate |
| --- | --- | --- | --- | --- |
| Exact | 1.827292 → 4.990542 | 2.731 [2.569, 2.843] | 2.738 | 20.140625 → 20.78125 |
| Competitive | 1.362333 → 4.091333 | 3.003 [2.904, 4.164] | 4.080 | 20.828125 → 21.375 |

The protected opaque controls also expose large costs: Exact/Competitive opaque cold medians are 2.350×/2.457× baseline; opaque-fill/full 2.598×/2.734×; Exact opaque-clip/full 2.114×; Exact opaque-tiny/cold 1.247×; Competitive opaque64/tile 1.448×. For these seven controls and both targets the reported median lower bound exceeds 1.02, so the pilot contains strong descriptive evidence of material regressions, not merely upper-bound misses. Other misses may be noisy or inconclusive and must not all be labeled proven regressions.

Peak RSS misses include large and small differences. The largest observed ratio is Competitive opaque64/tiled: **27.859375 → 39.140625 MiB (1.405×)**, despite its 0.935 point median latency ratio and very wide latency bounds. Exact opaque-clip rises 9.359375 → 11.4375 MiB (1.222×). Exact opaque64/tiled improves the point latency ratio to 0.678 but still misses memory (20.421875 → 21.03125 MiB). Some other RSS misses are only 16 KiB. These are maxima from five fresh-process samples per arm, not a causal attribution or a confidence interval; none may be silently waived under the frozen zero-growth rule.

The following complete ledger makes every numerical miss explicit. `M`, `P`, and `R` mean the median-upper, p95-upper, and peak-RSS threshold missed; `—` means no numerical miss. All rows are protected except the marked target row.

| Case / operation | Exact misses | Competitive misses |
| --- | --- | --- |
| sparse64 / cold | MP | MPR |
| sparse64 / full | MPR | MP |
| sparse64 / tile | MPR | MPR |
| sparse64 / tiled | — | MPR |
| dense64 / full | MPR | MPR |
| tiny / cold | M | MPR |
| tiny / full | MPR | MPR |
| opaque64 / full **target** | MPR | MPR |
| one-source64 / full | MPR | MPR |
| overcapacity128 / full | MP | MPR |
| nested64 / full | — | MPR |
| default-knockout64 / full | MPR | MP |
| outline2 / full | MR | MPR |
| outline2-knockout / full | MPR | M |
| outline2-late64 / full | MPR | MPR |
| clip-churn / full | MP | R |
| image-only / full | MP | MP |
| opaque64 / cold | MPR | MPR |
| opaque64 / tile | R | MPR |
| opaque64 / tiled | R | MPR |
| opaque-clip / full | MPR | MPR |
| opaque-tiny / cold | MPR | MPR |
| opaque-fill / full | MPR | MPR |

Pilot receipt `2e4da8ae-fa2d-44b2-9a34-60c7510a80df` completed with unchanged source and exit 0; 14 observer scans found no known contention. Short jobs between scans and ordinary desktop activity remain outside that guarantee. No confirmation report was present in the reviewed result directory. Do not spend the frozen confirmation on this known-regressing candidate merely to seek favorable bounds. Any changed candidate needs newly frozen bindings and an explicit new exploratory/confirmation protocol; retain this pilot unchanged.

## Profile and next change

Both serial Exact profiles completed 1,000 verified renders without observed known contention. Of 697 samples inside candidate render, 389 were in an inlined N16 output loop; disassembly identifies widening integer division and channel packing there. A further 52 were in page-surface preparation. The first revision will simplify exact channel conversion before adding scratch reuse or cache structures. The surrounding FNV checksum dominates whole-process samples, so these are diagnostic stack counts, not speed measurements or an estimated achievable percentage.

The [machine-readable results](opaque-composition-experiment.json) preserve every cell, accuracy reports, profile counts and evidence hashes. No confirmation was launched. A revised candidate gets separate frozen bindings and fresh samples; this failed pilot remains unchanged.

## Exact evidence pins

SHA-256 below was reread from the retained bytes during this review. The receipt bindings and analysis agree on the report/guard/plan/bindings hashes. The full-check log has a separately calculated hash; the runner receipt directly pins the built binary, not that log.

```text
opaque-composition-plan.json
  de0edb42fad216f37aac858d682dc8723e24d126f8ee7ad9635a83e4a1d420d3
opaque-composition-bindings.json
  4fdb3374d815469dc1ab0e43c4bce25bf9d1df42982dcdcb21a3b1de6e05a0e4
local/opaque-correction/pilot-001-analysis.json
  8ff987fa156aca75f966a8169a7d65c950854675ec52ac8ae48d6c7f2b210564
local/opaque-correction/pilot-001/report.json
  1692d2bed4cf5921348acc146f2ebeb4ea318a280cf3d507dd0e73381dc7f0bd
local/opaque-correction/pilot-001-receipt.json
  aae8c622faddabc0df3978186fbd6bfb483d8d01869df3ffe45449a02e3cd167
local/opaque-correction/pilot-001-observer/guard.json
  4995634414783b64fa2b2a51429532c7fb8006e6c51f295bfacfaa2ecea0dbcb
local/opaque-correction/full-check-001-receipt.json
  516435efc68e6f469702e17eff809e9c232d65bea12dcdf38ac3b2b9940c9c83
local/opaque-correction/full-check-001.log
  288f1aca34c5140287de0bdba2bbc30f42facd1dbb4887cd36d80b17afbad1f7
local/opaque-correction/accuracy-001-receipt.json
  c734cda7ae9bc06086f7fca15538e5c5ca670dc2fec92bdef305841b468dd545
local/opaque-correction/accuracy-001/report.json
  96dbdb40bda132986ebf44f7e7b4490e3c1e3b5c87a7f52b28c9105d68f5a8bd
local/opaque-correction/host-001/report.json
  01d4c28637b82cf3ffe9145bec3034002b8026aa6eb3086f65fa10802475934d
```

Source identities use different recorded scopes and must not be substituted for one another: paired-driver candidate `f72405c46866fb785d8fd050a931353f1438257b073561343a503583882423d2`; paired-driver baseline `f087987a0ab2dde87d2a6ffe432295e0f4dabdb0def1b1deef173263a90e8f6a`; runner candidate `ae8f6c41e4c1c86e1e79c0d4c52a8151ceaf984880452da3adf743b9ff2fe4cc`. Release profile binary pins are baseline `9257d268ab05bc0e0b2ba6b5b3a24281eefec89f058c8122ef926db9ffb86163` and candidate `bc32a8dbf58491718a47eea53bbb1bd9ef4f058ad906b5028e07b6da4f398354`. The authoritative control build is `control-build-002-receipt.json`; the earlier wrong-root receipt is retained and superseded as explained in the bindings.

Host report pins native `bf1c77044bb4ea4c110adc6ae0111b7b2cd8db925d307c15d5d7633d26c5d141` and WASI `23b50be936ef396068db081a78eb2e03641d8bb855489254f94022efe613a829`. These build and observed-source bindings are local coordinator provenance, not an independently reproducible rebuild or hostile-runner attestation.

