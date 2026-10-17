# Pagura exploratory benchmark

Recorded: 2026-10-17T15:14:05.462675+00:00

**Release gate: BLOCKED.** These are bootstrap end-to-end process measurements,
including runtime startup, wrapper work, file I/O and PPM output. They are not
a comparison of warm rendering cores and do not establish general 20% superiority.

| Fixture | Competitor | Correct pairs | Pagura median ms | Baseline median ms | Ratio |
|---|---|---:|---:|---:|---:|
| blank | hayro | 100/100 | 3.060 | 3.321 | 0.922 |
| clipped-to-page | hayro | 100/100 | 3.642 | 3.908 | 0.932 |
| graphics-state | hayro | 100/100 | 3.130 | 3.386 | 0.924 |
| inherited-box | hayro | 100/100 | 3.018 | 3.333 | 0.906 |
| many-rectangles | hayro | 100/100 | 4.376 | 5.173 | 0.846 |
| negative-rectangle | hayro | 100/100 | 3.082 | 3.404 | 0.905 |
| rectangle | hayro | 100/100 | 3.473 | 3.887 | 0.893 |

Gate blockers:

- comparison did not pass: blank / hayro
- comparison did not pass: clipped-to-page / hayro
- comparison did not pass: graphics-state / hayro
- comparison did not pass: inherited-box / hayro
- comparison did not pass: many-rectangles / hayro
- comparison did not pass: negative-rectangle / hayro
- comparison did not pass: rectangle / hayro
- controlled runner evidence missing
- cross-platform, resolution and concurrency aggregation is not implemented
- exploratory smoke evidence is not a release suite
- missing 322 declared comparison cells
- missing engine identity: adobe
- missing engine identity: mupdf
- missing engine identity: pdfium
- missing engine identity: pdfjs
- missing engine identity: poppler
- missing engine identity: quartz
- operation not benchmarked: edit-save
- operation not benchmarked: open-cold
- operation not benchmarked: render-cold
- operation not benchmarked: render-warm
- operation not benchmarked: save-noop
- operation not benchmarked: text-search
- operation not benchmarked: tile-zoom
- unbound release workload: annotations-acroforms
- unbound release workload: encrypted-and-signed-documents
- unbound release workload: icc-cmyk-spot-overprint
- unbound release workload: images-and-codecs
- unbound release workload: large-scanned-and-vector-documents
- unbound release workload: latin-and-cjk-fonts
- unbound release workload: malformed-bounded-recovery
- unbound release workload: paths-strokes-clipping-shading
- unbound release workload: tagged-accessible-documents
- unbound release workload: transparency-blend-knockout
- unbound release workload: xref-object-streams-incremental

The raw report (versions, artifact hashes, randomized order, peak RSS and every sample) is
`benchmarks/results/hayro-native.json` in commit 36bd935, SHA-256 `d4b21a46ad5a35267abc915931268599c5b9d7d2bf0f247e6f257011ab8953e0`.
Raw reports are no longer committed (ADR 0007).
