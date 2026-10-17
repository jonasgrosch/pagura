# Scoreboard: pdfium

Recorded 2026-10-17T17:24:08+00:00 at commit `6d3c14c`; binary SHA-256 `36c4f1463031bd69…`;
corpus digest `118d5cae2cc0a461…` over 343 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 7 |
| PDF files opened | 294 / 336 (87.5%) |
| … strictly | 220 |
| … only with repairs | 74 |
| Failed to open | 42 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 114 / 327 (34.9%) |

| Open failure | Count |
|---|---:|
| `syntax` | 34 |
| `password` | 7 |
| `limit` | 1 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 108 |
| `syntax` | 36 |
| `annotation.appearance` | 25 |
| `image.xobject` | 23 |
| `transparency.group-colorspace` | 15 |
| `cycle` | 6 |
| `pattern.colorspace` | 5 |
| `filter.unknown` | 4 |
| `transparency.ca` | 4 |
| `missing-object` | 3 |
| `transparency.BM` | 1 |
| `extgstate.SM` | 1 |
| `shading.paint` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 44 |
| `filter.truncated` | 12 |
| `stream.length-corrected` | 9 |
| `xref.offset-corrected` | 9 |
| `pages.count-mismatch` | 4 |
| `syntax.duplicate-key` | 3 |
| `trailer.reconstructed` | 2 |
| `xref.stream-boundary` | 1 |
| `filter.raw-deflate` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfium.json` (not committed).
