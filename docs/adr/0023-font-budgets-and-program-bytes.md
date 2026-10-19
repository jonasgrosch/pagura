# ADR 0023: Font budgets, format ceilings and borrowed program bytes

Status: accepted, 2026-10-18. Applies ADR 0016 and ADR 0017 to
`pagura-font`, and keeps the outline type of ADR 0009.

## Context

A review of `pagura-font` found:

- **Borrowed budgets.** Font parsing and outlining were bounded by
  `operations` (content-stream tokens), `objects` (cross-reference entries),
  `value_nodes` (values of one PDF object) and `decoded_bytes / size_of::<T>()`,
  combined through `.min()` chains at 54 sites. Several caps were also
  magic numbers (48, 513, 10, 96, 64).
- **One error kind for two causes.** Every cap, whether a font format's
  own ceiling (48 Type 2 operands, ten nested subroutine calls, 96 stems) or
  a project budget, returned `Limit`. The strict `parse` and `outline` then
  relabelled every `Limit` as `Syntax` (`legacy_error`), so callers could
  not tell "raise the budget" from "malformed font".
- **Copied static data.** `FontProgram::parse(impl Into<Arc<[u8]>>)` copied
  `include_bytes!` data to the heap: about 6 MB for the bundled standard 14
  fonts and 8 MB per CJK font, each byte resident twice.
- **Positions in derived buffers.** Type 1 errors carried offsets into the
  PFB-unframed, hex-decoded or decrypted copies as if they were offsets
  into the program.
- **The raster dependency.** The crate depends on `pagura-raster` for
  `Path`, `PathBuilder`, `Point`, `Segment` and `Transform`. The review asked
  whether an `OutlineSink` trait should replace it.

## Decision

1. **Format ceilings are syntax; budgets are limits.** A limit that a font
   specification fixes is a named constant next to its parser, and a
   program beyond it is malformed: `Syntax`. Composite-glyph cycles and
   arithmetic beyond `f64` range are malformed too. Only an exhausted
   `Limits` budget, and offset overflow, is `Limit` (CONTRIBUTING,
   Conventions 6). The strict entry points are the strict forms of the
   diagnostic-aware ones and return the same kinds; `legacy_error` is gone.
   In repair mode a malformed glyph, including one beyond a ceiling,
   becomes an empty outline with `font.glyph-data`, like any other
   malformed glyph; a budget failure is never repaired.
2. **One `FontBudget`, derived once.** `FontProgram::parse` derives it from
   `Limits` and stores it with the program. Each parse and each outline or
   advance charges its own `Meter`s:

   | Budget | Field | Bounds |
   |---|---|---|
   | work | `font_work` | tokens, operators, validated table entries and outline points of one parse or outline |
   | entries | `font_entries` | glyphs, subroutines, names, cmap and encoding records one program holds; points of one outline |
   | input | `file_bytes`, `decoded_bytes` | the program bytes, a decoded stream or a standalone file |
   | decoded | `decoded_bytes` | each decoded Type 1 copy (framing, hexadecimal, cipher) |
   | strings | `string_bytes` | one glyph name or token |
   | depth | `depth` | nested subroutine calls and composite glyphs |
   | segments | `path_segments` | segments of one outline |

   Work and entries are cumulative over a parse, where they used to be
   counted per table, so the same program cannot be admitted piecewise.
3. **Static program bytes are borrowed.** `parse` takes
   `impl Into<FontBytes>`, an enum of `Static(&'static [u8])` and
   `Shared(Arc<[u8]>)`. `&'static [u8]` converts without copying, so the
   bundled-data call sites stop copying with no change. `retained_bytes`
   counts shared input only: static data is not heap.
4. **Outlines stay `pagura_raster::Path`** (ADR 0009, ADR 0022). An
   `OutlineSink` was considered and declined. The font matrix and the
   composite and seac transforms still need `Point` and `Transform`, so the
   crate would keep the raster types or need its own copies; the renderer
   caches outlines as `Arc<Path>` and would build the same path through the
   sink; quadratics already convert to cubics exactly; and raster depends
   only on `pagura-core`, so the dependency adds build order, not coupling.
5. **Type 1 positions point into the input.** The parser keeps a map from
   decoded offsets back to program offsets: PFB segment starts, the eexec
   section, the cipher's discarded prefix, and, for hexadecimal eexec data,
   every digit pair that does not follow its predecessor. Errors and
   repairs are placed through it.

## Consequences

Each `Limits` field means what its documentation says for fonts too, and a
caller can retry a `Limit` with larger limits knowing that malformed fonts
fail as `Syntax`. Callers that lowered `operations`, `objects` or
`value_nodes` to keep font work small must lower `font_work` and
`font_entries` instead; the fuzz harness does. Rendering in repair mode
draws glyphs beyond a format ceiling as empty, reported glyphs instead of
failing the page. Bundled fonts are no longer resident twice.
