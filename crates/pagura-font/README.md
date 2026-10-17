# Font programs

`FontProgram` reads decoded embedded-font bytes without PDF dictionaries,
host access, hint execution, or external dependencies. ISO 32000-2 clause 9.9
and Table 124 identify the font containers; their internal layouts follow
Adobe Type 1 Font Format, Adobe Technical Notes 5176/5177, and OpenType.

The public interface returns glyph-space paths, horizontal advances, font
matrices, name and CID mappings, cmap selections, and built-in encodings.
Bytes and parsed metadata are shared across clones. Outlines are decoded on
demand; they are not cached. Parse success validates the container and mapping
tables, while `outline` validates the requested glyph and its dependencies.
`advance` returns `None` if a charstring cannot be interpreted; `outline`
provides the corresponding structured error. OpenType advances come from
`hmtx`, including the shared final advance for trailing side bearings.

Implemented formats:

- TrueType and OpenType `glyf`: short/long `loca`, simple contours, repeated
  point flags, implied quadratic points, affine composite components and
  point attachments; cmap formats 0, 4, 6 and 12; post formats 1, 2 and 3.
- Bare CFF and OpenType CFF: checked INDEX/DICT data, ISOAdobe and explicit
  charsets, StandardEncoding and explicit encodings with supplements,
  CID charsets, FDArray and FDSelect formats 0/3, local/global subroutines,
  Type 2 paths, flex, arithmetic, transient storage, hint masks and seac.
- CFF2: static outlines, implicit subroutine returns, and charstring blends
  evaluated at normalized default coordinates using the variation store.
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
Type 2 `random` uses the permitted deterministic value 0.5.

Remaining coverage: expert predefined CFF charsets/encoding and standard
expert SIDs, FDSelect format 4, CFF2 Private DICT blends, nondefault variation
instances, post formats other than 1/2/3, cmap formats other than 0/4/6/12,
TrueType collections, and arbitrary PostScript evaluation or custom
OtherSubrs. Static Type 1 extraction recognizes conventional RD/ND/NP and
`-|`/`|-` binary forms; it does not implement a PostScript virtual machine.
Checksums and TrueType instruction semantics are not validated. These are
font-program APIs; PDF font dictionaries, CMaps and text positioning belong
to the text and rendering layers.

Independent Python writers in `tools/fixturegen/font.py` provide small fonts,
analytic outlines, metrics and mapping expectations. Crate tests consume those
vectors and also exercise every truncated prefix and lowered resource limits.
