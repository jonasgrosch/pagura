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

- **M7 foundations, alongside performance work:** caller-owned transactions,
  append-only save and filled-form persistence (ADR 0031). Work and review use
  an isolated branch; tests run on hosted runners. Full rewrite, sanitizing
  redaction, encryption-on-write and general page-content editing remain later M7 work.
  Inline form controls support filling, preview and PDF download.

- **Specification verification:** every requirement registry is being checked
  against the primary standards (ISO 32000-2 and the format standards in the
  private reference cache); discrepancies become fixtures with independent
  expectations. A registry whose `scope` does not state this is unverified.
- **Rendering speed without spending accuracy:** one compositor with f32
  page and group buffers under ADR 0004's accuracy contract (every pixel
  within one level of its exact result), opaque text-object groups elided,
  device geometry prepared once. Speed comes from the rasterizer and the
  compositor, not from glyph-position snapping (ADR 0013).
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
- Multithreaded tile rendering (native threads, `wasm32-wasip1-threads`).
- Linearization and incremental loading for large files.

Deferred capabilities remain release blockers wherever the performance or
feature contract requires them. Exclusions are explicit product decisions,
never post-hoc benchmark filtering.
