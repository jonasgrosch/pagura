# ADR 0016: One meaning per `Limits` field; domain budgets

Status: accepted, 2026-10-18. Amended 2026-10-19 (signature, function and
traversal budgets). Refines invariant 3 of `docs/ARCHITECTURE.md` and ADR
0012.

## Context

`pagura_core::Limits` is the single source of per-operation budgets. Its
fields were documented narrowly, but the crates used several of them as
general-purpose knobs:

- `operations` ("content-stream tokens per page") also capped CFF and Type 1
  parsing, DCT, JPEG 2000, JBIG2 and CCITT decoding, CMap parsing, text-string
  length, page-text layout and search, and calculator functions.
- `objects.min(value_nodes)` capped glyph counts, and `value_nodes` ("values
  produced by parsing one object") capped the page-tree walk, navigation,
  CMap entries, ICC tags, the calculator stack and page-text item counts.
- `display_commands` capped font-cache entries.
- `decoded_bytes` separately capped the content buffer, the ICC cache, the
  text/image cache and the compositor surfaces, so peak memory was a
  multiple of it; JBIG2 also mutated a copy of it to mean "remaining".
- `raster_work` ("rasterization work per fill") capped JBIG2 pixel work.

Tuning one budget therefore silently retuned unrelated parsers. The visible
symptom: DCT charges 64 work units per 8×8 block of every component in every
scan against `operations`, so a baseline 4:4:4 JPEG above about 16.7
megapixels, or a progressive 4:2:0 photo above about 6 megapixels, failed
with `Limit` although both are far below `pixels`.

## Decision

1. **Each field has exactly one meaning,** the one its documentation states.
   Code never borrows a field for a different budget. A parser that needs a
   budget no field describes adds a field and amends this ADR. Limits that a
   standard defines (the Type 4 operand stack, the CFF argument stack, the
   charstring subroutine depth) are named constants next to their parser.
2. **Domain fields** are added, with defaults that keep today's behaviour
   wherever today's behaviour was intended:

   | Field | Default | Replaces the borrowed use of |
   |---|---|---|
   | `traversal_work` | 1,000,000 nodes | `value_nodes` for page-tree, outline, name/number-tree, structure-tree, annotation and form-field walks |
   | `decode_work` | 32 × `pixels` (2 Gi units) | `operations` in DCT, JPEG 2000 and CCITT, `operations` and `raster_work` in JBIG2; ICC profile parsing |
   | `decode_bytes` | 256 MiB | `decoded_bytes` as a decoder's working memory (JPEG 2000 allocations, JBIG2 "remaining", ICC tables) |
   | `font_work` | 50,000,000 units | `operations` in font-program, CMap and AFM parsing |
   | `font_entries` | 1,000,000 | `objects.min(value_nodes)` and `value_nodes` for glyph, charstring and CMap entry counts |
   | `text_items` | 1,000,000 | `value_nodes` for positioned glyphs, marked-content spans and text objects per page |
   | `text_work` | 50,000,000 units | `operations` in page-text layout, structure and search |
   | `render_bytes` | 512 MiB | the separate `decoded_bytes` caps of the render caches and compositor surfaces, as one aggregate |

   `decode_work` is the one deliberate change: it scales with image area, as
   32 units per pixel at the pixel cap. That admits a progressive 4:4:4 image
   with eight scans at the full `pixels` budget, and fixes the JPEG cap above.
   It remains a bound of the same order as `raster_work`. Text strings are
   bounded by `string_bytes`, not by `operations`.
3. **Every default fits a 32-bit `usize`,** the WebAssembly address width. A
   unit test enforces it.
4. **Charging goes through `pagura_core::Meter`,** created from one field
   with a fixed message, so every budget is checked arithmetic before the
   work or allocation it protects.
5. **`Limits` stays a plain record.** Callers build custom limits with
   functional record update from `Limits::default()`, about 130 times in the
   tests alone. `#[non_exhaustive]` would forbid that syntax outside
   `pagura-core`, and fields are added often, so the struct keeps public
   fields and is not `#[non_exhaustive]`; adding a field breaks no caller
   that uses the update syntax. Revisit before a published 0.1 release.
   Functions take `&Limits`; only constructors that store limits take them
   by value.
6. **Session-wide budgets are not `Limits` fields.** `Limits` bounds one
   operation on one document. The host's open-document count and total
   source bytes belong to a host `SessionLimits`.

## Migration

The fields are added first, in `pagura-core`. Each crate then moves its
borrowed uses to the field in the table in its own change, together with its
`Meter` migration. Until a crate has migrated, its old borrowed use remains
and is the behaviour users see.

## Consequences

A budget can be tuned for one domain without changing another, and the
documentation of each field is complete. `Limits` grows by eight fields; the
grouped table on the type keeps it navigable. Peak memory for one page is
bounded by `render_bytes` rather than by a multiple of `decoded_bytes` once
the renderer charges the aggregate. Realistic-size codec fixtures decoded
under `Limits::default()` are still needed to guard the image budgets.

## Amendment, 2026-10-19: signature, function and traversal budgets

The signature work that landed after this ADR borrowed fields again:
certification path validation (RFC 5280 6, with offline CRLs) charged its
search to `depth`, `value_nodes`, `operations`, `document_bytes` and
`string_bytes` and capped host signature verifications at `operations /
500,000`, so lowering `operations` to harden content streams would silently
have cut path verifications to two. The ASN.1 reader borrowed `file_bytes`,
`depth`, `value_nodes`, `document_bytes`, `string_bytes` and
`decoded_bytes`. Type 4 calculator functions and sampled functions still
borrowed `operations` and `value_nodes`.

1. **Signature budgets are one nested group,** `Limits::signatures:
   SignatureLimits`, with the defaults the borrowed fields had. It also
   takes the RSA fields, which were top-level.

   | Field | Default | Replaces the borrowed use of |
   |---|---|---|
   | `asn1_depth` | 64 | `depth` for the nesting of one ASN.1 encoding (ADR 0015) |
   | `asn1_elements` | 1,000,000 | `value_nodes` for the elements indexed for one encoding |
   | `asn1_bytes` | 128 MiB | `document_bytes` for the element index, `decoded_bytes` for concatenated BER octets and signed-attribute signature input |
   | `path_certificates` | 64 | `depth` for the certificates of one path |
   | `path_work` | 50,000,000 units | `operations` for scanning, comparisons, policy-graph work and signature input bytes |
   | `path_nodes` | 1,000,000 | `value_nodes` for prepared values, branches, results and policy-graph nodes and edges |
   | `path_bytes` | 128 MiB | `document_bytes` for the cumulative allocation estimate |
   | `path_verifications` | 100 | `operations / 500,000` for distinct host signature verifications |
   | `rsa_modulus_bits`, `rsa_exponent_bits`, `rsa_work` | 16,384 / 256 / 64,000,000 | (moved from the top level) |

   One path budget covers a whole path search, including every candidate's
   validation and its CRL processing. `file_bytes`, `string_bytes` and
   `decoded_bytes` no longer apply to ASN.1 values or copied path strings,
   which are charged to `asn1_bytes`, `path_bytes` and `path_work`.
2. **`Limits` stays within 256 bytes,** the size up to which clippy accepts
   `Copy` values passed by value. Converting the remaining by-value
   parameters across every crate is out of proportion, and raising clippy's
   threshold would only defer the question. So a domain with many budgets
   of its own gets a nested group whose fields are `u32`: every default fits
   32 bits anyway (decision 3), and callers widen with
   `pagura_core::convert::u32_to_usize`. A unit test enforces the size;
   `Limits` is 248 bytes with this amendment.
3. **`function_work`** (default 50,000,000 units, the `operations` default
   the renderer borrowed) bounds evaluating functions (7.10) during one
   page's compilation or rendering: one unit per Type 4 operator executed
   and per sample looked up or interpolated in a sampled, exponential or
   stitching function.
4. **The Type 4 operand stack is the constant `Limits::CALCULATOR_STACK`**
   (100 entries, ISO 32000-1 Annex C, Table C.1), not a budget, replacing
   the borrowed `value_nodes`. Standard limits otherwise stay next to their
   parser (decision 1); this one sits on `Limits` because the table above
   used to borrow a field for it.
5. **`traversal_work` also covers signature inspection:** walking signature
   fields and signed revisions, and the DocMDP/FieldMDP change analysis
   between revisions (12.8), which run on the same bounded navigation
   context as outlines and name trees.

