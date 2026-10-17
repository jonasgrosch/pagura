# Scoreboard: pdfjs

Recorded 2026-10-17T21:27:50+00:00 at commit `0451af7`; binary SHA-256 `58eba42660d3bda1…`;
corpus digest `928cf4a073e547ed…` over 997 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened, pages enumerated | 974 / 996 (97.8%) |
| … strictly, every scanned page compiling | 694 |
| … only with repairs | 280 |
| Failed to open | 18 |
| Opened, but the page tree failed | 4 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 685 / 1836 (37.3%) |

| Open failure | Count |
|---|---:|
| `password` | 10 |
| `syntax` | 7 |
| `filter.unknown` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 2 |
| `missing-object` | 2 |

| Page failure | Count |
|---|---:|
| `syntax` | 642 |
| `font.non-embedded` | 355 |
| `annotation.appearance` | 40 |
| `pattern.tiling` | 20 |
| `transparency.group-colorspace` | 17 |
| `pattern.colorspace` | 13 |
| `colorspace.BlackPoint` | 11 |
| `font.predefined-cmap` | 10 |
| `raster.stroke-adjustment-geometry` | 7 |
| `filter.JPXDecode` | 6 |
| `font.type1-postscript` | 6 |
| `font.cmap-program` | 6 |
| `shading.mesh` | 6 |
| `limit` | 5 |
| `annotation.flags` | 3 |
| `colorspace.special-ink` | 2 |
| `filter.JBIG2Decode` | 1 |
| `extgstate.unknown` | 1 |

| Repair | Count |
|---|---:|
| `filter.trailing-data` | 348 |
| `xref.rebuilt` | 205 |
| `xref.stream-boundary` | 60 |
| `font.stream-lengths` | 21 |
| `xref.offset-corrected` | 15 |
| `stream.length-corrected` | 11 |
| `filter.truncated` | 10 |
| `pages.count-mismatch` | 7 |
| `stream.eol` | 6 |
| `file.header-format` | 5 |
| `syntax.duplicate-key` | 3 |
| `pages.parent-mismatch` | 3 |
| `object.end-missing` | 3 |
| `syntax.key-solidus` | 3 |
| `syntax.name-escape` | 3 |
| `pages.media-box` | 2 |
| `pages.crop-box` | 2 |
| `trailer.reconstructed` | 2 |
| `security.hash-padding` | 2 |
| `pages.cycle` | 1 |
| `security.permissions-unsigned` | 1 |
| `filter.checksum-mismatch` | 1 |
| `syntax.dictionary-closer` | 1 |
| `catalog.type-missing` | 1 |
| `file.header-offset` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfjs.json` (not committed).
