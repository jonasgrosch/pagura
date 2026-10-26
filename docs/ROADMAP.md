# Roadmap

Work is organized as vertical milestones measured on pinned corpora
(ADR 0007). A milestone is done when its metric is met, `python3
tools/check.py` passes, and `docs/STATUS.md` records the evidence.

The [competitive feature parity plan](COMPETITIVE_PARITY.md) is the consolidated
backlog against `render/one-pass` and its upcoming merge into `main`. It defines
23 packages, owners, dependencies and independent acceptance gates against the
versioned union of PDFium, MuPDF, pdf.js, Poppler, hayro and CoreGraphics. The
[capability inventory](STATUS.md) separates implemented features from missing
integration. Milestone numbers are outcomes, not completion claims.

## Milestones

| ID | Outcome | Metric |
|---|---|---|
| M1 | **Open the world.** Real-world files open (strictly or with reported repairs) and enumerate pages, natively and in WebAssembly. | ≥95% of the `realworld` corpus opens; every `generated` document case meets its expectation; no panic or timeout on the fuzz corpus; strict and repaired opens counted separately. |
| M2 | **Vector pages.** Pages using paths, device colours, clipping and basic graphics state render with exact-area antialiasing, as tiles, in the browser viewer. | Analytic raster oracles pass; scoreboard reports pages rendered and the tag histogram of the rest; conversion benchmark runs on real documents. |
| M3 | **Text.** Embedded and standard fonts: Type 1/CFF, TrueType, Type 3, CID-keyed fonts with CMaps, vertical text, text render modes. | Glyph-outline oracles; text pages render; tag histogram shows no font gaps on the corpus. |
| M4 | **Images.** DCT, JPX, JBIG2, CCITT, inline images, masks, decode arrays, indexed and 16-bit samples. | Codec vectors; image pages render; decoder output caps proven by tests. |
| M5 | **Colour and transparency.** ICC, Separation/DeviceN, Lab, shadings, patterns, transparency groups, soft masks, blend modes, overprint simulation. | Ghent and PDF Differences cases investigated with reviewed expectations. |
| M6 | **Interactive viewer.** Annotation appearances, form display, outlines and links, text extraction, search, selection, accessibility, signature validation (ADR 0015). | Structure-aware cases; viewer usability checks; signature results kept separate (integrity, revision coverage, permitted changes, signer trust, timestamps, revocation) against CAVP, Wycheproof and PKITS vectors. |
| M7 | **Editing.** Transactions, incremental save, full rewrite, sanitizing redaction, signature-aware save, encryption on write. | Untouched-byte preservation; cross-viewer reopen; hidden-content scans. |
| M8 | **Release.** PDF/A and PDF/UA validation, controlled benchmark runners, the ADR 0002 gate. | Every required comparison cell ≤ 0.80; no memory regression. |

## Current focus

- **Establish competitive coverage (G00/G01):** inventory actual baseline APIs
  and builds, reconcile live unsupported paths and stale requirement summaries,
  add operation adapters and refresh the pinned corpus evidence. Historical
  scoreboards do not establish current-head parity.
- **Performance on the one-pass architecture (ADR 0034):** one interpreter,
  a recorded scene per page, preparation that validates and reserves in
  source order, the band rasterizer and a band executor with per-buffer
  storage formats are in place, held to ADR 0004's one-level contract by the
  reference harness. The next goal is exact rendering faster than hayro in
  every category (text, vector, mixed, images), then the 0.80 target of ADR
  0002, judged on the scoreboard harness (C02). Profile the remaining coverage,
  preparation, execution and output costs on the current source; earlier
  experiments are not a list of still-missing features. Every retained change
  needs independent accuracy evidence and native/WASI SIMD/scalar measurements.
  Exact is the sole rendering mode; the former Competitive path is removed.
- **M7 foundations are implemented:** caller-owned transactions, append-only
  save, filled-form persistence and browser filling/preview/download (ADR 0031).
  Full rewrite, sanitizing redaction, encryption-on-write, undo/redo and general
  editing remain E01–E03. Structural DocMDP/FieldMDP analysis exists; save
  enforcement and complete signature trust validation remain S01. Preserve
  isolated changes and ADR 0031's hosted-validation constraints.
- **Specification verification:** every requirement registry is being checked
  against the primary standards (ISO 32000-2 and the format standards in the
  private reference cache); discrepancies become fixtures with independent
  expectations. A registry whose `scope` does not state this is unverified.
