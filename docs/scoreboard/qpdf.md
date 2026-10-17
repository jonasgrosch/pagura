# Scoreboard: qpdf

Recorded 2026-10-17T17:24:09+00:00 at commit `6d3c14c`; binary SHA-256 `36c4f1463031bd69…`;
corpus digest `580f302df3467dc9…` over 717 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 16 |
| PDF files opened | 659 / 701 (94.0%) |
| … strictly | 588 |
| … only with repairs | 71 |
| Failed to open | 42 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 18 / 3691 (0.5%) |

| Open failure | Count |
|---|---:|
| `password` | 24 |
| `syntax` | 17 |
| `limit` | 1 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 3315 |
| `transparency.group-colorspace` | 128 |
| `image.xobject` | 120 |
| `annotation.appearance` | 73 |
| `syntax` | 30 |
| `missing-object` | 8 |
| `image.inline` | 8 |
| `cycle` | 4 |
| `predictor.dimensions` | 2 |
| `limit` | 1 |
| `transparency.group` | 1 |
| `transparency.SMask` | 1 |
| `operator.ID` | 1 |
| `filter.unknown` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 48 |
| `trailer.reconstructed` | 17 |
| `xref.stream-boundary` | 12 |
| `syntax.duplicate-key` | 5 |
| `filter.raw-deflate` | 2 |
| `filter.truncated` | 2 |
| `stream.length-corrected` | 1 |
| `xref.offset-corrected` | 1 |
| `security.hash-padding` | 1 |
| `security.permissions-unsigned` | 1 |
| `file.header-format` | 1 |
| `file.header-offset` | 1 |
| `pages.count-mismatch` | 1 |
| `stream.eol` | 1 |

Raw report: `benchmarks/results/local/scoreboard-qpdf.json` (not committed).
