# ADR 0003: Malformed input is repaired only when reported

Status: accepted, 2026-10-17. Amended 2026-10-18 (diagnostic kinds).

## Context

A viewer must open real-world files: broken cross-reference offsets, wrong
stream `/Length`, junk after `%%EOF`, wrong page `/Count` and duplicate keys
are common. Pagura's rule that unsupported or invalid input is never silently
accepted still holds; what changes is that a repair is allowed if it is
explicit, bounded and visible to the caller.

## Decision

- `pagura_core::Recovery` has two modes. `Strict` turns every deviation from
  the standard into an error; it is used for validation, for tests that pin
  conformance, and before an incremental save. `Repair` (the default for
  viewing) applies documented repairs and reports each one.
- Every repair goes through `pagura_core::Diagnostics::repair`, which returns
  the error in strict mode and records a `Diagnostic` otherwise. There is no
  other way to repair input; code review rejects ad-hoc leniency.
- A `Diagnostic` carries `DiagnosticKind::Repair(Repair)` or
  `DiagnosticKind::Information(&'static str)`, a fixed message, a `Position`
  and the object. Both kinds have stable `area.item` tags; `tag()` returns
  either tag and `repair()` returns only a repair tag. Published tag text
  does not change. Strict rejects repairs only; information is permitted in
  both modes and never makes a document a repaired document.
- `font.substituted` and `font.cid-glyph-missing` describe permitted provider
  behaviour (ADR 0009), not malformed input. They are information. Every
  other existing diagnostic remains a repair.
- `Document::diagnostics()` returns diagnostics from opening and lazy loading
  in discovery order. Diagnostic identity includes kind, tag, object and
  position; information and repairs share the same bounded sink (ADR 0012).
- Repairs are bounded by the same `Limits` as normal parsing. Reconstruction
  scans are linear in the file size.
- Where readers disagree (for example which duplicate key wins), Pagura picks
  one behaviour, documents it next to the tag, and always reports it.
  Such choices are security-relevant (shadow attacks) and must be listed in
  `docs/STATUS.md`.
- A document with repairs is never saved incrementally (the original bytes
  are not a valid base revision). Saving it requires a full rewrite.

## Repair catalogue (initial)

| Tag | Repair |
|---|---|
| `file.trailing-bytes` | data after the final `%%EOF` is ignored |
| `syntax.duplicate-key` | the last value of a repeated dictionary key wins |
| `pages.count-mismatch` | the leaves found replace a wrong `/Count` |
| `file.header-offset` | header found after leading junk (first 1024 bytes); offsets shifted |
| `xref.rebuilt` | cross-reference data rebuilt by scanning for `N G obj` |
| `xref.offset-corrected` | an entry's offset corrected to the nearby object header |
| `stream.length-corrected` | stream data ends at `endstream`, not at a wrong `/Length` |
| `trailer.reconstructed` | trailer recovered from the last `trailer` or from the catalog |
| `filter.truncated` | filter input ends before its end marker or checksum; the decoded prefix is kept (incomplete predictor rows stay errors) |
| `filter.checksum-mismatch` | decoded Flate data is kept despite a wrong Adler-32 |
| `filter.trailing-data` | bytes after a filter's end marker are ignored |
| `filter.raw-deflate` | FlateDecode data without the zlib wrapper is decoded as raw DEFLATE |

The catalogue grows with the implementation; every new tag needs a
minimized fixture showing both the strict error and the repaired result.

## Consequences

Callers can show "opened with repairs" and list them. The scoreboard counts
strict-valid and repaired opens separately, so repairs cannot hide conformance
regressions.
