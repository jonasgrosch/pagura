### Independent validation

The completed speed/memory and retention decision is in the separate
[confirmation result](n16-painted-interval-experiment.md).

The painted-interval change preserves the tested N16 output, resource and error
contracts in both accuracy modes. The evidence below is complete; it does not
include or anticipate the recovery-confirmation result.

- **Authoritative checks:** all 14 ordered repository checks passed against one
  frozen candidate source/configuration. This was a complete staged check, not
  one uninterrupted `check.py` invocation. Each stage retains its own command,
  log, final receipt and observer record; interrupted and failed attempts remain
  in the evidence ledger. [Staged aggregate](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/staged-check-001/aggregate.json).
- **Native and WASI regression tests:** all six arm/platform combinations passed:
  baseline 231 and candidate 238 renderer library tests on native, WASI SIMD and
  WASI scalar, including the two temporary diagnostic tests. The additional
  candidate tests cover validated path/fragment interval updates, partial
  failures, clipped origins, untouched holes, irregular full/tile partitions,
  colored backgrounds and painted→blank→painted storage reuse.
  [Six-arm report](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/diagnostics-006/report.json).
- **Resource/error equivalence:** all 47 ordered diagnostic records matched
  baseline versus candidate on each platform. These include pixel/decoded/work
  admission boundaries, function/command/depth work and off-page/tile/nested
  source errors with their positions and diagnostics. Output skipping neither
  removes full N16 initialization nor changes the `count*8` pixel allocation
  charge. This checks the selected boundaries; it is not an exhaustive error
  equivalence proof for every PDF.
- **Actual layouts:** size/alignment did not grow for Buffer, Option<Buffer>,
  Surface or MaskSink. Native sizes are 80/80/176/56 bytes, all alignment 8;
  WASI SIMD and scalar sizes are 40/40/120/40, alignments 4/4/8/4. Context also
  remained unchanged. The probe adds functions, not layout-affecting fields or
  cfg alternatives to those types. Unchanged layouts do not establish unchanged
  process RSS or generated stack frames. [Quality assembly](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/quality-002.json).
- **Independent rational precision:** the default Exact native and Node-WASI
  host returned identical raw responses, empty diagnostics and opaque alpha.
  The two Normal source-over references are `44617/204` (allowed byte 218–219,
  observed 219) and `255*(254/255)^1000` (allowed 5–6, observed 5). These are
  references for the declared coverage bytes, not continuous geometric coverage.
  The host protocol does not exercise Competitive; both modes are exercised by
  the separate native/WASI regression tests. [Host report](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/precision-host-001/report.json).
- **Route reach, with the original miss retained:** R1's six-row Competitive
  opaque64 prediction failed although its independent RGBA check passed. Source
  analysis identified a submitted zero-coverage phase row. Separately frozen R2
  passed 24 native full-render requests, including three new glyph placement/
  clipping controls in both modes. Per opaque64 chunk, Exact converts 1536 pixels
  in 6 calls; Competitive converts 1792 in 7, with 1536 positive samples and an
  observed zero halo. No pixel tolerance or benchmark threshold changed.
  [Failed R1](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/reach-001/report.json),
  [R2 report](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/reach-r2-001/report.json),
  [verified source restoration](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/reach-r2-preparation-001/removed.json).
- **Uninstrumented output checks:** all 104 native preflight outputs passed the
  fixed 52-cell matrix across both binary arms. The completed native pilot's 1040
  outputs and the WASI diagnostic's 336 outputs were independently verified
  against their fixed analytic references, including actual alpha; baseline
  pixels were not used as the oracle. [Preflight](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/native-preflight-002/report.json),
  [pilot audit](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/pilot-001-analysis.json),
  [WASI audit](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/wasi-001-analysis.json).

The native build identities are baseline `a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`
and candidate `069d62d75c1a9a2756b2833169c4c314d1c14cab`, with content digests
`b3a124d48e4761b66a717edb542ceebf56ffa91b0e3f702f997b005d1a30a2c7`
and `53062678753e6012a291b95dbf07cb8b069e7134ab836d587919975d1ed057c7`.
Their native executable SHA256 values are
`d63aaa060f4972052a3b338191e359a768d82f8e1162f1c323f30394a2f7ab97`
and `40da46dfa96b32d24c959943419f71786cf927c8d59def915954ec99a30cd3eb`.
Commit labels alone do not identify an overlay. Build/source identities and the
separately scoped runner digest are retained in the [frozen bindings](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/n16-painted-interval-bindings.json)
(SHA256 `1d1068bcfe81843728d29c1b5267d3207bfb3a3d6f03d3bdf1196f35641e9e6a`).
The [quality manifest](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/local/n16-painted-interval/quality-002-manifest.json)
pins the underlying reports, final receipts, observer records and restoration;
the [validation ledger](/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura/benchmarks/results/n16-painted-interval-validation.json)
retains earlier failed attempts rather than counting them as passes.

These results support this bounded optimization, not global PDF accuracy or a
competitor comparison. Native reach covers 24 full requests, not every timed tile
or WASI route. The instrumented route timings are unusable. The native pilot's
numeric gates did **not** all pass and cannot admit the change; successful pixel
checks do not erase those misses. The 336-output WASI run is a limited diagnostic,
not a calibrated speed or total-memory claim. Final retention remains subject
to the separately frozen performance, RSS, provenance and recovery audit.
