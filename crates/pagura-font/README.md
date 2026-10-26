# Font programs

`FontProgram` reads decoded embedded-font bytes without PDF dictionaries,
host access, hint execution, or external dependencies. ISO 32000-2 clause 9.9
and Tables 124/125 identify the font containers and PDF embedding rules.
The supported Type 1 behavior has also been checked against Adobe Type 1
Font Format chapters 2/3/5–8/10; CFF1 against TN 5176 sections 2–19 and
appendices A–C/F/H; Type 2 against TN 5177 sections 2–4 and appendices A–C;
and CID selection against TN 5014 sections 2.3/2.4, 3.2 and 4.1/4.2.
The requirement registry records a rule, locator, implementation verdict and
independent vectors for each operator and format item. These are reviews of
the supported behavior, not complete font or PostScript conformance claims.

Selected OpenType GSUB layout and `vert`/`vrt2` behavior have also been reviewed
against Microsoft's primary specification, as recorded by
`font-vertical-substitution`. Post 1/2.5 count and signed-name-index rules were
reviewed against Microsoft and Apple, recorded in `font-program-glyph-names`.
Remaining TrueType Reference Manual and
OpenType/ISO 14496-22 formats (including CFF2),
TN 5015 supplements such as Type 1 lenIV -1, and TN 5040 PFB packaging remain
**pending primary-format review**. The
Type 1 book verifies binary/hex eexec, both ciphers, and lenIV 4/0/1.
No external known-answer font dataset has been used.

The public interface returns glyph-space paths, horizontal advances, font
matrices, name and CID mappings, cmap selections, and built-in encodings.
Bytes and parsed metadata are shared across clones; `FontBytes::Static` data,
such as bundled `include_bytes!` fonts, is used in place without a copy.
Outlines are decoded on demand; they are not cached. Parse success validates
the container and mapping tables, while `outline` validates the requested
glyph and its dependencies. `advance` interprets a charstring only up to its
width, so a fault later in the glyph is reported by `outline` alone; it
returns `Ok(None)` when the program records no advance (CFF2 without `hmtx`).
OpenType advances come from `hmtx` when present, including the shared final
advance for trailing side bearings. Per 9.9.1 Table 124, OpenType CFF requires CFF and cmap without
requiring glyf's header/metrics tables; absent horizontal tables use CFF widths.
PDF `(3,1)` cmap selection is distinct from `(3,10)` and platform 0, as required
by 9.6.5.4. `has_cmap_subtable` reports the exact selection's usable presence.
The generic reader permits cmap-free TrueType for CID use. The PDF font loader
must enforce simple/CID-specific cmap rules, Type 1 binary eexec and Length1/2/3,
stream subtype compatibility and subset naming. Hex PFA/PFB fixture success
is not a claim that these wrappers are permitted PDF embeddings.

