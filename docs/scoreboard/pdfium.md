# Scoreboard: pdfium

Recorded 2026-10-17T21:28:05+00:00 at commit `0451af7`; binary SHA-256 `58eba42660d3bda1…`;
corpus digest `118d5cae2cc0a461…` over 343 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 7 |
| PDF files opened, pages enumerated | 305 / 336 (90.8%) |
| … strictly, every scanned page compiling | 185 |
| … only with repairs | 120 |
| Failed to open | 29 |
| Opened, but the page tree failed | 2 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 148 / 379 (39.1%) |

| Open failure | Count |
|---|---:|
| `syntax` | 22 |
| `password` | 6 |
| `limit` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 1 |
| `cycle` | 1 |

| Page failure | Count |
|---|---:|
| `font.non-embedded` | 126 |
| `syntax` | 51 |
| `annotation.appearance` | 32 |
| `filter.unknown` | 5 |
| `pattern.tiling` | 5 |
| `annotation.flags` | 3 |
| `limit` | 2 |
| `missing-object` | 2 |
| `transparency.group-colorspace` | 2 |
| `font.predefined-cmap` | 2 |
| `shading.mesh` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 48 |
| `stream.length-corrected` | 46 |
| `filter.truncated` | 36 |
| `xref.offset-corrected` | 24 |
| `object.end-missing` | 15 |
| `pages.media-box` | 13 |
| `pages.count-mismatch` | 12 |
| `pages.type-missing` | 8 |
| `pages.repeated-node` | 7 |
| `pages.parent-mismatch` | 7 |
| `catalog.type-missing` | 4 |
| `syntax.key-solidus` | 4 |
| `font.stream-lengths` | 3 |
| `trailer.reconstructed` | 3 |
| `filter.trailing-data` | 2 |
| `syntax.dictionary-opener` | 2 |
| `syntax.duplicate-key` | 2 |
| `filter.raw-deflate` | 2 |
| `pages.type-invalid` | 1 |
| `xref.stream-boundary` | 1 |
| `pages.crop-box` | 1 |
| `syntax.direct-stream` | 1 |
| `syntax.reference-zero` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfium.json` (not committed).
