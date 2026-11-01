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


## Amendment, 2026-10-29: function work of 2 Gi units

`function_work` defaults to 2,147,483,648 units, matching `decode_work`:
32 units per pixel of a 64 Mi-pixel reference. This raises its previous
50,000,000-unit allowance. The value fits a 32-bit `usize`. The existing
2,000,000,000-unit `render_work` allowance, 64 Mi pixel cap, byte caps and
host frame cap remain unchanged. The fuzz driver explicitly retains its
previous 50,000,000-unit function allowance.

Function preparation reserves its evaluation cost per sample. A boundary
regression checks the largest admitted sample count at the default and
refusal of one more. Newly admitted renders must match the unchanged
renderer with only `function_work` raised; a prior refusal is not a
performance baseline. Rendering arithmetic and per-sample charging are
unchanged.

## Amendment, 2026-10-31: explicit tiled image collections

An image's source coordinate grid and a decoded raster surface are distinct.
`pixels` means width × height of one declared rendered or decoded surface,
regardless of storage layout. Whole-image decode still asks for one surface
and retains its whole-grid gate. An explicit plural-surface operation may
represent a larger source grid only through individually bounded decoded
surfaces and aggregate bounds for the complete collection.

`Limits::tiled_images: TiledImageLimits` adds two `u32` fields:

| Field | Default | Domain |
| --- | ---: | --- |
| `samples` | 268,435,456 | Logical component sample codes represented by one complete tiled image collection, summed once over its tiles |
| `work` | 536,870,912 | Construction/validation/partition/transpose operations of that collection: one unit per sample-code read/write, packed byte read/write/initialization and processed row/tile record |

No meter resets at a tile boundary. This is a deliberate new admission policy,
not resource equivalence inferred from keeping existing numeric caps unchanged.
The existing `decode_work` coded-image/ICC domain is not borrowed for raw tile
construction, and general-filter decoding keeps its own stream rules.

The aggregate packed output, row padding and tile/layout metadata obey
`decoded_bytes`. Retained decoded input, decoder scratch and transient
repartitioning are conservatively included in aggregate live `decode_bytes`
besides the output. Work and capacity reservations use `Meter` before the work
or allocation. Actual retained capacities and all collection metadata feed
Store/render accounting, independent of cache state. Individual tiles obey
`pixels`; the collection cannot materialize an unchecked whole decoded surface.
Original document admission limits remain binding if a source outlives its
document or a later caller supplies larger limits.

The explicit domain group adds eight bytes per owned `Limits` record. Its
reviewed record bound is 264 bytes on 64-bit targets and 156 bytes on wasm32,
replacing the former 256-byte native bound. Tests enforce those exact bounds,
the eight-byte group and exhaustive default addressability. Existing numeric
defaults and custom `usize` domains are unchanged. There is no Clippy threshold
override or blanket lint allowance. Non-storing functions borrow `&Limits`;
constructors whose returned owner retains limits and required `From<Limits>`
conversions retain their ownership semantics. Further record growth requires
another explicit review.

Initial eligibility and exact sampling are specified in ADR 0010. Failed
collection construction returns no partial tiles; preparation defers new pending
decoded-tile publication until success. Unrelated legacy cache semantics remain
unchanged. The representation never permits padding missing samples, ignoring
excess rows or treating a filter repair as a completeness certificate.


### Scene ownership of tiled source roots

The recorder's existing conservative per-reference policy stays in force for
whole images and every per-occurrence Image, Decode, colour-space, Matte and
node record. Explicit tiled mask source roots are a second held-identity
exception beside ICC profiles: one scene pays once for each identical held
`Arc<ImageSource>` root, including its header/data/Admission owners. Equal bytes
in distinct roots still pay separately. Neither Store state nor Arc reference
counts participate in admission.

A per-recording identity index holds the source Arc to prevent address reuse,
reserving its key/handle/map capacity before allocation. Finishing drops and
refunds only the proven temporary index capacity. Every distinct source payload
remains charged once in the finished scene's byte estimate for its full lifetime.
A new recorder pays again. An aborted recording returns no partial Scene; any
held roots and index are dropped with the recorder. Continuing a Device after
an error can conservatively retain earlier charges, as with the profile index;
it never discounts a new source or authorizes partial renderer publication.

