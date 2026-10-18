# Capability and evidence status

Pagura is in **milestones M1–M2** (`docs/ROADMAP.md`). It opens real-world files reliably and renders vector pages.
Font programs, PDF fonts and image codecs now decode, but the renderer does
not paint text or images yet, so most real pages still fail explicitly. No
claim of conformance or speed superiority is made. Requirement registries
whose `scope` does not say so have not yet been verified against the
primary standards.

| Area | Implemented | Remaining |
|---|---|---|
| Core | Order-preserving dictionaries (key-indexed above 16 entries), inline names, finite reals, generation-aware references; per-object value budgets; structured errors with `Feature` tags, positions, objects and the repair a strict rejection refused; reported repairs (`Recovery::Strict`/`Repair`) | Broader syntax recovery |
| Document | Classic and stream cross-references, object streams, `/Prev` revisions with cycle detection, hybrid files, lazily parsed `OnceLock`-cached objects, references to undefined objects as null, typed cached page tree with shared resources, the ADR 0003 repair catalogue including full reconstruction | Linearization hints; the remaining failures of other projects' bug-reproduction suites |
| Filters | ASCIIHex, ASCII85, RunLength, LZW, Flate (zlib and reported raw DEFLATE), PNG/TIFF predictors at 1–16 bits; caps enforced while decoding | — |
| Security | MD5, SHA-2, RC4, AES; standard handler R2–R4, R5 (Adobe extension), R6; crypt filters; `/Perms` checks reported | SASLprep for non-ASCII R6 passwords; public-key handlers |
| Fonts | `pagura-font`: TrueType and OpenType (glyf and CFF/CFF2 default instance), bare and CID-keyed CFF, Type 1 (PFA, PFB, eexec); outlines, advances, matrices, names, built-in encodings, cmap formats 0, 4, 6, 12 | Expert CFF charsets, FDSelect 4, CFF2 Private DICT blends and variations, other cmap formats, hinting |
| Text | `pagura-text`: simple, Type 3 and composite fonts; standard encodings and Differences; widths, CID metrics, embedded CMaps with usecmap, CIDToGIDMap, ToUnicode; non-embedded standard fonts through metric-compatible substitutes and the predefined CJK CMaps, both from `pagura-data` through a `Provider` (ADR 0009, on by default in the `pagura` facade, `pagura licenses` lists the notices); fonts and outlines shared across a document's pages by `RenderContext`; TrueType glyph selection per 9.6.5.4 with documented reader fallbacks; per-font retained-memory estimates | Symbol and ZapfDingbats substitutes; non-embedded CID fonts |
| Images | `pagura-image`: samples at 1–16 bits, decode arrays, mask metadata, inline images; JPEG (baseline and progressive Huffman, CMYK/YCCK); CCITT G3/G4; JBIG2 generic, symbol and text regions; JPEG 2000 Part 1 with default precincts | Painting (M4); JBIG2/JPX wiring into XObjects; JBIG2 refinement, Huffman, halftones; JPX precinct and code-block modes; JPEG arithmetic and 12-bit |
| Rendering | Display list; sweep rasterizer with exact-area antialiasing; strokes with caps, joins, dashes and stroke adjustment; geometric clipping; Device, Default, Indexed, ICC-via-alternate, CalGray, Lab, Separation and DeviceN colour; PDF functions (Types 0, 2, 3, 4) and transfer functions; form XObjects; opaque transparency groups including RGB blending spaces; optional content with View usage; crop, rotation, UserUnit; tiles | Dashes under scaling and negative dash phases (8.4.3.6); CalRGB; painting text (M3) and images (M4); non-opaque transparency, shadings, patterns (M5) |
| Writing | Deterministic new documents, insertion-ordered dictionaries, budgets | Incremental saves, signatures, redaction (M7) |
| WebAssembly | `pagura-host` WASI command (protocol v1, `crates/pagura-host/PROTOCOL.md`); dependency-free browser shim and viewer; native/WASI byte parity; protocol 1.1 viewport generations; bounded LRU of compiled pages; continuous scrolling, zoom and a password prompt; verified in a real browser (cross-origin isolated, exact pixels at DPR 2, multi-page navigation, explicit unsupported banner) | Threads; interrupting a running tile; browser checks of the new gestures |
| Safety | Workspace-only product dependencies; every product target forbids unsafe; clippy pedantic, `missing_docs`, no `unwrap`/`expect`/`panic`; nine cargo-fuzz targets with a bounded smoke run (`tools/fuzz_smoke.py`) and every finding replayed on stable Rust | Process sandbox, long fuzz campaigns |
| Benchmarks | Stage-timed conversion track (open/render/emit, document mode), regression track, raw-format normalization, competitors out of tree (PDFium, MuPDF, pdf.js, hayro, CoreGraphics, Poppler) | Controlled runners; release gate |

## Evidence (scoreboards in `docs/scoreboard/`)

Each file gets 60 seconds of CPU time, so machine load does not turn into
timeouts. Real-world files that exceed it now render all their pages too slowly;
painting dominates their time and is being optimized.

