# Roadmap

Work is organized as vertical milestones measured on pinned corpora
(ADR 0007). A milestone is done when its metric is met, `python3
tools/check.py` passes, and `docs/STATUS.md` records the evidence.

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

- **Performance on the one-pass architecture (ADR 0034):** one interpreter,
  a recorded scene per page, preparation that validates and reserves in
  source order, the band rasterizer and a band executor with per-buffer
  storage formats are in place, held to ADR 0004's one-level contract by the
  reference harness. The next goal is ADR 0004's absolute goal table for the
  four fast parameters together, then exact rendering towards the same
  table, then the 0.80 target of ADR 0002, judged on the scoreboard harness
  in both policies. The levers in order of evidence: band cell coverage
  (setup cost per mark, interior spans), non-overlapping stroke pieces,
  prepared paint (span evaluators, column reuse), the executor on opaque
  pages and band height, images decoded at size and region, interpretation
  (numeric fast path, operator dispatch, scene reuse), glyph edge setup, and
  occlusion culling. Byte storage promoted to N16 per 16×16 chunk is an
  experiment that needs ADR 0004's proof first. WebAssembly SIMD and scalar
  costs are measured for every kept lever.
- **M7 foundations:** caller-owned transactions, append-only save and
  filled-form persistence (ADR 0031). Work and review use an isolated branch;
  tests run on hosted runners. Full rewrite, sanitizing redaction,
  encryption-on-write and general page-content editing remain later M7 work.
  Inline form controls support filling, preview and PDF download.
- **Specification verification:** every requirement registry is being checked
  against the primary standards (ISO 32000-2 and the format standards in the
  private reference cache); discrepancies become fixtures with independent
  expectations. A registry whose `scope` does not state this is unverified.
- **M3/M5 gaps on real files:** vertical writing with substituted CJK fonts,
  Type 1 programs that need PostScript, non-RGB transparency group colour
  spaces, overprint, and transfer functions under transparency.
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
- PNG output (Flate compressor, PNG encoder) for the conversion use case.
- **Loading:** a `ByteSource` whose reads may return `Unavailable` naming
  every missing range; page prefetch that walks a page's resource graph in
  one round; tail-first opening of non-linearized files and linearization
  hints; objects as shared handles the `Store` can evict; revisions as a
  parent plus an overlay (an ADR 0012 amendment).
- **Host and viewer:** protocol v2 replacing versions 1.0–1.7; rendering in
  band steps, cancellable between bands; a JSPI transport so cross-origin
  isolation becomes optional, with the Atomics path as fallback (an ADR 0005
  amendment); low-resolution previews and stale tiles while zooming; a worker
  pool fed with serialized prepared scenes; ARIA from the logical structure
  view; a size budget for the core module.
- **More devices** on the interpreter's `Device` trait: SVG output, PDF
  re-emission through the operator layer, hit testing.
- **Concurrency:** native band threads (`std::thread::scope`) and WebAssembly
  threads under cross-origin isolation, on `PreparedScene` bands.
- **Editing additions to M7:** sessions on snapshots with undo and redo;
  semantic operations with XFDF; content filters on `pagura-content`;
  redaction with a residual scan; one DocMDP/FieldMDP change classifier;
  two-phase external signing (signature creation needs its own decision; ADR
  0015 is verification only); a native AcroForm AF function library, with no
  JavaScript engine.
- **Crate splits:** `pagura-interactive` (fields, navigation, actions) and
  `pagura-sign`.

Deferred capabilities remain release blockers wherever the performance or
feature contract requires them. Exclusions are explicit product decisions,
never post-hoc benchmark filtering.
