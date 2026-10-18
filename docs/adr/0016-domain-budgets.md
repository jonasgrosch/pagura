# ADR 0016: One meaning per `Limits` field; domain budgets

Status: accepted, 2026-10-18. Refines invariant 3 of `docs/ARCHITECTURE.md`
and ADR 0012.

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
