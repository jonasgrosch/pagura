# Glyph-hull memo on the retained combined stack

Status: **useful partial win retained experimentally; full validation passed;
no performance admission**.
The control is `776fd4b9`, including the measured conditional-viewport clip
change and the earlier image/text/stroke stack. This experiment adds the
bounded root-run hull memo only. The next parentless-clip idea is an unapplied
patch and is excluded from both binaries.

The memo reuses the existing finite device hull for the first 64 root command
ordinals, only for multi-source glyph runs at the canonical device transform
and when internal regions repeat. It preserves traversal, arithmetic, required
work charges and error ordering. Default-knockout and other nested groups do
not use it. Opaque Exact text can bypass the compositor entirely.

The current native layout occupies **2,064 bytes per Context**, including when
disabled, with alignment 8. That fixed stack cost is separate from process RSS
and is not a decoded-storage reservation or process memory cap.

## Frozen comparison

The [plan](glyph-memo-plan.json) declares 34 cells across Exact and Competitive:
the original sparse/dense/tiny/opaque/single-source/overcapacity/nested/default-
knockout cases, cold/first-tile/assembled-tile controls, two-source L/T outlines,
late text after 64 paths, and clip/image-only controls. Only `sparse64/full` in
each mode is a target. The independent L/T pixel derivation uses disjoint
integer rectangle partitions and separately checks opaque alpha.

The [bindings](glyph-memo-bindings.json) freeze source, binaries, harnesses and
policy. The control binary was reused only after all 497 retained Cargo/crates
build-input hashes matched the new control checkout. The candidate release
build passed with one Cargo job and an unchanged-source receipt. A rejected
build invocation used an unsupported wrapper kind and launched no build; its
preflight log is retained separately.

The pilot uses five measured pairs and five warmups; confirmation uses 200
fresh pairs and five warmups per cell. Every output must match independently
derived RGB exactly, have alpha 255, and equal the same-mode control bytes.
Cold means open/compile/render with a warmed filesystem, excluding output
emission. This is not a persistent-process or representative-corpus benchmark.

Original median, p95 and peak-RSS gates remain unchanged. A partial win can be
retained experimentally when at least one designated target clears its frozen
median upper bound and correctness/work/error checks pass. Failed protected,
tail and memory gates remain visible and prevent full admission. Pilot or
historical samples cannot be pooled into confirmation; no rerun-until-pass rule
is allowed.

## Preflight and pilot

All 13 focused memo tests passed, including actual Competitive fragment glyphs
with warmed memo/mask caches, visible and off-tile regions, every work-budget
boundary, partial output and positioned errors. A separately retained temporary
diagnostic confirmed the new fixtures compile to the intended root/source/group
and ordinal structures. It was removed before the product build.

The pilot verified **680 outputs** and observed no competing jobs. Its target
median ratios were 0.8286 in Exact and 0.9169 in Competitive. These are exploratory
values, not accepted gains. Tail and memory results were uneven; notably one
protected knockout cell had an RSS ratio of about 1.85. That observation must
remain visible and cannot be dismissed from timing results alone.

## Fixed confirmation

All 34 cells completed with 200 fresh measured pairs and five warmup pairs each.
Reverification checked **13,940 outputs** against independently regenerated
RGB, opaque alpha and same-mode bytes, including warmups. Nine distinct raw
output blobs were rehashed. Source, binaries, harness and policy remained bound.
The observer completed 261 process scans over 1,353.49 seconds without finding
competing jobs. This sampled local observation is not a controlled-release-runner
attestation or proof that all background activity was absent.

| Full-render target | Control median | Memo median | Median ratio | Median upper | p95 upper | Peak RSS, control → memo |
|---|---:|---:|---:|---:|---:|---:|
| Exact sparse64 | 8.479 ms | 6.981 ms | 0.8233 | 0.8305 | 0.9913 | 32.000 → 29.859 MiB |
| Competitive sparse64 | 1.595 ms | 1.451 ms | 0.9096 | 0.9187 | 0.9475 | 21.141 → 21.047 MiB |

Both targets pass their frozen median, p95 and raw peak-RSS gates. Their median
reductions are **17.7%** and **9.0%**, incremental to the combined stack plus the
retained clip change. These are this workload's results, not whole-engine or
competitor speedups. Do not add or multiply historical percentages.

The full matrix still fails admission: only 7 of 32 protected cells pass both
latency bounds, and 19 of 34 cells pass the raw maximum-RSS rule. Some misses
are inconclusive upper bounds; others contain positive regression evidence:

- Exact image-only median rises 4.264 → 4.684 ms (**9.9%**); its p95 lower ratio
  is 1.0347, above the protected 1.02 limit.
- Tiny Competitive cold median rises 0.230 → 0.241 ms (**4.4%**); its median
  lower ratio is 1.0261, above 1.02. Both claims use the declared marginal
  intervals, without a family-wide multiple-comparison guarantee.
- The more ordinary two-source L/T control shows essentially no Exact median
  gain (ratio 1.0000). Competitive's ratio is 0.9862 with upper 1.0002; it does
  not establish a reliable gain. The favorable 64-source target must not be
  generalized to typical text.
- Clip-churn medians remain near control (Exact ratio 1.0023, Competitive
  1.0007). Tail bounds still miss; this does not constitute full non-regression.

The pilot's Exact outline-knockout RSS spike remains part of the record:
9.172 → 16.938 MiB (ratio 1.8467). Fresh confirmation maxima are
31.922 → 29.297 MiB (0.9178), while its median RSS rises 9.078 → 9.109 MiB.
This does not establish a causal explanation for either peak. Other protected
RSS ratios still regress, including nested Exact 1.0831 and outline-knockout
Competitive 1.0708. Observed lower target peaks do not erase the known 2,064-byte
fixed memo cost. Descriptive RSS distributions are retained without changing
the frozen maximum-RSS rule.

## Decision and next step

Keep the memo on this **experimental** branch under the predeclared partial-win
rule: both designated targets clear their gain bound and independent correctness
and work/error tests pass. The supported protected regressions remain real
follow-up obligations. No production/default performance admission is granted.
This one confirmation is complete; no rerun-until-pass or sample pooling is
authorized by its result. A revision needs a separately frozen experiment.

The [machine-readable record](glyph-memo-experiment.json) preserves every pilot
and confirmation cell, execution/artifact hashes and validation status.
The complete integrated `python3 tools/check.py --jobs 1` check passed in
772.15 seconds: policy, fixture/data verification, formatting, Clippy, workspace
and doc tests, documentation, native/WASM builds, host checks, 196 Python tests
and all 1,237 generated-PDF conformance cases. The completed receipt records
unchanged source `0106c6da903589edfc3cba638702176d252cab80bef1582954895dfe8a894b12`;
148 observer scans found no competing jobs. Its first two
attempts stopped on a fixed-chunk-iteration Clippy warning and the array
dereference needed by that test-only cleanup. Both failed attempts are retained.
[Post-measurement bindings](glyph-memo-post-measurement-bindings.json) prove that
substituting the original test bytes reconstructs both measured source hashes
exactly; runtime product code and both measured binaries remain unchanged.
The next experiment
should investigate disabled-path regressions while preserving the primary gain;
the separate parentless-clip proposal remains unapplied. The
[standing goal](../../docs/plans/renderer-standing-goal.md) continues through
independent accuracy, memory and fair competitor release requirements.
