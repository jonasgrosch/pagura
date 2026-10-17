# Scoreboard: pdfium

Recorded 2026-10-17T18:45:00+00:00 at commit `21e75c4`; binary SHA-256 `16bbc42e2162f09c…`;
corpus digest `118d5cae2cc0a461…` over 343 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 7 |
| PDF files opened, pages enumerated | 281 / 336 (83.6%) |
| … strictly, every scanned page compiling | 207 |
| … only with repairs | 74 |
| Failed to open | 29 |
| Opened, but the page tree failed | 26 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 121 / 333 (36.3%) |

| Open failure | Count |
|---|---:|
| `syntax` | 21 |
| `password` | 7 |
| `limit` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 18 |
| `cycle` | 6 |
| `missing-object` | 2 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 131 |
| `image.xobject` | 29 |
| `annotation.appearance` | 26 |
| `syntax` | 8 |
| `pattern.colorspace` | 5 |
| `filter.unknown` | 4 |
| `transparency.ca` | 4 |
| `missing-object` | 2 |
| `transparency.BM` | 1 |
| `transparency.CA` | 1 |
| `shading.paint` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 43 |
| `stream.length-corrected` | 32 |
| `filter.truncated` | 13 |
| `object.end-missing` | 11 |
| `xref.offset-corrected` | 9 |
| `pages.count-mismatch` | 4 |
| `syntax.duplicate-key` | 3 |
| `trailer.reconstructed` | 3 |
| `filter.raw-deflate` | 2 |
| `xref.stream-boundary` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfium.json` (not committed).
