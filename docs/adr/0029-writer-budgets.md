# ADR 0029: The writer applies the reader's budgets to the file it writes

Status: accepted, 2026-10-18. Applies ADR 0016 to `pagura-write`.

## Context

`pagura-write` takes a `Limits` like every other crate, but a review found
that it used the fields differently from the parser that reads its output:

- `value_nodes`, "values produced by parsing one object", was one counter
  for the whole document, so a file with more than a million values in total
  could not be written even when every object was small.
- `file_bytes` was compared with the output only after each object had been
  appended to an unbounded buffer, so the output could overshoot it by a
  whole stream, and a failed allocation aborted instead of returning `Limit`.
- `file_bytes` is documented as the maximum *input* size. ADR 0016 forbids
  giving a field a second meaning.

## Decision

1. **Each field bounds the file being written with the meaning the reader
   gives it.** A file that Pagura writes under some `Limits` is one Pagura
   reads back under the same `Limits`, and the writer refuses, with `Limit`,
   what the reader would refuse:

   | Field | Writer's bound |
   |---|---|
   | `file_bytes` | size of the written file |
   | `objects` | cross-reference entries, including the free entries of unused numbers below the largest |
   | `value_nodes` | values of one object, counted as the parser counts them, including the `/Length` the writer adds to a stream |
   | `depth` | nesting of one object |
   | `string_bytes` | decoded length of one string or name |
   | `stream_bytes` | stored length of one stream |

   This is the reader's meaning applied to the same file, not a borrowed
   one, so no field is added. The `file_bytes` documentation should say
   "read or written" when `pagura-core` next changes.
2. **All output goes through one sink** that charges a `Meter` for
   `file_bytes` before it reserves capacity, and grows capacity geometrically
   but never beyond the budget.
3. **Objects are `NewObject::Value` or `NewObject::Stream { dictionary,
   data }`.** New documents use generation 0 only (7.3.10), so the builder
   keeps objects by number. Stream data is stored exactly as given: encoding
   it is the caller's job until the writer gains an opt-in filter, which
   depends on how `pagura-codec` exposes its encoders.

## Consequences

Budgets set for reading also bound writing, with no new knob. A round-trip
test writes objects at each limit and parses them back under the same
limits. Peak memory while writing is the output plus the builder's objects,
both bounded. Writing a file larger than `file_bytes` requires raising it,
which also lets the reader open that file.
