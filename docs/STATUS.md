# Capability and evidence status

Pagura is in **milestones M3–M5** (`docs/ROADMAP.md`). It opens real-world
files reliably and paints vector graphics, text and images; colour and
transparency are partly done, and the remaining gaps fail explicitly with
stable tags. Exploratory timings put rendering at roughly 6–12× slower than
PDFium, MuPDF and CoreGraphics on the benchmark pages and about level with
pdf.js. That is not an ADR 0002 measurement. No claim of conformance or
speed superiority is made. Requirement registries
whose `scope` does not say so have not yet been verified against the
primary standards.

| Area | Implemented | Remaining |
|---|---|---|
| Core | Order-preserving dictionaries (key-indexed above 16 entries), inline names, finite reals, generation-aware references; per-object value budgets; structured errors with `Feature` tags, positions, objects and the repair a strict rejection refused; typed repair/information diagnostics (`Recovery::Strict`/`Repair`) | Broader syntax recovery |
| Document | Classic and stream cross-references, object streams, `/Prev` revisions with cycle detection, hybrid files, lazily parsed `OnceLock`-cached objects, references to undefined objects as null, typed cached page tree with shared resources, the ADR 0003 repair catalogue including full reconstruction | Linearization hints; the remaining failures of other projects' bug-reproduction suites |
| Filters | ASCIIHex, ASCII85, RunLength, LZW, Flate (zlib and reported raw DEFLATE), PNG/TIFF predictors at 1–16 bits; caps enforced while decoding | — |
| Security | MD5, SHA-2, RC4, AES; standard handler R2–R4, R5 (Adobe extension), R6 with SASLprep (Unicode 3.2.0 NFKC) for non-ASCII passwords; crypt filters; `/Perms` checks reported | Public-key handlers (`security.Adobe-PubSec`) |
| Fonts | `pagura-font`: TrueType and OpenType (glyf and CFF/CFF2 default instance), bare and CID-keyed CFF, Type 1 (PFA, PFB, eexec); outlines, advances, matrices, names, built-in encodings, cmap formats 0, 4, 6, 12 | Expert CFF charsets, FDSelect 4, CFF2 Private DICT blends and variations, other cmap formats, hinting |
| Text | `pagura-text`: simple, Type 3 and composite fonts; standard encodings and Differences; widths, CID metrics, embedded CMaps with usecmap, CIDToGIDMap, ToUnicode; non-embedded fonts through bundled substitutes via a `Provider` (ADR 0009): metric-compatible Latin outlines, FoxitSymbol and FoxitDingbats, and, with the opt-in `cjk-fonts` feature, Source Han Sans for the four CJK collections; the predefined CJK CMaps; `pagura licenses` lists the notices; fonts and outlines shared across a document's pages by `RenderContext`; TrueType glyph selection per 9.6.5.4 with documented reader fallbacks; per-font retained-memory estimates | Vertical writing with substituted CID fonts (`font.cid-substitute-vertical`); non-embedded fonts with unknown symbolic encodings or Identity-ordered collections; Type 1 programs that need PostScript |
| Images | `pagura-image`: samples at 1–16 bits, decode arrays, mask metadata, inline images; JPEG (baseline, extended sequential and progressive, Huffman/QM arithmetic, 8/12-bit precision, CMYK/YCCK); CCITT G3/G4; JBIG2 generic, refinement, symbol, text, halftone and Huffman-coded regions; JPEG 2000 Part 1 with precincts, progression changes, code-block modes, ROI and partial-quality reconstruction; reported repairs for damaged data and malformed parameters | JPEG lossless/hierarchical processes; JBIG2 extended templates; JPEG 2000 colour specifications and sample layouts outside the supported set |
| Rendering | Display list; sweep rasterizer with exact-area antialiasing; strokes with caps, joins, dashes and stroke adjustment; geometric clipping; Device, Default, Indexed, ICC (matrix/TRC and LUT profiles), CalGray, CalRGB, Lab, Separation and DeviceN colour; PDF functions with linear and cubic sampled interpolation and transfer functions; form XObjects; text in every render mode, Type 3 glyphs and bundled substitutes; images with masks, soft masks, Decode, Interpolate and separable minification; tiling patterns and mesh shadings; transparency groups, soft masks and blend modes; CMYK and spot overprint display simulation; optional content with View usage; annotation appearances; crop, rotation, UserUnit; tiles; reported recovery of common content faults | Non-RGB group colour spaces (`transparency.group-colorspace`); fractional, pattern and per-colourant transfer regions (`transparency.transfer`); nonempty widgets and rotated widget borders without explicit appearances; known defect: the 8-bit path for all-opaque pages can exceed ADR 0004's one-level bound at partially covered pixels, because it quantizes source colours and rounds twice per mark (up to about 1.5 levels per partial store, accumulating) |
| Reading | Positioned text extraction from the content interpreter, with words, lines, reading order, ActualText/Alt/ReversedChars (14.9.4) and search with highlight quads (`pagura text`, `pagura find`); a separate logical view for tagged pages (14.7–14.8: structure order, role maps, artifacts, structure ActualText/Alt, linked to the page's glyphs) that never changes page search or selection; viewer search, selection, copy, links and outlines (protocol 1.4); navigation: name and number trees, every destination type, named destinations, outlines, link annotations, inert actions (never followed or executed) and page labels; a form field model (AcroForm 12.7: field tree, types, inherited values, widgets, inert field actions; XFA detected, not interpreted); in-memory filling of text, check box, radio and choice fields, with their widget appearances prepared as data (12.7.4.3: DA and DR, auto-size, quadding, comb, multiline, MK borders and backgrounds) | The logical view in the viewer's accessibility tree; structure namespaces (`text.structure-namespace`); bidirectional text; rendering and saving filled forms; XFA (`form.xfa`), rich text (`form.rich-text`) and other field types (`form.field-type`); appearances with oblique text matrices or negative font sizes (`form.text-matrix`), other DA operators (`form.default-appearance`), fonts without a reversible simple encoding (`form.font-encoding`), rounded, beveled or inset borders (`form.border-style`), rotated widgets (`form.widget-rotation`) and AP updates with inline images, several Tx sections or crossed scopes (`form.appearance-update`); accessibility; signature validation (ADR 0015) (M6) |
| Signatures | Read-only fields, source-bound ByteRange and revision coverage; `pagura::signatures` verifies CMS integrity for adbe.pkcs7.detached, ETSI.CAdES.detached and ETSI.RFC3161: SHA-1 (reported weak), SHA-224/256/384/512, RSA v1.5/PSS and ECDSA P-256/P-384/P-521, signed attributes, signer-certificate identifiers, ESS v1/v2 binding, CMSAlgorithmProtection and timestamp imprints/token signatures; isolated signature errors and malformed certificate schemas; Perms/DocMDP certification link | Host trust anchors/path validation, revocation, modification classification, TSA authorization and clock trust, complete PAdES attribute/policy validation; normalized issuer-name matching; unreadable form values can still prevent discovery (ADR 0015) |
| Writing | Deterministic new documents; immutable editing transactions; append-only classic/xref-stream revisions; saved text/choice fields and existing checkbox/radio states with appearances; inline browser form controls and PDF download (ADR 0031) | Full rewrite, object deletion, encryption-on-write, redaction, undo/redo and general page-content editing; signed-change permission enforcement; generated button states and nested direct fields |
| WebAssembly | `pagura-host` WASI command (protocol v1, `crates/pagura-host/PROTOCOL.md`); dependency-free browser shim and viewer; native/WASI byte parity; protocol 1.1 viewport generations; bounded LRU of compiled pages; continuous scrolling, zoom and a password prompt; verified in a real browser (cross-origin isolated, exact pixels at DPR 2, multi-page navigation, explicit unsupported banner) | Threads; interrupting a running tile; browser checks of the new gestures |
| Safety | Workspace-only product dependencies; every product target forbids unsafe; clippy pedantic, `missing_docs`, no `unwrap`/`expect`/`panic`; eleven cargo-fuzz targets with a bounded smoke run (`tools/fuzz_smoke.py`) and every finding replayed on stable Rust | Process sandbox, long fuzz campaigns |
| Benchmarks | Stage-timed conversion track (open/render/emit, document mode), regression track, raw-format normalization, competitors out of tree (PDFium, MuPDF, pdf.js, hayro, CoreGraphics, Poppler) | Controlled runners; release gate |

## Editing foundations (M7)

[ADR 0031](adr/0031-editing-foundations.md) records the reviewed pending writer
and typed-document dependencies. This branch implements library APIs described
in [Editing](EDITING.md); it does not complete M7. Modified saves require strict,
unencrypted input. Signed input needs an explicit original-byte preservation
policy, which makes no signature-validity or permitted-change claim.

`pagura/tests/api/editing.rs` exercises independent source syntax, prefix/trailer and
generation preservation, rollback, repeated saves, bounds, signature integrity,
and form save/reopen pixel equivalence. Hosted CI performs the full product
contract; `editing-interoperability` also checks saved outputs with qpdf and
compares Poppler page counts against their original inputs. Poppler 24.02.0
rejects the original `document/hybrid-precedence` fixture and its saved version;
qpdf 11.9.0 accepts both. This is recorded as a baseline incompatibility, not a
successful Poppler reopen. The PR check results record validation for each exact
branch head.
No local builds, benchmarks or test suites run in this lane.

## Accuracy policy

`RenderOptions::accuracy` selects `Accuracy::Exact`, the default, which keeps
every pixel within one level of its exact result, or `Accuracy::Competitive`,
an approximate path measured against ADR 0004's competitive accuracy gate:
row-fragment glyph masks, signed-cell fills, clip products, one-round byte
composition, fused decoding of sole JPEG pages and drawn-size reconstruction
of other minified JPEG images. The CLI exposes `--accuracy exact|competitive`;
the host protocol and viewer render exactly.

## Overprint display policy

`RenderOptions::overprint` selects `OverprintPreview::{Auto, Always, Never}`;
Auto is the default. Auto simulates only when the document declares PDF/X print
intent: an `OutputIntents` entry with `S /GTS_PDFX` (14.11.5, ISO 15930), or a
`GTS_PDFXVersion` string in Info. Catalog and PDF 2.0 page output intents are
recognized, including indirect entries. A resolved page array takes precedence
over the catalog, even when empty; the Info declaration remains independent.
This detects a declaration, not PDF/X conformance; malformed advisory metadata
is treated as absent. Always enables
simulation wherever it can affect painted colourants. Never ignores overprint
and paints ordinary composite content, including in CMYK transparency groups
and soft-mask sources.
The CLI exposes `--overprint auto|always|never` for rendering and scanning.
The host protocol and viewer use Auto; a versioned render option and viewer
toggle are the next host step.

When enabled, simulation (8.6.7, 11.7.3–11.7.4) uses a CMYK process device if
an overprinting paint coexists with painted Separation or DeviceN colour, or
if an overprinting direct DeviceCMYK paint uses OPM 1. No-op flags keep the
ordinary rendering path and blending space. Explicit group blending spaces
still apply to process colour; spot plates pass through groups without colour
conversion, and soft masks always use their alternate spaces.

At page output, spot alternates supply CMYK ink equivalents, which are added
with saturation before RGB conversion. The first painted definition of a
colourant name supplies its alternate transform; channels sharing that
first DeviceN definition are transformed together. This is a display policy,
not a prediction of physical ink interaction. A simulated page supports at
most 32 spot plates; larger sets fail with `Limit`. Destination ICC output
transforms and transfer selection under transparency remain unimplemented.

## Evidence (scoreboards in `docs/scoreboard/`)

The realworld and adversarial rows were re-recorded at commit c18ae90; the
other rows date from commit 8909e1c.
Each file gets 60 seconds of CPU time, so machine load does not turn into
timeouts. Real-world files that exceed it now render all their pages too slowly;
painting dominates their time and is being optimized.

| Corpus | PDFs | Open, pages enumerated | Strictly | Pages rendered | Crashes / timeouts |
|---|---:|---:|---:|---:|---:|
| realworld (SafeDocs Common Crawl zip 0000) | 998 | 99.5% | 614 | 94.0% (13163/14008) | 0 / 3 |
| adversarial (SafeDocs UNSAFE, mutated) | 1,416 | 99.9% | 886 | 88.7% (3544/3997) | 0 / 0 |
| veraPDF | 2,906 | 100.0% | 2,633 | 97.9% (12681/12948) | 0 / 0 |
| pdf.js test PDFs | 996 | 98.0% | 635 | 60.7% (1115/1838) | 0 / 0 |
| qpdf test PDFs | 701 | 95.7% | 578 | 4.2% (159/3817) | 0 / 0 |
| PDFium test PDFs | 336 | 90.8% | 184 | 44.9% (170/379) | 0 / 0 |
| PDFBox test PDFs | 174 | 94.8% | 138 | 36.2% (111/307) | 0 / 0 |
| Poppler test PDFs | 83 | 95.2% | 65 | 70.8% (68/96) | 0 / 0 |
| PDF Differences | 37 | 75.7% | 24 | 23.3% (10/43) | 0 / 0 |
| PDF 2.0 examples | 7 | 100.0% | 6 | 12.5% (1/8) | 0 / 0 |

Generated fixtures: 1022 PDF cases with independent expectations pass
(`tools/conformance.py`), plus 337 codec, 97 crypto and 19 raster vectors in
crate tests.

Informational diagnostics (`DiagnosticKind::Information`, ADR 0003/0009):
`font.substituted` reports permitted non-embedded font substitution;
`font.cid-glyph-missing` reports incomplete substitute coverage once per font,
keeping the PDF advance while an unavailable glyph paints nothing. Both are
permitted in Strict and Repair modes; neither is an applied repair. All other
existing diagnostic tags remain repairs. CLI reports separate `repairs` and
`information` lists. Host protocol 1.4 and the viewer preserve the distinction;
older protocol diagnostic snapshots contain repairs only.

Signature integrity is a separate mathematical result, never an unqualified
validity flag. `Document::signatures` remains structural; the facade adds CMS
verification. CMS buffer errors and repairs are retained per signature, and
malformed embedded certificates remain visible without suppressing a usable
signer certificate. Limits bound parsing, hashing, certificate counts and
verification counts; no URLs are fetched. Issuer/serial and ESS issuer names
currently require identical DER. Unreadable form trees or non-dictionary Sig
values fail discovery; dictionary-level errors are per field. Timestamp
integrity does not establish the asserted time or TSA trust. Independent
signed-PDF cases are in `corpus/manifests/cms-signatures.json`.

Repairs with reader-dependent behaviour (security-relevant, ADR 0003):

- `asn1.ber-set-order` (X.690 11.6; RFC 5652 5.1): retain BER SET OF source
  order with a diagnostic. Signed attributes, certificates and DER entry
  points remain strict. BER length, indefinite-container and OCTET STRING
  deviations also remain reported; none normalizes signed bytes.

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
  aliases lose their Unicode value. Strict rejects font loading on malformed
  ToUnicode data, preserving the original error kind, repair tag, object and
  position without reporting an applied repair. Repair mode retains drawable
  fonts and reports extraction repairs or discarded maps. Null ToUnicode
  values, including indirect and
  undefined references, are absent without repairs (7.3.9–7.3.10).
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
- `image.decode-params` (7.3.8.1, Table 5; 7.4.1): a `(null)` string or an
  unused wrong-typed entry in an image's DecodeParms is treated as absent;
  `jpeg.entropy-excess` (T.81 B.2.1): excess bytes immediately before EOI are
  discarded; `image.stencil-color-space` (8.9.5.1, Table 87): an image mask's
  extraneous ColorSpace is ignored; `image.filter-abbreviation` (8.9.7, Table
  92): the inline-image filter abbreviations are accepted on image XObjects.
- `text.clip-mode-reset` (9.3.6, 9.4.1): a text rendering mode reset to a
  non-clipping mode before `ET` keeps the clip already accumulated.
- `text.marked-property` (14.6.2, 14.9.3–14.9.5): an unusable marked-content
  property list, or a wrong-typed ActualText, Alt or E, is ignored; glyphs
  are unaffected.
- `text.structure-fallback` (14.7–14.8): an unusable or unsupported
  structure tree leaves the page without a logical view; its page text,
  search and selection are unaffected.
- `font.descriptor-flags` (9.8.2, Table 121; 9.6.5): Symbolic and
  Nonsymbolic both set or both clear; Symbolic is kept and Nonsymbolic is
  treated as its inverse. Valid pairs are not repairs and keep their 9.6.5
  selection routes, even when the font's character set disagrees.
- `image.ccitt-columns` (7.4.6, 8.9.5.1): CCITT data whose `Columns` differs
  from the image `Width` decodes with `Columns`, then each row is cropped or
  extended with fax white to `Width`.
- `annotation.appearance-empty` (7.9.5, 12.5.5): an appearance with zero extent
  is skipped without running its content.
- Navigation (7.9.6–7.9.7, 12.3.2–12.3.3, 12.5.6.5, 12.6.2): `tree.limits`,
  `tree.order`, `tree.cycle` and `tree.structure` read malformed name and
  number trees in their best-defined order; `outline.links`, `outline.cycle`,
  `outline.count` and `outline.type` rebuild a consistent outline tree;
  `destination.page-number`, `string.invalid-text`, `navigation.target-conflict`,
  `link.quad-points` and `action.cycle` cover the remaining navigation faults.
- `font.cff-dict-operands`, `font.cff-dict-operator` (CFF section 4 and
  Appendix H): unused hint arrays with no operands are dropped, and reserved
  DICT operators are ignored with their operands; `font.cff-encoding-supplement`
  (CFF section 12): a supplement for a glyph absent from the main encoding is
  dropped; `font.type1-duplicate-glyph` (Type 1 chapters 6 and 10.3): the first
  definition of a repeated glyph name is kept.
- Forms (7.9.2.2, 12.6.2, 12.7.3–12.7.4): `form.fields`, `form.name`,
  `form.duplicate-name`, `form.parent`, `form.cycle` and `form.structure` read a
  malformed field tree into a consistent model; equivalent duplicate names
  stay valid.
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

- `content.graphics-unclosed` (7.8.2, 8.4.2): unmatched saved graphics states
  are discarded at the end of an otherwise finished content sequence.
- `font.type3-width` (9.6.4, Table 111): `Widths` controls the advance when a
  CharProc's `d0`/`d1` width disagrees or declares a nonzero vertical advance;
  glyph geometry is retained.
- `font.type3-empty-bounds` (9.6.4, Table 111): reversed `d1` bounds are
  ignored only after the entire Type 3 procedure compiles without drawing
  commands. `Widths`, text extraction and following page content are retained.
  Strict mode rejects this repair; reversed bounds on painted glyphs and
  other procedure errors remain failures in both modes.
- `content.pattern-underlying` (8.7.3.2, Table 73): coloured patterns keep their
  intrinsic colours and accept a lone pattern name even when a base space is
  declared. Extra numeric underlying-colour operands are reported and discarded.
  The base space remains available for a later uncoloured pattern.
- `filter.lzw-initial-clear` (7.4.4.2): a literal or EOD before the initial
  clear code uses the initial dictionary; invalid dictionary codes still fail.
- `jpeg.trailing-restart` (T.81 B.2.1, B.2.4.4): a restart immediately before
  EOI after the complete MCU sequence is discarded.
- `image.predictor-precision` (7.4.4.3): syntax failures with conflicting valid
  predictor precision are retried with the image's BitsPerComponent. Only a
  successful retry reports a repair; a failed retry retains the original error.
- `jpx.tile-part-count` (T.800 A.4.2): inconsistent tile-part counts are ignored
  while ordered part indices, exact byte lengths and packet validation remain
  mandatory. No samples or entropy codes are synthesized.
- `jpx.mapping-box-location` (T.800 I.5.3.4–I.5.3.5): a sole mapping box before
  the codestream supplies the header palette's missing mapping; duplicate or
  late mappings and missing palettes stay errors.
- `image.xobject-type` (8.9.5, Table 87): `Subtype /Image` identifies an image
  XObject despite a malformed optional `Type`; invalid subtypes remain errors.
- `image.filter-mismatch` (7.4.4, 7.4.8): a purported JPEG with predictor
  parameters uses Flate only after strict zlib framing, checksum, row-tag and
  exact eight-bit sample-count validation. Incomplete, checksum-damaged or
  ambiguously described data remain errors.

Empty text and unsigned signature widgets without an explicit appearance paint
only their specified `MK` background and border (12.5.6.19, 12.7.5). Field type
and value inherit through `Parent`; nonempty values still require an appearance.
Cubic sampled functions use local Hermite interpolation with centred interior
and one-sided endpoint tangents; 7.10.2 leaves the boundary conditions unspecified.
Applying the linear fallback independently to axes with fewer than four samples
is the project reading of 7.10.2. Transfer-region selection follows
11.7.5.2 for whole device pixels; arrays of four `Identity` names are the
default transfer, including through indirect references. A fractional boundary involving a nondefault
transfer still fails explicitly: averaging colour before region selection would
lose the correlation needed for nonlinear transfer functions.

ICC profiles are not applied: `ICCBased` spaces use their alternate or the
`/N` default, as clause 8.6.5.5 permits.

`python3 tools/check.py` runs every product check offline; `python3
tools/policy.py --local-assets` verifies private caches and pinned corpora.
Release gate output is expected to be BLOCKED with exit code 2. The ISO
32000-2:2020 text (sponsored, with errata collection 3) is registered as a
locally supplied, single-user reference (`references/lock.json`); it is never
committed or redistributed.
