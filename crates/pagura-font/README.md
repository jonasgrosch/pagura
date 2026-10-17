# Font programs

`FontProgram` reads decoded embedded-font bytes without PDF dictionaries,
host access, hint execution, or external dependencies. ISO 32000-2 clause 9.9
and Tables 124/125 identify the font containers and PDF embedding rules.
Only those PDF rules and the program-facing rules in 9.6/9.7 have been checked
against the local sponsored EC3 text. Internal format interpretations remain
**pending primary-text review** of Adobe Type 1 Font Format (and Supplement
5015), Adobe Technical Notes 5176/5177, the TrueType Reference Manual, and
OpenType / ISO/IEC 14496-22:2019. Independently authored binary fixtures and
analytic geometry do not verify those external specifications. The secondary
comparisons below are also not primary-text review; no external known-answer
font dataset has been used.

The public interface returns glyph-space paths, horizontal advances, font
matrices, name and CID mappings, cmap selections, and built-in encodings.
Bytes and parsed metadata are shared across clones. Outlines are decoded on
demand; they are not cached. Parse success validates the container and mapping
tables, while `outline` validates the requested glyph and its dependencies.
`advance` returns `None` if a charstring cannot be interpreted; `outline`
provides the corresponding structured error. OpenType advances come from
`hmtx` when present, including the shared final advance for trailing side
bearings. Per ISO 9.9.1 Table 124, OpenType CFF requires CFF and cmap without
requiring glyf's header/metrics tables; absent horizontal tables use CFF widths.
PDF `(3,1)` cmap selection is distinct from `(3,10)` and platform 0, as required
by 9.6.5.4. `has_cmap_subtable` reports the exact selection's usable presence.
The generic reader permits cmap-free TrueType for CID use. The PDF font loader
must enforce simple/CID-specific cmap rules, Type 1 binary eexec and Length1/2/3,
stream subtype compatibility and subset naming. Hex PFA/PFB fixture success
is not a claim that these wrappers are permitted PDF embeddings.

Implemented formats:

- TrueType and OpenType `glyf`: short/long `loca`, simple contours, repeated
  point flags, implied quadratic points, affine composite components and
  point attachments; cmap formats 0, 2, 4, 6, 10, 12 and 13; post formats 1, 2 and 3.
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
  hint replacement, and StandardEncoding seac. Hint replacement consumes its
  operands and returns the requested subroutine number without applying hints.

All font syntax errors, including exceeded font budgets, carry a
`Position::Buffer`. For decrypted Type 1 charstrings the position refers to
the decrypted charstring buffer. `Limits::objects`/`value_nodes` bound glyphs,
indices and metadata; `file_bytes`/`decoded_bytes` bound input and tables;
`string_bytes` bounds names; `operations` bounds interpreted tokens;
`path_segments` bounds output and composite expansion. Recursion is bounded by
`min(Limits::depth, 64)`, with format stack limits of 24 (Type 1), 48 (Type 2)
and 513 (CFF2). Geometry must remain finite. Cycles terminate with syntax
errors. Unsupported capabilities use stable `font.*` tags.

TrueType component offsets use unscaled translation when neither offset flag
is present; explicit scaled/unscaled flags take precedence. Quadratics are
converted exactly to cubic Béziers. Hints and instruction bytes are validated
for extent and skipped. Type 1 and Type 2 flex always retain both curves.
Type 2 `random` uses the documented deterministic choice 0.5; its external-format semantics remain pending primary-text review.

Remaining coverage: nondefault variation instances, CFF2 Font DICT matrices,
post formats other than 1/2/3, cmap format 8 and TrueType collections, and
arbitrary PostScript evaluation or custom OtherSubrs. CFF2 Font DICT matrices
are explicitly unsupported after a secondary outline disagreement, pending
primary review. Format 14 UVS records do not change this API's single-code PDF
selection: 9.6.5.4 maps one code through a named base subtable, and 9.7.4.2
selects CIDs/GIDs. A fixture includes a nondefault UVS mapping and confirms
that it does not replace the base glyph.
Static Type 1 extraction recognizes conventional RD/ND/NP and
`-|`/`|-` binary forms; it does not implement a PostScript virtual machine.
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
FDSelect 4 and FD index 256. Neither comparison verifies external standards.
No external known-answer font dataset has been used.

For fixed stack/depth ceilings, outline work is linear in executed tokens
and expanded path segments. The shared budgets include subroutine and seac
expansion; arithmetic maintains a finite-stack invariant without rescanning
all operands after every operator. Independent 64/128/256-line vectors with
and without subroutines assert exact work/segment bounds and reject one fewer
unit. A branching-subroutine vector checks aggregate budget exhaustion.