| Corpus | PDFs | Open, pages enumerated | Strictly | Pages rendered | Crashes / timeouts |
|---|---:|---:|---:|---:|---:|
| realworld (SafeDocs Common Crawl zip 0000) | 998 | 98.8% | 705 | 53.1% (6476/12204) | 0 / 10 |
| adversarial (SafeDocs UNSAFE, mutated) | 1,416 | 99.9% | 963 | 49.8% (1989/3997) | 0 / 0 |
| veraPDF | 2,906 | 100.0% | 2,633 | 97.9% (12681/12948) | 0 / 0 |
| pdf.js test PDFs | 996 | 98.0% | 635 | 60.7% (1115/1838) | 0 / 0 |
| qpdf test PDFs | 701 | 95.7% | 578 | 4.2% (159/3817) | 0 / 0 |
| PDFium test PDFs | 336 | 90.8% | 184 | 44.9% (170/379) | 0 / 0 |
| PDFBox test PDFs | 174 | 94.8% | 138 | 36.2% (111/307) | 0 / 0 |
| Poppler test PDFs | 83 | 95.2% | 65 | 70.8% (68/96) | 0 / 0 |
| PDF Differences | 37 | 75.7% | 24 | 23.3% (10/43) | 0 / 0 |
| PDF 2.0 examples | 7 | 100.0% | 6 | 12.5% (1/8) | 0 / 0 |

Generated fixtures: 918 PDF cases with independent expectations pass
(`tools/conformance.py`), plus 337 codec, 97 crypto and 19 raster vectors in
crate tests.

Repairs with reader-dependent behaviour (security-relevant, ADR 0003):

- `font.sfnt-version`, `font.table-alignment`, `font.table-bounds`,
  `font.cmap-truncated`, `font.cmap-data`, `font.cmap-alignment`,
  `font.component-flags`, `font.charstring-operator`, `font.glyph-data`:
  diagnostic-aware font APIs infer usable containers, retain intact
  tables/mappings, ignore reserved component bits, end unknown charstrings and
  replace unusable glyphs with empty paths; page callers still need to forward
  these diagnostics.
- `syntax.duplicate-key`: the last value of a repeated key wins.
- `image.data-short`, `image.data-excess` (8.9.3, 8.9.5.1): zero-fill missing
  packed bytes or ignore the excess suffix; `jpeg.entropy-padding` (T.81
  B.1.1.5, E.1.4): ignore non-one unused entropy bits; `jpeg.adobe-marker`
  (7.4.8, Table 13): use the known APP14 prefix and ignore its extension (Adobe
  TN 5116 review pending); `ccitt.eob-padding` (Table 11; T.6 2.4.1.1-2): ignore
  up to seven zero fill bits before a byte-aligned Group 4 EOFB;
  `ccitt.damaged-row` (Table 11): substitute the preceding good row or white
  within the declared tolerance.
- `font.cid-to-gid-identity` (9.7.4.1, Table 115): a missing embedded Type2
  CIDFont map uses Identity; `font.cmap-block-count` (9.7.5.4; TN5014 7.4):
  accept blocks above 100 within budgets; `font.cmap-codespace` (9.7.6.2): split
  malformed byte bounds into exact disjoint integer-interval pieces;
  `font.tounicode-simple-codes`, `font.tounicode-utf16`, `font.tounicode-range`,
  `font.tounicode-invalid` (9.10.3): use low-byte simple-font sources, omit
  invalid UTF-16 values, carry overflowing range destinations without
  full-string wrap, or discard unusable extraction data. Conflicting low-byte
  aliases lose their Unicode value. Strict rejects extraction repairs and
  discards their map, reporting the rejection while painting continues;
  painting-critical repairs still reject in Strict.
- `content.colour-operator` (8.6.8, Table 73): `sc`/`SC` on a space that needs
  `scn`/`SCN` reads matching numeric operands as `scn`/`SCN`;
  `content.empty-path` (8.5.3.1): painting with no current path paints nothing
  and discards a pending clip; `content.clip-order` (8.5.4): construction after
  `W`/`W*` keeps the deferred clip; `content.indexed-excess` (8.6.6.3): excess
  Indexed lookup bytes are ignored, short palettes stay errors.
  `content.optional-group`, `content.optional-config`,
  `content.optional-groups`, `content.optional-properties` (8.11.4.2, Tables
  98–99): unregistered groups start ON and incomplete configurations disable
  optional-content evaluation. Readers disagree on `AnyOff` membership and on
  empty clips; these are project policy.
- `font.descriptor-weight` (9.8.1, Table 120): an invalid optional FontWeight
  is ignored when choosing a substitute.
- `font.cff-blue-order` (CFF section 15, Table 23; Type 1 5.3–5.5): unused
  hint zones whose pairs do not ascend are ignored; outlines and widths are
  unchanged.
- `filter.truncated`, `filter.checksum-mismatch`, `filter.trailing-data`,
  `filter.raw-deflate`: damaged filter data yields the bytes decoded so far.
- `filter.lzw-full-table`, `filter.lzw-padding`: an LZW encoder that fills its
  table without a clear code, or leaves nonzero bits after EOD (7.4.4.2), is
  decoded with the table frozen and the padding ignored.
- `xref.rebuilt`, `xref.offset-corrected`, `xref.stream-boundary`,
  `stream.length-corrected`, `stream.eol`, `trailer.reconstructed`,
  `file.header-offset`, `file.header-format`, `catalog.type-missing`:
  structure recovered by scanning; the last complete physical definition of
  an object wins.
- `security.permissions-unsigned`, `security.literal-hash`,
  `security.hash-padding`: malformed encryption entries recovered; normal
  authentication is still required.

ICC profiles are not applied: `ICCBased` spaces use their alternate or the
`/N` default, as clause 8.6.5.5 permits.

`python3 tools/check.py` runs every product check offline; `python3
tools/policy.py --local-assets` verifies private caches and pinned corpora.
Release gate output is expected to be BLOCKED with exit code 2. The ISO
32000-2:2020 text (sponsored, with errata collection 3) is registered as a
locally supplied, single-user reference (`references/lock.json`); it is never
committed or redistributed.