## Amendment, 2026-11-01: indexed source ownership and reservations

The optional index adds no numeric quota. In addition to global caller admission, its new operations intersect the source's captured original `render_bytes` and `render_work` allowances. The work allowance covers construction, indexed queries and source-root admission/retrofit scans. It is **not** a retroactive original-source cap on old-route queries, parent work or other pre-existing domains. The byte allowance bounds aggregate indexed/decoded owner payloads, retained capacities and charged owner/publication records across aliases of the same held SourceArc. Caller/original intersections and prospective live peaks are checked before decode/index allocation, then actual returned capacities are rechecked.

A preparation-local bounded inline root ledger holds strong SourceArc identity through its existing owner keys. Equal encoded bytes, equal ImageIds or even a synthetic shared decoded Arc in distinct source roots never establish aliasing. Orientation and validated tile height identify each decoded owner. Root records conservatively aggregate each owner slot/payload; duplicate charging of shared decoded capacities across distinct owner keys is permitted and disclosed, with no unmetered dedup scan or cross-root discount. Store state and Arc reference counts do not affect admission. The temporary ledger and private owner tokens are cleared before PreparedScene publication.

Every active-root comparison is precharged to both the caller and the visited root. Initial owner validation/alias propagation and prior-paint validation/attachment passes are separately pre-admitted before executing. Executed planning, decoding, coefficients, constructor and parent work are never refunded. For a first index the traversal envelope is `32*(owner_entries+prior_paints)+32`, plus actual root comparisons; later aliases/indexes retain their applicable bounds. The constructor's checked bound is `6*logical_bits + 8*vertical_words + 16*rows + 8*blocks + width`; each indexed query reserves 64 times the conservative column count per covered pixel. These are explicit bounds on the actual algorithms, not claims about benchmark speed.

A private typed owner ledger may replace only previously charged **unexecuted mask query reservations**, before rendering. `Meter::replace_reservation` checks old<=used, checked subtraction/addition and the unchanged cap, with no mutation on error. Its public caller contract cannot itself prove unexecutedness; paint's `ReservedMaskQueries` type and call boundaries provide that proof. No memory refund or per-root/tile meter reset is used. Completed sampler precomputation and all constructor/planning work remain charged, including after a later failure.

Real bookkeeping growth is charged and disclosed. Measured native/wasm layouts are `Entry` 104/56 bytes and `HeldTiles` 64/32 bytes. The intermediate packed implementation before indexing had `Entry` 32/20 and `HeldTiles` 56/28; these are not cc99 baseline types, which did not exist there. Actual vector capacity and overlapping growth are reserved before allocation. Native frozen low-budget controls observed +8charged bytes for five single-owner positives and +320 for the forty-use positive; the current wasm controls also passed all nine literal contracts, both backgrounds of all five independent full-pixel cases, and the literal word/range/Fraction controls. Current supported-target layouts and actual charged bytes were measured. No intermediate pre-index wasm byte baseline was measured, so no wasm growth delta is inferred from native results. Those are observations, not universal byte deltas or equivalence inferred from unchanged caps. `Limits` is 264 native/156 wasm bytes versus the actual cc99 baseline of 256/148 bytes, an explicit eight-byte per-owned-record cost.

Pending decoded-tile publication is deferred until successful preparation, and failed collection construction returns no partial tiles. Existing whole-image/font/colour cache failure semantics remain intact; this is not a globally transactional Store or a claim about allocations after PreparedScene creation. Draw receives a fully prepared, bounded index and no mutable source ledger.


These are bounded observations, not a universal-DPI or memory-neutrality claim. An exploratory full-page 72-DPI render of the motivating source passed the default work limit and independently authored clear-region checks; that single diagnostic is not full-public accuracy, compatibility admission or PR qualification. Higher-DPI/default-limit outcomes, clean source CI, full conformance/reference, runtime parity and cost/memory qualification remain separate gates. No numeric cap, authored oracle or tolerance changes with this index.
