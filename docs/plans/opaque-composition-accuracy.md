# Plain opaque composition: corrected accuracy, remaining performance costs

Status, 2026-10-21: shared opaque composition landed in `4dfbaea9` on the
experimental stack. It removes the legacy `DisplayList::render_opaque`
traversal and routes eligible opaque marks through the existing bounded N16
compositor in both modes. The independent failures below are corrected, but
its original pilot measured substantial speed/RSS regressions. It remains an
experimental accuracy improvement, without performance or release admission.

The following retained revision also excludes nonopaque backgrounds from
narrow storage and simplifies exact N16 output conversion. Complete validation
and the fresh incremental performance experiment have finished. Keep these
stages and their source/binary bindings distinct.

## Independent composition bounds

The [original reproducer](../../benchmarks/results/opaque-precision-baseline.md)
and [landed correction evidence](../../benchmarks/results/opaque-composition-experiment.md)
retain the baseline failures, corrected outputs and validation records.

| Direct-root case | Unrounded answer in byte units | Allowed bytes | Earlier Exact / Competitive | Shared Exact / Competitive |
| --- | --- | --- | --- | --- |
| Gray 0.85, then two gray 0.95 marks at coverage byte 10 | `44617/204 = 218.710784...` | 218–219 | 217 / 219 | 219 / 219 |
| 1,000 black marks at coverage byte 1 over white | `255*(254/255)^1000 = 5.012754...` | 5–6 | 127 / 127 | 5 / 5 |

These bounds are against unrounded source-over recurrences on independently
specified coverage bytes. Tolerance one against rounded 219 would wrongly
admit 220; tolerance one against rounded 5 would wrongly admit 4. Keep the
strict intervals and existing general-compositor fixtures. Neither these
one-pixel cases nor agreement with Exact proves continuous-shape coverage,
complete PDF conformance, or a universal accuracy guarantee for Competitive.

## Implemented and validated

- Shared traversal retains statically admitted N16 storage in both modes;
  Competitive glyph fragments feed its narrow mask sink with mixed full,
  partial and zero coverage. N16 chunks remain bounded at 256 by 256 pixels,
  or 512 KiB of pixel scratch, in addition to output and other live storage.
- Bounded source-order validation before culling preserves required unsupported
  ICC/tint and stroke-geometry failures for plain opaque scenes. Its command,
  function and geometry work shares the execution ledgers. Transfer-bearing
  scenes use existing deferred device semantics and F64 exclusions. This is
  not a promise of identical legacy first-error selection or resource use.
- The current background regression independently checks straight colored
  RGBA `(200,100,50,a)` at alpha 0, 128, 254 and 255, with an empty scene, a
  half-pixel opaque black mark and a half-opacity full-pixel black mark, in both modes and full/tile entry points.
  Before the guard, an empty transparent background incorrectly emitted
  `(200,100,50,0)` instead of premultiplied `(0,0,0,0)`.
  [Failing reproduction](../../benchmarks/results/local/n16-output/background-reproducer-001.log)
  and [passing correction](../../benchmarks/results/local/n16-output/background-fixed-002.log)
  remain separate evidence.
- `render_policy` now selects F64 when background alpha is not 255, before
  context creation and Competitive byte-format selection. The existing wide
  output path composites onto premultiplied output. Opaque backgrounds retain
  their prior precision plan. This conservative exclusion applies to N16,
  F32 and U8; it does not claim those kernels now support arbitrary backdrops.
- The latest [full-check receipt](../../benchmarks/results/local/n16-output-guarded/full-check-001-receipt.json)
  completed with unchanged source and exit 0. Its
  [log](../../benchmarks/results/local/n16-output-guarded/full-check-001.log)
  records 210 renderer tests, 196 Python tests, native/WASI and host checks,
  and all 1,249 conformance cases passing. The separate
  [accuracy probe](../../benchmarks/results/local/n16-output-guarded/accuracy-001/report.json)
  verifies eight independent full/tile outputs in both modes. The
  [host probe](../../benchmarks/results/local/n16-output-guarded/host-001/report.json)
  verifies default Exact values 219 and 5, raw native/Node-WASI parity, strict
  open and empty diagnostics; the host protocol does not select Competitive.

## Measured costs and current experiment

The original 46-cell correction pilot against `0560506c` measured opaque64/full
median times of 1.827292 → 4.990542 ms in Exact (2.731×) and
1.362333 → 4.091333 ms in Competitive (3.003×). Zero of two target latency
gates, 5/44 protected latency gates and 11/46 peak-RSS gates passed. The
[complete ledger](../../benchmarks/results/opaque-composition-experiment.json)
remains unchanged. These are measured costs of the accuracy correction, not
an accepted speed optimization. The glyph memo and clip cleanup are present
in both arms; earlier gains cannot be added to these results.

Profiles identify N16 output conversion and surface preparation as candidates
for reducing cost; they do not establish causal percentages or attainable
speedups. The current exact flat quantizer replaces `(word+128)/257` with an
algebraically identical shift/subtract expression and removes channel packing.
Exhaustive u16 and offset/tail tests passed. It adds no storage. The separately
frozen [guarded experiment](../../benchmarks/results/n16-output-guarded-experiment.md)
verified 18,860 outputs and measured 12.5% lower Exact and 14.4% lower
Competitive target medians. Both target latency gates pass, but only 15/44
protected latency and 25/46 peak-RSS gates pass. Both target RSS gates fail,
and default-knockout Exact has a supported tail regression. Retain this partial
gain experimentally; full performance admission remains false. The original
[plan](../../benchmarks/results/n16-output-guarded-plan.json) and
[bindings](../../benchmarks/results/n16-output-guarded-bindings.json) are unchanged.

The earlier quantizer confirmation was stopped for the independently found
background defect, not to tune against timing results. Its ordinal is consumed;
[the stop record](../../benchmarks/results/local/n16-output/confirmation-stop.json)
and partial execution remain retained. Do not resume it, pool its samples,
or treat it as a completed confirmation. The completed guarded experiment is a
new correctness revision with fresh samples and unchanged acceptance gates.

Remaining costs include repeated source validation/conversion, chunk replay,
initialization and output traversal. The background correction uses existing
F64 accounting (56 bytes per pixel on this target, versus N16's eight), so
some low-memory cases may now return `Limit`; inaccurate narrow output is not
a fallback. Its nonopaque-background speed/RSS needs an independent,
equivalent-output matrix because the former background output is incorrect.
Any scratch reuse or painted-extent metadata remains a future measured
hypothesis, with bounded retained memory and explicit work/error behavior.

Keep accuracy-changing cases separate from baseline-byte-equivalent timings,
preserve all frozen thresholds and protected cells, and report incremental
results against the same stack. Even a quantizer win does not establish a net
win over the legacy route. The [standing goal](renderer-standing-goal.md) and
its independent accuracy, competitor and controlled-runner gates remain open.