- **M3/M4 gaps (R02–R04):** vertical substituted CJK fonts and executable font
  programs; higher/mixed-precision JPX sYCC, Lab-input/multi-profile ICC fallback and Indexed container-palette opacity integration; residual
  codec variants. Expert CFF charsets, FDSelect 4, default CFF2 blends and JPX
  embedded alpha already exist and are not scheduled for reimplementation.
- **M5 gaps (R05/R06):** inverse/destination ICC transforms, calibrated blending
  destinations and remaining transfer/graphics combinations. Gray/RGB/CMYK
  groups, knockout and process/spot overprint display simulation already exist.
- **M6 integration:** annotation/form appearances (V01/V02), tagged
  accessibility (V04), layers/printing (V05) and PDF integration of existing PKI
  path/CRL validation (S01). XFA/scripts and signature creation need explicit
  scope ADRs (V03/S02); an intentional exclusion remains a competitive difference.
- **M1:** files whose catalog opens but whose page tree fails, and the
  remaining open failures in other projects' bug-reproduction suites.
- **Robustness:** a short, routine coverage-guided fuzz run with every finding
  replayed as a regression test on stable Rust.
- **Benchmarks:** the conversion track on real documents against PDFium,
  MuPDF, pdf.js, Poppler, hayro and CoreGraphics.

## Later

- **Grow the real-world corpus when the current one is perfect.** Trigger:
  every file of the pinned `realworld` corpus (SafeDocs CC-MAIN-2021-31 zip
  `0000`) opens and every page renders or fails only with tags the current
  milestone does not own. Then add zips `0001`–`0009` (about 11 GB, from the
  same Digital Corpora location) to the lock, which narrows the scoreboard's
  95% confidence interval from about ±1.4 to ±0.45 points; consider an EC2
  runner for that size.
- Long coverage-guided fuzz campaigns (the `fuzz/` targets) on dedicated hardware.
- **Loading, image extraction and output (D01/D02a):** the
  [consolidated detailed design](plans/async-pdf-loading-and-image-streaming.md)
  and [proposed ADR 0035](adr/0035-progressive-sources-and-streaming-jobs.md)
  cover immutable range sources, missing-data jobs, tail-first/linearized
  loading, image assets, resumable bands, incremental PNG/DEFLATE and native
  async/browser integration. Band callbacks, stop callbacks, CLI streamed PPM
  and whole-buffer PNG/zlib encoders already exist. P0–P9 subdivide these parity
  packages; they are not a duplicate roadmap. Parsed-object eviction and
  parent/overlay revisions remain separate ADR 0012 follow-ups.
- **Host and viewer:** protocol v2 replacing versions 1.0–1.7; rendering in
  persistent band steps and host cancellation using the existing band driver;
  a JSPI transport so cross-origin
  isolation becomes optional, with the Atomics path as fallback (an ADR 0005
  amendment); low-resolution previews and stale tiles while zooming; a worker
  pool fed with serialized prepared scenes; ARIA from the logical structure
  view; a size budget for the core module.
- **Preview rendering:** a rough profile for thumbnails and progressive
  first paint (greeking small text, images decoded at drawn size or as their
  average colour, coarse coverage), built only after exact rendering beats
  hayro. This remains a proposal, separately named and measured; it does not
  restore the retired Competitive mode or weaken exact rendering's contract.
- **More devices** on the interpreter's `Device` trait: SVG output, PDF
  re-emission through the operator layer, hit testing.
- **Concurrency:** native band threads (`std::thread::scope`) and WebAssembly
  threads under cross-origin isolation, on `PreparedScene` bands.
- **Editing additions to M7:** sessions on snapshots with undo and redo;
  semantic operations with XFDF; content filters on `pagura-content`;
  redaction with a residual scan (E01–E03); connect the existing DocMDP/FieldMDP
  structural classifier to save policy and trust evidence (S01);
  two-phase external signing (signature creation needs its own decision; ADR
  0015 is verification only); a native AcroForm AF function library, with no
  JavaScript engine. Broader script parity remains V03's explicit ADR decision.
- **Document services and profiles:** metadata validation/writes, typed attachments/portfolios
  and export APIs (D02), public-key security and encrypted history (D03), and
  version/profile validation (C01). Recognizing a version or PDF/X declaration
  does not establish conformance.
- **Crate splits:** `pagura-interactive` (fields, navigation, actions) and
  `pagura-sign`.

Deferred capabilities remain release blockers wherever the performance or
feature contract requires them. Exclusions are explicit product decisions,
never post-hoc benchmark filtering.
