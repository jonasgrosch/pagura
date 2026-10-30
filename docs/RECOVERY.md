# Reported recovery and information diagnostics

These policy notes were retained from the capability inventory when its overview
was reconciled against `render/one-pass` at `19059e684`. They describe supported
compatibility choices, not
an exhaustive repair catalogue or new test results. See [ADR 0003](adr/0003-reported-recovery.md),
[requirement registries](../references/requirements/) and each crate’s repair
constants for authoritative scope and independent fixtures. Current capability
boundaries are in [STATUS.md](STATUS.md).

Informational diagnostics (`DiagnosticKind::Information`, ADR 0003/0009):
`font.substituted` reports permitted non-embedded font substitution;
`font.cid-glyph-missing` reports incomplete substitute coverage once per font,
keeping the PDF advance while an unavailable glyph paints nothing.
`pages.contents-undefined` reports a page whose `/Contents` refers to an
undefined object: the reference is null (7.3.10), so the page renders without
content instead of failing, and the report keeps that page from looking complete.
`form.appearance-kept` reports keeping an untouched field's valid stored
appearance when generating a replacement is unsupported. These information
diagnostics are permitted in Strict and Repair modes and are not applied
repairs. CLI reports separate `repairs` and `information` lists. Host protocol
1.4 and the viewer preserve the distinction;
older protocol diagnostic snapshots contain repairs only.

Repairs with reader-dependent behaviour (security-relevant, ADR 0003):

- `jbig2.page-capabilities` (T.88 7.4.8.5, bits 1 and 5): complete
  auxiliary/refinement region segments override contradicting page capability
  declarations. This is a reported viewing policy, not conforming input.
  Strict decoding rejects the contradiction. Region syntax, reference identity,
  combination operators, framing, cycles and all work/storage budgets retain
  their checks; failed decoding never returns a partial bitmap.

- `jpx.raw-codestream` (7.4.9): viewing may decode an unboxed JPEG 2000
  codestream using an explicit PDF image colour space. This is a reported
  repair of the required full-file framing, not conforming JPX input.
  Missing colour spaces and embedded-opacity requests stay errors. Dimensions,
  components, sampling geometry, complete codestream syntax and all resource
  limits retain their existing validation; no colour or opacity metadata is
  guessed. Strict mode rejects the same unboxed input. An explicit PDF
  colour space remains authoritative (7.4.9), including subsampled RGB;
  T.800 A.5.1/B.2 component grid steps do not imply YCbCr. A reader that
  infers YCbCr from sampling ratios can disagree and is not a pixel oracle
  for this policy; the subsampled fixture derives pixels from stated RGB
  sample arrays and grid placement.

- `asn1.ber-set-order` (X.690 11.6; RFC 5652 5.1): retain BER SET OF source
  order with a diagnostic. Signed attributes, certificates and DER entry
  points remain strict. BER length, indefinite-container and OCTET STRING
  deviations also remain reported; none normalizes signed bytes.

- `font.sfnt-version`, `font.table-alignment`, `font.table-bounds`,
  `font.cmap-truncated`, `font.cmap-data`, `font.cmap-alignment`,
  `font.component-flags`, `font.charstring-operator`, `font.glyph-data`:
  diagnostic-aware font APIs infer usable containers, retain intact
  tables/mappings, ignore reserved component bits, end unknown charstrings and
  replace unusable glyphs with empty paths; callers must forward their
  diagnostics. Page outline decoding already forwards its sink through the
  document reporting API.
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
- `content.color-operator` (8.6.8, Table 73): `sc`/`SC` on a space that needs
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
- `content.text-clip-reset` (9.3.6, 9.4.1): a text rendering mode reset to a
  non-clipping mode before `ET` keeps the clip already accumulated.
- `text.marked-property` (14.6.2, 14.9.3–14.9.5): an unusable marked-content
  property list, or a wrong-typed ActualText, Alt or E, is ignored; glyphs
  are unaffected.
- `text.structure-fallback` (14.7–14.8): an unusable or unsupported
  structure tree leaves the page without a logical view; its page text,
  search and selection are unaffected.
- `font.descriptor-flags` (9.8.2, Table 121; 9.6.5): Symbolic and
  Nonsymbolic both set or both clear, or `/Flags` missing; Symbolic is kept
  and Nonsymbolic is treated as its inverse. Valid pairs are not repairs and
  keep their 9.6.5 selection routes, even when the font's character set
  disagrees. Applied by `pagura-text` for every caller of `PdfFont`.
- `font.symbolic-latin-encoding` (9.6.5, 9.8.2; ADR 0009): a non-embedded
  font flagged Symbolic whose name is a known Latin family and whose
  `/Encoding` names a Latin base encoding (Standard, WinAnsi or MacRoman)
  uses that encoding with the standard face instead of failing. Symbol,
  dingbat and unknown families, and fonts without such an encoding, still
  fail with `font.non-embedded`.

- `font.embedded-unusable` (9.9; ADR 0009): an embedded program that still
  cannot be used after its parser's own repairs (syntax, binding or
  unsupported construct) is replaced by the substitute a non-embedded font with
  the same name and descriptor would get, with the PDF's widths. Strict mode
  keeps the program's error; budget and cycle errors are never replaced.
- `font.widths-length` (Table 109): a `Widths` array whose length disagrees
  with `FirstChar` and `LastChar`; codes past the array use `MissingWidth`
  and extra entries are ignored.
- `font.encoding-name` (Table 112): an `/Encoding` or `/BaseEncoding` name
  that is not a predefined encoding; the font's default encoding applies,
  with any `Differences`.
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
  definition of a repeated glyph name is kept. `font.type1-sparse-subrs`
  (Type 1 chapters 6 and 8) retains unassigned array entries as unavailable,
  preserving declared indexes and original bytes. Strict rejects them. No
  return program is invented; calling a hole remains malformed and follows
  the existing reported `font.glyph-data` fallback in Repair mode. That
  fallback does not reconstruct the missing outline.
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
- `object.stray-delimiter` (7.3.7, 7.3.10): unmatched `>>` or `]` between a
  complete indirect value and its `endobj` are ignored. A closing delimiter
  cannot begin a value, so nothing that could hold content is skipped; any
  other token before `endobj` remains an error.
- `security.permissions-unsigned`, `security.literal-hash`,
  `security.hash-padding`: malformed encryption entries recovered; normal
  authentication is still required.

- `content.graphics-unclosed` (7.8.2, 8.4.2): unmatched saved graphics states
  are discarded at the end of an otherwise finished content sequence.
- `content.marked-unclosed` (14.6): marked-content sequences left open at the
  end of an otherwise finished content sequence are closed there, so optional
  content stays hidden or visible until that end. Open sequences nest without
  a depth limit; each costs one operator of the content.
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

- `content.graphics-unmatched`, `content.text-unmatched` (8.4.2, 9.4.1):
  unmatched `Q` or `ET` leaves the current graphics and text states unchanged.
  These viewing policies report every deviation. Strict rejects them, and
  incomplete operands, text objects, paths and compatibility sections, unknown
  operators, crossed boundaries and resource limits remain errors. Readers may
  disagree on malformed state boundaries.
