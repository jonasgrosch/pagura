# Scoreboard: verapdf

Recorded 2026-10-17T22:28:16+00:00 at commit `c38e654`; binary SHA-256 `40ac06f4a363b57c…`;
corpus digest `58a0f31c0ab96e5e…` over 2906 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 0 |
| PDF files opened, pages enumerated | 2905 / 2906 (100.0%) |
| … strictly, every scanned page compiling | 2633 |
| … only with repairs | 272 |
| Failed to open | 1 |
| Opened, but the page tree failed | 0 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 12681 / 12948 (97.9%) |

| Open failure | Count |
|---|---:|
| `syntax` | 1 |

| Page failure | Count |
|---|---:|
| `syntax` | 113 |
| `annotation.appearance` | 59 |
| `transparency.group-colorspace` | 51 |
| `font.non-embedded` | 19 |
| `font.predefined-cmap` | 11 |
| `stream.external` | 3 |
| `xobject.subtype` | 2 |
| `transparency.BM` | 2 |
| `filter.unknown` | 2 |
| `pattern.tiling` | 2 |
| `limit` | 2 |
| `filter.JPXDecode` | 1 |

| Repair | Count |
|---|---:|
| `font.table-alignment` | 217 |
| `font.tounicode-simple-codes` | 142 |
| `jpeg.entropy-padding` | 57 |
| `xref.offset-corrected` | 30 |
| `image.data-excess` | 27 |
| `font.tounicode-invalid` | 8 |
| `font.cid-to-gid-identity` | 6 |
| `file.header-offset` | 4 |
| `file.header-format` | 4 |
| `file.trailing-bytes` | 2 |
| `xref.rebuilt` | 2 |
| `filter.trailing-data` | 1 |
| `stream.eol` | 1 |
| `security.hash-padding` | 1 |

Raw report: `benchmarks/results/local/scoreboard-verapdf.json` (not committed).
