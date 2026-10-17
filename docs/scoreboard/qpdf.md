# Scoreboard: qpdf

Recorded 2026-10-17T18:26:45+00:00 at commit `4390401`; binary SHA-256 `5f8a67a475781cf9…`;
corpus digest `580f302df3467dc9…` over 717 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 16 |
| PDF files opened, pages enumerated | 654 / 701 (93.3%) |
| … strictly, every scanned page compiling | 590 |
| … only with repairs | 64 |
| Failed to open | 37 |
| Opened, but the page tree failed | 10 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 46 / 3706 (1.2%) |

| Open failure | Count |
|---|---:|
| `password` | 24 |
| `syntax` | 12 |
| `limit` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 5 |
| `cycle` | 4 |
| `limit` | 1 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 3360 |
| `annotation.appearance` | 143 |
| `image.xobject` | 120 |
| `syntax` | 20 |
| `image.inline` | 8 |
| `predictor.dimensions` | 2 |
| `transparency.BM` | 1 |
| `missing-object` | 1 |
| `cycle` | 1 |
| `transparency.group` | 1 |
| `transparency.SMask` | 1 |
| `operator.ID` | 1 |
| `filter.unknown` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 48 |
| `object.end-missing` | 28 |
| `trailer.reconstructed` | 17 |
| `xref.stream-boundary` | 12 |
| `syntax.duplicate-key` | 5 |
| `stream.length-corrected` | 3 |
| `filter.raw-deflate` | 2 |
| `filter.truncated` | 2 |
| `pages.type-missing` | 2 |
| `xref.offset-corrected` | 1 |
| `catalog.type-invalid` | 1 |
| `security.hash-padding` | 1 |
| `security.permissions-unsigned` | 1 |
| `file.header-format` | 1 |
| `file.header-offset` | 1 |
| `pages.count-mismatch` | 1 |
| `stream.eol` | 1 |

Raw report: `benchmarks/results/local/scoreboard-qpdf.json` (not committed).
