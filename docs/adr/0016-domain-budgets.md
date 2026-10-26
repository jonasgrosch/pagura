# ADR 0016: One meaning per `Limits` field; domain budgets

Status: accepted, 2026-10-18. Amended 2026-10-19 (signature, function and
traversal budgets) and 2026-10-21 (render budgets, ADR 0034). Refines
invariant 3 of `docs/ARCHITECTURE.md` and ADR 0012.

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

## Amendment, 2026-10-26: progressive source and job budgets (ADR 0036)

Loading, extraction and encoding use separate domain configuration records, passed
by reference or owned by their operation. They do not enlarge `Limits`, which
remains at its 256-byte boundary, and do not borrow syntax, image or render fields.
Every allocation/work charge still goes through `Meter` before the work.

`pagura-document::source::SourceLimits` bounds retained source/assembly allocations,
one assembly window, stored chunk records, a returned missing-range frontier and
cumulative source work. `Limits::file_bytes` continues to bound the declared file
length. An accepted allocation owns a reservation; sharing a byte lease shares
that reservation, and the last lease releases it. Closing a source drops its map
but cannot refund externally pinned storage. Failed supply publishes no fragment;
duplicate supply changes no availability generation but consumes its actual
validation work. Materialization reserves peak duplicate storage explicitly.

Loading-job work, extracted records and encoder work/storage have their own
domain meters and do not reset on yield or unavailable input. Prepared rendering
retains its established charges. Host limits bound active/queued jobs, workers,
transport frames, source totals and pinned output slots across operations. A
source or output wait releases CPU admission; the bytes it retains remain charged.
Defaults and adversarial checks accompany each implementation before admission
is connected to a host. Domain configuration is not evidence of a measured bound.

`RenderJobLimits::scene_attempts` (default 64) admits calls to scene recording,
charged before each call, including a scene-cache hit. An unchanged source wait
uses no attempt. Until interpretation itself retains instruction state, changed
availability can replay an unfinished prefix; normal interpreter/font/colour
limits apply per attempt, while the document's source-work charge is cumulative.
This bounds replay explicitly without claiming instruction-level resumption.
The controller's fixed fields are excluded from source/page/render quotas; its
active page, cached dependencies and prepared output keep their existing owners.
