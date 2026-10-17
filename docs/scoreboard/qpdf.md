# Scoreboard: qpdf

Recorded 2026-10-17T21:28:01+00:00 at commit `0451af7`; binary SHA-256 `58eba42660d3bda1…`;
corpus digest `580f302df3467dc9…` over 717 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 16 |
| PDF files opened, pages enumerated | 671 / 701 (95.7%) |
| … strictly, every scanned page compiling | 582 |
| … only with repairs | 89 |
| Failed to open | 29 |
| Opened, but the page tree failed | 1 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 131 / 3817 (3.4%) |

| Open failure | Count |
|---|---:|
| `password` | 19 |
| `syntax` | 9 |
| `limit` | 1 |

| Page-tree failure | Count |
|---|---:|
| `limit` | 1 |

| Page failure | Count |
|---|---:|
| `font.non-embedded` | 3524 |
| `syntax` | 147 |
| `annotation.flags` | 4 |
| `missing-object` | 2 |
| `annotation.appearance` | 2 |
| `predictor.dimensions` | 2 |
| `limit` | 2 |
| `cycle` | 1 |
| `transparency.group` | 1 |
| `filter.unknown` | 1 |

| Repair | Count |
|---|---:|
| `filter.trailing-data` | 274 |
| `xref.rebuilt` | 52 |
| `object.end-missing` | 16 |
| `trailer.reconstructed` | 14 |
| `xref.stream-boundary` | 12 |
| `syntax.name-escape` | 11 |
| `security.hash-padding` | 4 |
| `stream.length-corrected` | 3 |
| `syntax.duplicate-key` | 3 |
| `syntax.key-solidus` | 3 |
| `pages.repeated-node` | 3 |
| `pages.media-box` | 3 |
| `object.stream-self-entry` | 2 |
| `pages.parent-mismatch` | 2 |
| `filter.raw-deflate` | 2 |
| `filter.truncated` | 2 |
| `pages.type-missing` | 2 |
| `security.key-length` | 1 |
| `xref.offset-corrected` | 1 |
| `object.stream-type` | 1 |
| `catalog.type-invalid` | 1 |
| `pages.direct-kid` | 1 |
| `security.permissions-unsigned` | 1 |
| `file.header-format` | 1 |
| `syntax.reference-zero` | 1 |
| `catalog.object-number` | 1 |
| `file.header-offset` | 1 |
| `pages.count-mismatch` | 1 |
| `pages.cycle` | 1 |
| `stream.eol` | 1 |

Raw report: `benchmarks/results/local/scoreboard-qpdf.json` (not committed).
