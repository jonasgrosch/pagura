# Pagura exploratory benchmark

Recorded: 2026-10-17T15:12:50.809538+00:00

**Release gate: BLOCKED.** These are bootstrap end-to-end process measurements,
including runtime startup, wrapper work, file I/O and PPM output. They are not
a comparison of warm rendering cores and do not establish general 20% superiority.

| Fixture | Competitor | Correct pairs | Pagura median ms | Baseline median ms | Ratio |
|---|---|---:|---:|---:|---:|
| blank | adobe | unavailable | — | — | — |
| blank | hayro | 10/10 | 3.197 | 3.486 | 0.917 |
| blank | mupdf | 10/10 | 3.871 | 89.472 | 0.043 |
| blank | pdfium | 10/10 | 3.983 | 64.855 | 0.061 |
| blank | pdfjs | 10/10 | 3.890 | 164.144 | 0.024 |
| blank | poppler | 10/10 | 3.611 | 22.085 | 0.163 |
| blank | quartz | 10/10 | 3.418 | 7.285 | 0.469 |
| clipped-to-page | adobe | unavailable | — | — | — |
| clipped-to-page | hayro | 10/10 | 3.691 | 3.708 | 0.995 |
| clipped-to-page | mupdf | 10/10 | 3.893 | 87.950 | 0.044 |
| clipped-to-page | pdfium | 10/10 | 3.956 | 66.167 | 0.060 |
| clipped-to-page | pdfjs | 10/10 | 5.222 | 183.668 | 0.028 |
| clipped-to-page | poppler | 10/10 | 5.356 | 32.201 | 0.166 |
| clipped-to-page | quartz | 10/10 | 3.401 | 7.964 | 0.427 |
| graphics-state | adobe | unavailable | — | — | — |
| graphics-state | hayro | 10/10 | 3.313 | 3.529 | 0.939 |
| graphics-state | mupdf | 10/10 | 3.992 | 88.196 | 0.045 |
| graphics-state | pdfium | 10/10 | 3.759 | 69.553 | 0.054 |
| graphics-state | pdfjs | 10/10 | 6.838 | 217.618 | 0.031 |
| graphics-state | poppler | 10/10 | 4.068 | 26.124 | 0.156 |
| graphics-state | quartz | 10/10 | 3.332 | 7.725 | 0.431 |
| inherited-box | adobe | unavailable | — | — | — |
| inherited-box | hayro | 10/10 | 3.349 | 3.673 | 0.912 |
| inherited-box | mupdf | 10/10 | 3.856 | 90.284 | 0.043 |
| inherited-box | pdfium | 10/10 | 3.858 | 64.384 | 0.060 |
| inherited-box | pdfjs | 10/10 | 6.510 | 200.879 | 0.032 |
| inherited-box | poppler | 10/10 | 3.555 | 21.960 | 0.162 |
| inherited-box | quartz | 10/10 | 3.471 | 7.706 | 0.450 |
| many-rectangles | adobe | unavailable | — | — | — |
| many-rectangles | hayro | 10/10 | 10.621 | 11.857 | 0.896 |
| many-rectangles | mupdf | 10/10 | 4.817 | 98.004 | 0.049 |
| many-rectangles | pdfium | 10/10 | 4.694 | 65.961 | 0.071 |
| many-rectangles | pdfjs | 10/10 | 4.599 | 181.020 | 0.025 |
| many-rectangles | poppler | 10/10 | 4.709 | 27.538 | 0.171 |
| many-rectangles | quartz | 10/10 | 4.412 | 33.186 | 0.133 |
| negative-rectangle | adobe | unavailable | — | — | — |
| negative-rectangle | hayro | 10/10 | 3.331 | 3.810 | 0.874 |
| negative-rectangle | mupdf | 10/10 | 4.820 | 117.918 | 0.041 |
| negative-rectangle | pdfium | 10/10 | 4.108 | 63.175 | 0.065 |
| negative-rectangle | pdfjs | 10/10 | 4.235 | 166.329 | 0.025 |
| negative-rectangle | poppler | 10/10 | 3.525 | 22.858 | 0.154 |
| negative-rectangle | quartz | 10/10 | 3.883 | 8.102 | 0.479 |
| rectangle | adobe | unavailable | — | — | — |
| rectangle | hayro | 10/10 | 3.225 | 3.406 | 0.947 |
| rectangle | mupdf | 10/10 | 4.093 | 85.960 | 0.048 |
| rectangle | pdfium | 10/10 | 4.087 | 63.834 | 0.064 |
| rectangle | pdfjs | 10/10 | 3.948 | 164.615 | 0.024 |
| rectangle | poppler | 10/10 | 3.583 | 22.165 | 0.162 |
| rectangle | quartz | 10/10 | 3.813 | 7.403 | 0.515 |

