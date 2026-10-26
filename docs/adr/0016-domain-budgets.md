# ADR 0016: One meaning per `Limits` field; domain budgets

Status: accepted, 2026-10-18. Amended 2026-10-19 (signature, function and
traversal budgets), 2026-10-21 (render budgets, ADR 0034) and 2026-10-26
(`function_work` scales with the pixel cap). Refines invariant 3 of
`docs/ARCHITECTURE.md` and ADR 0012.

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

## Amendment, 2026-10-21: render budgets (ADR 0034)

The renderer never read `render_bytes` or `function_work`: functions charged
`operations`, the compositor charged its cumulative work to `raster_work`,
which is documented per fill, and its surfaces and caches each borrowed
`decoded_bytes`. ADR 0034 makes preparation the only fallible step of a
render, so every budget that execution consumes must be reserved before it.

1. **Render budgets.**

   | Field | Default | Bounds |
   |---|---|---|
   | `render_bytes` | 512 MiB | What one render holds besides the values it takes from the `Store`: prepared marks, coverage sources and paint, the band buffers of the page, groups and soft masks, coverage rows and scratch |
   | `render_work` (new) | 2,000,000,000 units | The rasterization work of the marks that meet the region, plus one unit per pixel each of them composites |
   | `function_work` | 50,000,000 units | Function evaluations of one render, in the units of the 2026-10-19 amendment, reserved through the prepared sources that evaluate them |
   | `raster_work` | 2,000,000,000 units | The rasterization work of one fill, as before, and nothing cumulative |
   | `path_segments` | 4,000,000 | The line segments of one flattened path, as before |

   Retained values move to the `Store` under its own capacity (ADR 0012,
   amendment of 2026-10-21), so `render_bytes` no longer covers decoded
   content, images, colour transforms or fonts, only one render's working
   storage. Functions no longer charge `operations`.
2. **Preparation reserves; execution cannot fail.** In source order over the
   marks that meet the region, preparation checks the per-fill budgets and
   reserves `render_bytes`, `render_work` and `function_work` through a
   `Meter` from prepared quantities: coverage cells and rows, covered area
   times samples, evaluations per sample and buffer bytes. Each reservation
   is an upper bound of what execution uses, which tests assert. The first
   failure in source order, of a budget or of validation, is the render's
   error. Execution charges nothing and stops only for cancellation between
   bands, which is not an error.
3. **Render charges do not depend on the `Store`.** A value taken from the
   store costs the render's budgets the same whether it was found or built;
   building it charges its own per-value budgets (`decode_work`,
   `font_work` and the others) when it is built.
4. **Budgets are per render of one region.** A whole page can reach a limit
   that each of its tiles stays below. This is documented, not hidden: for
   one region, charges and the reported error do not depend on band height
   or the store's state, but they do depend on the region.
5. **`Limits` is 256 bytes with `render_work`,** the threshold of decision 2
   of the 2026-10-19 amendment. The next render budget joins a nested group
   of `u32` fields, as the signature budgets did.

Peak memory of one render is therefore bounded by `render_bytes`, plus the
store's capacity, plus any value too large to retain, which its build budgets
bound; this replaces the consequence above that `render_bytes` alone bounds a
page's peak memory.

## Amendment, 2026-10-26: `function_work` scales with the pixel cap

`function_work` kept the 50,000,000 units of the `operations` default it
replaced (amendment of 2026-10-19). At the pixel cap that is less than one
unit per pixel, so it bounded nothing in particular. A full-page shading
whose function costs a few dozen units an evaluation exceeds it at ordinary
resolutions: two pages of a 1,000-page corpus reserved about 249M units at
150 dpi (2.1 and 2.4 million pixels at 119 and 101 units an evaluation)
and were refused, although they render in 2.7 and 0.3 s.

1. **Work budgets scale with the pixel cap.** `function_work` defaults to
   32 units per pixel of `pixels`, the allowance of `decode_work`
   (2,147,483,648 units). This replaces the default of the amendments of
   2026-10-19 and 2026-10-21.
2. **The allowance keeps the protection of the other work budgets.** It is
   chosen so that each work budget bounds a similar worst-case time, not to
   admit particular pages. Measured on an M3 Pro (cold renders, `sample`
   profiles of builds with the budgets lifted):

   | Budget | Measured | Worst case at its default |
   |---|---|---|
   | `function_work` | 3.35 ns per unit (Type 4 functions of a full-page shading) | about 7 s |
   | `decode_work` | 1.65–4.32 ns per unit (JPEG 2000) | 3.5–9.3 s |
   | `render_work` | 0.4–2 ns per unit (ordinary pages) | up to about 4 s |

   A reservation can exceed what execution evaluates, where identical
   inputs share an evaluation (0.14 ns per reserved unit on the second
   page), which only lowers the time a reservation stands for.
3. **Every default still fits a 32-bit `usize`** (decision 3):
   32 × 2^26 = 2^31. A unit test states the rule.
