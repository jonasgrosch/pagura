# Scoreboard: pdfjs

Recorded 2026-10-17T18:26:44+00:00 at commit `4390401`; binary SHA-256 `5f8a67a475781cf9…`;
corpus digest `928cf4a073e547ed…` over 997 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened, pages enumerated | 969 / 996 (97.3%) |
| … strictly, every scanned page compiling | 734 |
| … only with repairs | 235 |
| Failed to open | 17 |
| Opened, but the page tree failed | 10 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 79 / 1773 (4.5%) |

| Open failure | Count |
|---|---:|
| `password` | 11 |
| `syntax` | 5 |
| `filter.unknown` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 7 |
| `missing-object` | 2 |
| `cycle` | 1 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 820 |
| `extgstate.HT` | 351 |
| `image.xobject` | 178 |
| `annotation.appearance` | 175 |
| `pattern.colorspace` | 38 |
| `syntax` | 30 |
| `colorspace.CalRGB` | 18 |
| `operator.Tf` | 11 |
| `extgstate.unknown` | 11 |
| `shading.paint` | 10 |
| `transparency.group-colorspace` | 10 |
| `transparency.SMask` | 10 |
| `image.inline` | 7 |
| `transparency.knockout` | 6 |
| `transparency.BM` | 5 |
| `transparency.ca` | 4 |
| `transparency.CA` | 4 |
| `filter.JBIG2Decode` | 1 |
| `raster.stroke-adjustment-geometry` | 1 |
| `colorspace.BlackPoint` | 1 |
| `colorspace.special-ink` | 1 |
| `operator.TL` | 1 |
| `colorspace.unknown` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 201 |
| `xref.stream-boundary` | 60 |
| `filter.trailing-data` | 14 |
| `xref.offset-corrected` | 10 |
| `stream.length-corrected` | 9 |
| `stream.eol` | 5 |
| `filter.truncated` | 5 |
| `file.header-format` | 5 |
| `syntax.duplicate-key` | 3 |
| `pages.count-mismatch` | 3 |
| `trailer.reconstructed` | 2 |
| `security.hash-padding` | 2 |
| `object.end-missing` | 2 |
| `catalog.type-missing` | 1 |
| `file.header-offset` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfjs.json` (not committed).
