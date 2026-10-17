# Scoreboard: pdfbox

Recorded 2026-10-17T22:29:31+00:00 at commit `c38e654`; binary SHA-256 `40ac06f4a363b57c…`;
corpus digest `119dbf20058793df…` over 175 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened, pages enumerated | 165 / 174 (94.8%) |
| … strictly, every scanned page compiling | 138 |
| … only with repairs | 27 |
| Failed to open | 9 |
| Opened, but the page tree failed | 0 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 111 / 307 (36.2%) |

| Open failure | Count |
|---|---:|
| `password` | 4 |
| `security.Adobe-PubSec` | 4 |
| `syntax` | 1 |

| Page failure | Count |
|---|---:|
| `font.non-embedded` | 130 |
| `syntax` | 31 |
| `annotation.appearance` | 29 |
| `filter.JPXDecode` | 3 |
| `transparency.group-colorspace` | 2 |
| `font.predefined-cmap` | 1 |

| Repair | Count |
|---|---:|
| `filter.trailing-data` | 31 |
| `font.tounicode-range` | 23 |
| `font.table-alignment` | 12 |
| `font.tounicode-invalid` | 6 |
| `font.cid-to-gid-identity` | 5 |
| `font.tounicode-simple-codes` | 3 |
| `font.table-bounds` | 3 |
| `xref.rebuilt` | 2 |
| `font.cmap-codespace` | 1 |
| `font.stream-lengths` | 1 |
| `syntax.duplicate-key` | 1 |
| `file.trailing-bytes` | 1 |
| `pages.media-box` | 1 |
| `catalog.type-missing` | 1 |
| `filter.truncated` | 1 |
| `font.cmap-data` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfbox.json` (not committed).