Gate blockers:

- comparison did not pass: blank / adobe
- comparison did not pass: blank / hayro
- comparison did not pass: blank / mupdf
- comparison did not pass: blank / pdfium
- comparison did not pass: blank / pdfjs
- comparison did not pass: blank / poppler
- comparison did not pass: blank / quartz
- comparison did not pass: clipped-to-page / adobe
- comparison did not pass: clipped-to-page / hayro
- comparison did not pass: clipped-to-page / mupdf
- comparison did not pass: clipped-to-page / pdfium
- comparison did not pass: clipped-to-page / pdfjs
- comparison did not pass: clipped-to-page / poppler
- comparison did not pass: clipped-to-page / quartz
- comparison did not pass: graphics-state / adobe
- comparison did not pass: graphics-state / hayro
- comparison did not pass: graphics-state / mupdf
- comparison did not pass: graphics-state / pdfium
- comparison did not pass: graphics-state / pdfjs
- comparison did not pass: graphics-state / poppler
- comparison did not pass: graphics-state / quartz
- comparison did not pass: inherited-box / adobe
- comparison did not pass: inherited-box / hayro
- comparison did not pass: inherited-box / mupdf
- comparison did not pass: inherited-box / pdfium
- comparison did not pass: inherited-box / pdfjs
- comparison did not pass: inherited-box / poppler
- comparison did not pass: inherited-box / quartz
- comparison did not pass: many-rectangles / adobe
- comparison did not pass: many-rectangles / hayro
- comparison did not pass: many-rectangles / mupdf
- comparison did not pass: many-rectangles / pdfium
- comparison did not pass: many-rectangles / pdfjs
- comparison did not pass: many-rectangles / poppler
- comparison did not pass: many-rectangles / quartz
- comparison did not pass: negative-rectangle / adobe
- comparison did not pass: negative-rectangle / hayro
- comparison did not pass: negative-rectangle / mupdf
- comparison did not pass: negative-rectangle / pdfium
- comparison did not pass: negative-rectangle / pdfjs
- comparison did not pass: negative-rectangle / poppler
- comparison did not pass: negative-rectangle / quartz
- comparison did not pass: rectangle / adobe
- comparison did not pass: rectangle / hayro
- comparison did not pass: rectangle / mupdf
- comparison did not pass: rectangle / pdfium
- comparison did not pass: rectangle / pdfjs
- comparison did not pass: rectangle / poppler
- comparison did not pass: rectangle / quartz
- controlled runner evidence missing
- cross-platform, resolution and concurrency aggregation is not implemented
- exploratory smoke evidence is not a release suite
- insufficient warmups
- missing 280 declared comparison cells
- missing engine identity: adobe
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
`benchmarks/results/bootstrap.json` in commit 36bd935, SHA-256 `83a2347141c7129e5010dbedbe69bcd1f9b730dcfe1f4b47f524da5e7cf5f12b`.
Raw reports are no longer committed (ADR 0007).