`vertical_substitution(script, language)` reads selected GSUB tables lazily and
returns an owned glyph map. It supports single substitution formats 1/2,
coverage formats 1/2 and extension-wrapped single substitutions. Script and
language selection precede `vrt2`/`vert` preference; lookup order and first-match
subtable precedence are preserved. The stored `font_work` and `font_entries`
budgets bound compilation; the map reports retained bytes. This does not shape
strings, alter advances, or affect embedded PDF CID/GID selection. GDEF lookup
filters, other substitution types and feature variations remain explicit gaps.
Synthetic glyph-map, malformed-input and resource-bound controls pass the
[three-platform hosted checks](https://github.com/jonasgrosch/pagura/actions/runs/40385513689)
at `6ac4b1d27`, including the bundled CJK substitution and PDF metrics controls.

Legacy post 2.5 offsets use the existing glyph-name index, including its work
and entry budgets. Signed index bounds and the declared glyph count are checked.
Post 1 requires exactly 258 glyphs. Independent paired PDF controls preserve
cmap-first selection and name fallback. All 13 integrated controls pass at
`6ac4b1d27` in [hosted run 40385518384](https://github.com/jonasgrosch/pagura/actions/runs/40385518384).
The post 2 reserved-index correction passes [all three hosted platforms](https://github.com/jonasgrosch/pagura/actions/runs/40388811513)
at `2d92bdc21`, including the last permitted index and rejection before allocation.

Legacy Apple TrueType post 4 code arrays are admitted after maxp-count and
work-budget validation, so their existing cmap can select glyphs. This does not
synthesize printer-specific names or assert OpenType conformance. Four unchanged
direct-cmap subjects/controls pass with the preceding controls: 17/17 at
`b1fd431a8` in [hosted run 40393132189](https://github.com/jonasgrosch/pagura/actions/runs/40393132189).
Its [three-platform functional run](https://github.com/jonasgrosch/pagura/actions/runs/40393127365)
also passes. No new font selection path or allocation is introduced.

Legacy cmap format 8 reuses the existing bounded sequential-group validator
and binary lookup. It accepts the caller's complete numeric code unchanged;
it does not decode a word stream or pack a Unicode scalar into UTF-16 words.
The bitmap's extent is checked, while its word-decoding semantics remain
outside this API. Microsoft and Apple describe conflicting details, recorded
in `font-truetype`; no heuristic resolves them. Eight preserved diagnostic
PDFs isolate BMP and supplementary numeric mappings with format-12 controls.
All 25 integrated rendering controls pass at `e399327d7` in
[hosted run 40395913198](https://github.com/jonasgrosch/pagura/actions/runs/40395913198).
The corresponding [functional run](https://github.com/jonasgrosch/pagura/actions/runs/40395908628)
passes on Ubuntu, macOS and Windows.

Implemented formats:

- TrueType and OpenType `glyf`: short/long `loca`, simple contours, repeated
  point flags, implied quadratic points, affine composite components and
  point attachments; cmap formats 0, 2, 4, 6, 8, 10, 12 and 13; post names from
  formats 1, 2, 2.5 and 3, with legacy post 4 admission for cmap use.
- Bare CFF and OpenType CFF: checked INDEX/DICT data, ISOAdobe and explicit
  charsets, all 391 standard SIDs, Expert/ExpertSubset predefined charsets,
  StandardEncoding/ExpertEncoding and explicit encodings with supplements,
  CID charsets, FDArray and FDSelect formats 0/3/4 (including FD indices above 255), local/global subroutines,
  Type 2 paths, flex, arithmetic, transient storage, hint masks and seac.
- CFF2: static outlines, implicit subroutine returns, and charstring blends
  evaluated at normalized default coordinates using the variation store;
  Private DICT blends and their inherited variation index. A charstring can
  override that index. Private DICT parsing preserves pre-blend operands and
  evaluates blends before consuming their enclosing DICT operator.
- Type 1: binary/hex eexec, PFB segment framing, encrypted or `lenIV -1`
  charstrings, static dictionaries and encodings, local subroutines, flex,
  mandatory argument fallback for unrecognized OtherSubrs, hint replacement, and StandardEncoding seac. Hint replacement consumes its
  operands and returns the requested subroutine number without applying hints.

Font errors carry a `Position::Buffer` offset into the program bytes. For
Type 1 programs, offsets in the PFB-unframed, hexadecimal-decoded or
decrypted copies are mapped back to the input byte they came from.

Two kinds of bound apply (ADR 0016, ADR 0023). **Format ceilings** are fixed
by the font specifications and named next to their parser: operand stacks of
24/48/513 (Type 1, Type 2, CFF2; CFF DICTs 48/513), ten nested subroutine
calls (Type 1 Appendix 1; TN 5177 Appendix B), 96 stems and 32 transient
slots, 65,535-byte CFF charstrings and Type 1 tokens and RD strings (chapter
10), and seven flex points. A program beyond a ceiling is malformed: a
`Syntax` error, as are composite cycles and arithmetic beyond `f64` range.
**Budgets** come from `Limits`, each used for its documented meaning, and
exhausting one is a `Limit` error: `font_work` bounds the work of one parse
and of one outline (tokens, operators, validated entries, outline points);
`font_entries` bounds the entries a program holds (glyphs, subroutines,
names, cmap and encoding records) and the points of one outline;
`file_bytes`/`decoded_bytes` bound the input, and `decoded_bytes` each
decoded Type 1 copy; `string_bytes` bounds names and tokens; `depth` bounds
subroutine and composite nesting; `path_segments` bounds one outline. A
budget can restrict a format ceiling but never raises it. CFF1 INDEX's 16-bit
count bounds subroutine arrays to 65,535 entries, within Appendix B's 65,536
interpreter limit. Unsupported capabilities use stable `font.*` tags.

TrueType component offsets use unscaled translation when neither offset flag
is present; explicit scaled/unscaled flags take precedence. Quadratics are
converted exactly to cubic Béziers. Hints and instruction bytes are validated
for extent and skipped. Type 1 and Type 2 flex always retain both curves.
Type 2 `random` follows TN 5177 4.4's range (0,1]. Its deterministic
algorithm is a per-glyph 32-bit LCG (multiply 1,664,525, add 1,013,904,223,
wrapping), yielding (state+1)/2^32. The initialRandomSeed numeric bits are
folded by XOR of the high/low words; omitted seeds are zero. The available
texts specify the initial seed and range, not the algorithm. Uninitialized
transient reads return zero, an implementation choice for undefined behavior;
out-of-range index access and nonfinite arithmetic fail. Zero-element roll
does nothing. Deprecated dotsection is accepted as a no-op (Appendix C).
Masks require stems and zero padding bits; declarations follow section 3.1's
order. Composites cannot nest.

Remaining coverage: nondefault variation instances, CFF2 Font DICT matrices,
derived post 4 printer names and unknown post formats, TrueType collections, and
arbitrary PostScript evaluation, hybrid/synthetic Type 1 fonts, synthetic CFF,
CFF CharstringType 1, standalone TN 5014 CIDFont programs or nonhorizontal
Type 1 sbw advances. Unrecognized OtherSubrs use chapter 8’s required
argument fallback rather than invoking PostScript. CFF2 Font DICT matrices
are explicitly unsupported after a secondary outline disagreement, pending
primary review. Format 14 UVS records do not change this API's single-code PDF
selection: 9.6.5.4 maps one code through a named base subtable, and 9.7.4.2
selects CIDs/GIDs. A fixture includes a nondefault UVS mapping and confirms
that it does not replace the base glyph.
Type 1 seac follows chapter 3.1/6.4's explicit origin rule: component
paths already include their sidebearings, so the accent origin is translated
by adx/ady. Its asb is checked against the accent, and the base bearing/width
must match the composite. A nonzero-bearing vector distinguishes origin
translation from subtracting asb twice.

Static Type 1 extraction recognizes RD/ND/NP, the documented `-|`/`|-`/`|`
aliases, and explicitly declared custom names for the same three static
procedures (Type 1 2.4, 7.3 and 10.3). Definitions are matched to a literal binary
reader or dictionary/array assignment, with optional noaccess, bind and access
qualifiers where applicable. They may occur in the cleartext or Private
portion. Comments and whitespace are lexical only; nested or computed
procedure execution is unsupported. Length-delimited binary data stays opaque.
Canonical aliases remain available without declarations for the existing
abbreviated static extraction convention. This is not a PostScript virtual
machine and does not validate the entire dictionary-building program,
definefont or required-key placement.
CFF CID lookup ignores redundant Encoding data, and complete rejection of
noncanonical ROS ordering/metadata, predefined CID charsets and FD dictionary
domains remains a structural-validation gap. Font-owned vectors use
canonical dictionary placement; the shared font writers retain their default
byte contract for text-owned vectors through an explicit conforming=True
option. Migrating those text callers is a prerequisite to tightening these
structural checks without breaking their independently recorded fixtures.
Checksums and TrueType instruction semantics are not validated. These are
font-program APIs; PDF font dictionaries, CMaps and text positioning belong
to the text and rendering layers.

Independent Python writers in `tools/fixturegen/font.py` provide small fonts,
analytic outlines, metrics and mapping expectations. Crate tests consume those
vectors and also exercise every truncated prefix and lowered resource limits.

Secondary evidence (not primary-text verification): read-fonts 0.41.0 agreed
on 668 glyph-name mappings and 1,792 encoding mappings across seven generated
CFF metadata fixtures, normalizing undefined entries against GID 0. It also
agreed on 26 cmap 10/13 lookups; its mapping API does not implement format 2.
The ExpertEncoding gap at code 92 was corrected following this comparison.
Skrifa 0.46.2 agreed on bounds and curve counts of six generated default CFF2
glyphs, including inherited/overridden Private DICT variation selection,
FDSelect 4 and FD index 256. The CFF1 tables have now also been checked against the primary text: all
391 SID names, both 256-code encodings and both complete Expert charset
orders agree. The CFF2 and cmap comparisons remain secondary evidence,
independent of primary-format verification.
No external known-answer font dataset has been used.

For fixed stack/depth ceilings, outline work is linear in executed tokens
and expanded path segments. The shared budgets include subroutine and seac
expansion; arithmetic maintains a finite-stack invariant without rescanning
all operands after every operator. Independent 64/128/256-line vectors with
and without subroutines assert exact work/segment bounds and reject one fewer
unit of `font_work` or `path_segments`. A branching-subroutine vector checks
aggregate budget exhaustion.


The generated vectors now include every Type 1/Type 2 operator family,
width-detection entry points, local/global bias transitions at 1,240 and
33,900, operand/depth/hint/length boundaries, masked implicit vstem operands,
OtherSubr argument order and lifetime, and nibble real padding. The branching
subroutine vector uses ten nested calls so exhaustion exercises the shared
operation budget rather than violating the format nesting limit first.

`parse_with_diagnostics` and `outline_with_diagnostics` accept a core
`Diagnostics` sink. Its mode governs every repair; strict rejection includes
exactly the repair tag refused. `parse` and `outline` are their strict forms
and return the same error kinds: `Limit` for exhausted budgets and
overflowing sizes, so these are never mistaken for recoverable glyph syntax,
and `Syntax` for malformed programs.

In repair mode readable sfnt tables may be unaligned, or their extents may be
clamped to the available buffer. Required headers, glyph count and advances
must still be usable. Missing unused horizontal side bearings are ignored;
missing advances reject. Complete loca entries and cmap mappings survive
truncation. Missing glyphs yield empty paths, never bytes from another glyph.
An unusable cmap subtable is omitted while other subtables remain available.
Legacy `true` signatures are accepted directly; unknown signatures require an
unambiguous complete table set and `font.sfnt-version`. Reserved component bits
are ignored; conflicting transforms make that glyph unusable.

9.9.1/Table 124 supplies the embedding conformance requirement and 9.6.5.4
constrains cmap selection. The TrueType/OpenType internal layouts above still
await primary-text review. Local corpus inspection supports these recovery
choices without establishing format conformance. Type 1 6.3/Appendix 2 and
TN 5177 3.3/Appendix A leave unknown escaped operators without defined
semantics. The documented repair choice ends the current glyph and all its
subroutine callers, closes the completed prefix and ignores the suffix.

For rendering, a successfully recovered unusable glyph is an empty path with
`font.glyph-data`; paint nothing for it and continue PDF text positioning and
other glyphs. Failed composite dependencies discard the entire requested
glyph. Budget failures still propagate. Diagnostics retain the first position
of each repair kind per parse or outline operation, coalescing repetitions.
Type 1 positions are input offsets, as for errors. Bytes shared by the font
remain unchanged.

The caller must create the sink using `Document::recovery()`, call the
corresponding diagnostic-aware method, attach the font stream object to parse
diagnostics when known, and forward `diagnostics.take()` through
`Document::report` even when the operation fails. The font crate has no document
dependency. The text loader and renderer use these diagnostic-aware methods
and forward their diagnostics to the document.


`font.cff-blue-order` admits ordering faults only in unused blue hint metadata.
TN 5176 section 15/Table 23 orders OtherBlues after BlueValues and
FamilyOtherBlues after FamilyBlues. Type 1 5.3-5.5 requires each individual
alignment-zone pair to ascend, while zones themselves need not be globally
sorted. The reader checks pair direction by the sign of the second delta,
without accumulating unused absolute coordinates. Strict mode rejects with
the tag; repair mode reports it and retains unchanged widths, subroutines and
outlines. Original bytes are never sorted or changed. Full validation of zone
cardinality, spacing and BlueScale constraints remains outside this unhinted
API. The corresponding CFF2 recovery is project policy pending primary text.

`FontProgram::retained_bytes()` is computed once after parsing and returned in
constant time. It estimates the shared input bytes (static input is not heap
and is not counted), the Arc allocations, vector capacities, Type 1 decrypted
charstrings with their offsets, encoding arrays, name/CID trees and variation
data. sfnt table slices and CFF INDEX ranges do not duplicate input
bytes. Trees use ADR 0012's 64 bytes of estimated bookkeeping per entry; names
conservatively add their logical lengths, including names core may store
inline. Arithmetic saturates. This is not allocator telemetry, peak memory,
or an exclusive-ownership measurement: clones report the same full shared
estimate. Temporary parsers, aliases, dictionaries, diagnostics and outlines
are excluded, as are this value's inline size and any caller's surrounding
Arc. Callers charge those last two separately and charge a shared program
once, preserving the separate treatment of provider-owned substitutes.


Malformed CFF DICT metadata uses two reported policies (TN 5176 section 4,
Table 6; section 15, Table 23; Appendix H). `font.cff-dict-operands` drops
Private hint entries with invalid operand counts; these hints never affect
this unhinted API. `font.cff-dict-operator` discards a reserved operator slot
and its preceding operands. Recognized operators in the wrong dictionary,
consumed widths/offsets/matrices/random seeds, duplicate known operators and
invalid numbers remain errors. Reserved number markers 31 and 255 have no
defined payload size and are not salvaged. CFF2 operator semantics are not
extended by this policy. DICT diagnostics identify the actual operator byte.

`font.type1-duplicate-glyph` retains the first static CharStrings binding for
a repeated name, including `.notdef` and the empty name. Type 1 chapters 6
and 10 describe dictionary bindings and static organization; they do not
prescribe this first-definition policy. It deliberately avoids interpreting
PostScript replacement assignments. Later binary records are consumed and
their terminators checked, but their glyphs are not retained or decrypted.
The existing `.notdef` normalization and unique glyph indices are preserved.
Token, binary length, work and glyph budgets still apply.

`font.cff-encoding-supplement` drops unusable supplemental encoding records
(TN 5176 section 12, Tables 14/15). A supplement must reference a main-encoded
glyph and an unmapped code. Main mappings and the first accepted supplement
for each code remain intact; subsequent valid records survive an invalid one.
An incomplete supplemental tail is dropped within the available CFF buffer;
no codes or SIDs are guessed. Invalid main encoding data remains an error.
Each policy rejects with its tag in strict mode and reports through the
public parse diagnostics in repair mode, without modifying original bytes.


`pagura-text` reports `font.descriptor-flags` while loading a font, only for
an invalid descriptor flag pair (both Symbolic and Nonsymbolic set, or both
clear) or a missing `/Flags` (ISO 32000-2 9.8.2/Table 121). It preserves Symbolic as the primary reader bit and makes
Nonsymbolic its inverse. A valid pair remains unchanged even if its character-set
metadata disagrees with the program's repertoire or cmap platforms. Such a
disagreement alone does not require deviating from the selection algorithm.

Type 1 selection uses the built-in encoding with any PDF Encoding override
(9.6.5.2). TrueType uses the given flags and Encoding to select the named or raw
routes (9.6.5.4): Symbolic ignores Encoding; absent Encoding also selects raw
bytes. Named selection tries the required cmap and post mappings before the
reader fallbacks permitted by that subclause's final paragraph. These fallbacks
are part of the algorithm, not descriptor repairs. No repertoire heuristic
reclassifies a valid flag or changes a prescribed selection route. ToUnicode
never controls painting.

Strict rejects an invalid pair with the repair tag. Repair creates a bounded
temporary dictionary view, changes only those two bits, and reports the original
descriptor identity and content position through Document::report. Original
objects remain immutable; PDF widths and ToUnicode are preserved. The same view
feeds painting and positioned extraction. CID flag-pair normalization leaves
CID/CMap selection unchanged. Programs are parsed once, without a speculative
glyph decode or repertoire scan. Direct pagura-text PdfFont::load calls retain
strict flag-pair validation.

`FontProgram::glyph_name` looks up a selected glyph's existing program name
without changing selection. It uses TrueType `post`, or a name-keyed Type 1/CFF
charset; unnamed glyphs stay unknown. Names are indexed once, each slot an
entry and each copied name work (`font_entries`, `font_work`), and included
in `retained_bytes`; lookup is constant time and clones
share the parsed index. Repeated post names remain available for each glyph.

Positioned extraction can use that name through the text crate's existing AGL
converter when ToUnicode and encoding mappings provide no Unicode (9.10.2,
final paragraph: reader-selected fallback). This is extraction policy, not a
repair. It runs only during text recording, preserves all existing mappings,
and never changes the chosen glyph, advances or painting. Composite collection
mappings also retain priority. Unknown names and nameless programs remain
unknown; Unicode is not guessed from raw bytes or by inverting cmap tables.

The separate ZapfDingbats glyph-name list applies only to recognized
ZapfDingbats/Dingbats base fonts, including their supported subset/style
spellings (Annex D.6). Other fonts use AGL alone; names such as `a12` do not
acquire a Dingbats meaning. Glyph-name index errors retain the source offset.
