# Scoreboard: pdfjs

Recorded 2026-10-17T22:29:13+00:00 at commit `c38e654`; binary SHA-256 `40ac06f4a363b57c…`;
corpus digest `928cf4a073e547ed…` over 997 files. Pages scanned at 72 dpi.

| Metric | Value |
|---|---:|
| Not PDF (no `%PDF-` in the first 1024 bytes) | 1 |
| PDF files opened, pages enumerated | 976 / 996 (98.0%) |
| … strictly, every scanned page compiling | 635 |
| … only with repairs | 341 |
| Failed to open | 16 |
| Opened, but the page tree failed | 4 |
| Crashes | 0 |
| Timeouts (60 s CPU budget) | 0 |
| Pages rendered | 1115 / 1838 (60.7%) |

| Open failure | Count |
|---|---:|
| `password` | 10 |
| `syntax` | 5 |
| `filter.unknown` | 1 |

| Page-tree failure | Count |
|---|---:|
| `syntax` | 2 |
| `missing-object` | 2 |

| Page failure | Count |
|---|---:|
| `font.non-embedded` | 371 |
| `syntax` | 245 |
| `annotation.appearance` | 41 |
| `transparency.group-colorspace` | 17 |
| `colorspace.BlackPoint` | 11 |
| `font.predefined-cmap` | 11 |
| `filter.JPXDecode` | 6 |
| `font.type1-postscript` | 6 |
| `limit` | 5 |
| `transparency.overprint` | 3 |
| `image.stencil-pattern` | 2 |
| `pattern.tiling` | 2 |
| `cycle` | 1 |
| `filter.JBIG2Decode` | 1 |
| `transparency.transfer` | 1 |

| Repair | Count |
|---|---:|
| `filter.trailing-data` | 376 |
| `xref.rebuilt` | 205 |
| `font.tounicode-simple-codes` | 72 |
| `xref.stream-boundary` | 60 |
| `font.table-alignment` | 58 |
| `font.tounicode-utf16` | 44 |
| `font.tounicode-range` | 44 |
| `font.table-bounds` | 28 |
| `font.stream-lengths` | 23 |
| `font.cid-to-gid-identity` | 22 |
| `font.tounicode-invalid` | 21 |
| `syntax.key-solidus` | 18 |
| `xref.offset-corrected` | 16 |
| `stream.length-corrected` | 11 |
| `filter.truncated` | 10 |
| `stream.eol` | 8 |
| `pages.count-mismatch` | 7 |
| `font.cmap-block-count` | 6 |
| `font.cmap-data` | 5 |
| `file.header-format` | 5 |
| `font.cmap-alignment` | 4 |
| `syntax.duplicate-key` | 3 |
| `pages.parent-mismatch` | 3 |
| `object.end-missing` | 3 |
| `syntax.name-escape` | 3 |
| `font.cmap-truncated` | 2 |
| `pages.media-box` | 2 |
| `pages.crop-box` | 2 |
| `font.cmap-codespace` | 2 |
| `trailer.reconstructed` | 2 |
| `security.hash-padding` | 2 |
| `security.short-key` | 2 |
| `security.key-padding` | 2 |
| `jpeg.adobe-marker` | 2 |
| `pages.cycle` | 1 |
| `security.permissions-unsigned` | 1 |
| `security.crypt-filter-event` | 1 |
| `filter.checksum-mismatch` | 1 |
| `security.crypt-filter-name` | 1 |
| `image.data-excess` | 1 |
| `syntax.dictionary-closer` | 1 |
| `catalog.type-missing` | 1 |
| `file.header-offset` | 1 |

Raw report: `benchmarks/results/local/scoreboard-pdfjs.json` (not committed).
