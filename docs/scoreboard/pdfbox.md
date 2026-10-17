# Scoreboard: pdfbox

Recorded 2026-10-17T21:28:10+00:00 at commit `0451af7`; binary SHA-256 `58eba42660d3bda1…`;
corpus digest `119dbf20058793df…` over 175 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened, pages enumerated | 165 / 174 (94.8%) |
| … strictly, every scanned page compiling | 148 |
| … only with repairs | 17 |
| Failed to open | 9 |
| Opened, but the page tree failed | 0 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 76 / 307 (24.8%) |

| Open failure | Count |
|---|---:|
| `password` | 4 |
| `security.Adobe-PubSec` | 4 |
| `syntax` | 1 |

| Page failure | Count |
|---|---:|
| `font.non-embedded` | 125 |
| `syntax` | 64 |
| `annotation.appearance` | 28 |
| `pattern.colorspace` | 5 |
| `filter.JPXDecode` | 3 |
| `font.cmap-program` | 2 |
| `transparency.group-colorspace` | 2 |
| `extgstate.unknown` | 1 |
| `font.predefined-cmap` | 1 |

| Repair | Count |
|---|---:|
| `filter.trailing-data` | 28 |
| `xref.rebuilt` | 2 |
| `font.stream-lengths` | 1 |
| `syntax.duplicate-key` | 1 |
| `file.trailing-bytes` | 1 |
| `pages.media-box` | 1 |
| `catalog.type-missing` | 1 |
| `filter.truncated` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfbox.json` (not committed).
