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

The TrueType Reference Manual, OpenType/ISO 14496-22 (including CFF2),
TN 5015 supplements such as Type 1 lenIV -1, and TN 5040 PFB packaging remain
**pending primary-text review** because their texts are unavailable. The
Type 1 book verifies binary/hex eexec, both ciphers, and lenIV 4/0/1.
No external known-answer font dataset has been used.

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
  mandatory argument fallback for unrecognized OtherSubrs, hint replacement, and StandardEncoding seac. Hint replacement consumes its
  operands and returns the requested subroutine number without applying hints.

All font syntax errors, including exceeded font budgets, carry a
`Position::Buffer`. For decrypted Type 1 charstrings the position refers to
the decrypted charstring buffer. `Limits::objects`/`value_nodes` bound glyphs,
indices and metadata; `file_bytes`/`decoded_bytes` bound input and tables;
`string_bytes` bounds names; `operations` bounds interpreted tokens;
`path_segments` bounds output and composite expansion. Type 1 and CFF1 subroutine calls use
`min(Limits::depth, 10)` (Type 1 6.4/8; TN 5177 Appendix B). Other graph
recursion and CFF2 use `min(Limits::depth, 64)`. Operand ceilings are
`min(value_nodes, 24/48/513)` for Type 1/Type 2/CFF2, respectively; CFF1
DICTs also use 48 (TN 5176 section 4). CFF1 charstrings have a 65,535-byte
ceiling; Type 1 RD strings and tokens have that ceiling as well (chapter 10).
Type 2 has 96 total stems and 32 transient slots. CFF1 INDEX's 16-bit count
bounds subroutine arrays to 65,535 entries, within Appendix B's 65,536
interpreter limit. Project byte, value, operation and segment budgets can
further restrict these limits; they never raise a format ceiling. Geometry must remain finite. Cycles terminate with syntax
errors. Unsupported capabilities use stable `font.*` tags.

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
post formats other than 1/2/3, cmap format 8 and TrueType collections, and
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

Static Type 1 extraction recognizes conventional RD/ND/NP and
`-|`/`|-` binary forms; it does not implement a PostScript virtual machine or validate the entire
dictionary-building program, RD definitions, definefont or key placement.
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
unit. A branching-subroutine vector checks aggregate budget exhaustion.


The generated vectors now include every Type 1/Type 2 operator family,
width-detection entry points, local/global bias transitions at 1,240 and
33,900, operand/depth/hint/length boundaries, masked implicit vstem operands,
OtherSubr argument order and lifetime, and nibble real padding. The branching
subroutine vector uses ten nested calls so exhaustion exercises the shared
operation budget rather than violating the format nesting limit first.
