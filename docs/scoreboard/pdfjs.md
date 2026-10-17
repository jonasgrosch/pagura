# Scoreboard: pdfjs

Recorded 2026-10-17T17:24:07+00:00 at commit `6d3c14c`; binary SHA-256 `36c4f1463031bd69…`;
corpus digest `928cf4a073e547ed…` over 997 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened | 975 / 996 (97.9%) |
| … strictly | 738 |
| … only with repairs | 237 |
| Failed to open | 21 |
| Crashes | 0 |
| Timeouts | 0 |
| Pages rendered | 74 / 1769 (4.2%) |

| Open failure | Count |
|---|---:|
| `password` | 11 |
| `syntax` | 9 |
| `filter.unknown` | 1 |

| Page failure | Count |
|---|---:|
| `operator.BT` | 645 |
| `extgstate.HT` | 350 |
| `transparency.group-colorspace` | 196 |
| `image.xobject` | 170 |
| `annotation.appearance` | 151 |
| `syntax` | 33 |
| `extgstate.SM` | 31 |
| `pattern.colorspace` | 27 |
| `colorspace.CalRGB` | 17 |
| `extgstate.unknown` | 11 |
| `operator.Tf` | 10 |
| `shading.paint` | 9 |
| `transparency.SMask` | 8 |
| `extgstate.TK` | 6 |
| `image.inline` | 6 |
| `optionalcontent.usage` | 6 |
| `transparency.knockout` | 6 |
| `transparency.BM` | 5 |
| `colorspace.CalGray` | 3 |
| `extgstate.BG` | 3 |
| `colorspace.DeviceN` | 2 |
| `colorspace.Separation` | 2 |
| `missing-object` | 2 |
| `cycle` | 1 |
| `filter.JBIG2Decode` | 1 |
| `operator.TL` | 1 |
| `transparency.ca` | 1 |
| `transparency.CA` | 1 |
| `extgstate.TR` | 1 |

| Repair | Count |
|---|---:|
| `xref.rebuilt` | 205 |
| `xref.stream-boundary` | 60 |
| `xref.offset-corrected` | 10 |
| `stream.length-corrected` | 7 |
| `filter.trailing-data` | 6 |
| `stream.eol` | 5 |
| `filter.truncated` | 5 |
| `syntax.duplicate-key` | 3 |
| `pages.count-mismatch` | 3 |
| `trailer.reconstructed` | 2 |
| `security.hash-padding` | 2 |
| `file.header-format` | 2 |
| `catalog.type-missing` | 1 |
| `file.header-offset` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfjs.json` (not committed).
