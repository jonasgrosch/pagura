# ADR 0007: Corpus-scored milestones; benchmarks follow capability

Status: accepted, 2026-10-17. Amends the scheduling (not the gate) of ADR 0002.

## Decision

- Work is organized as **vertical milestones** (`docs/ROADMAP.md`), each
  defined by a measurable outcome on pinned corpora rather than by a list of
  spec chapters.
- `tools/scoreboard.py` runs the engine over a corpus and reports, per file:
  opened strictly, opened with repairs (and which), page count, pages
  rendered, and the `Feature` tag of every `Unsupported` failure. The
  aggregate is the milestone metric; the tag histogram is the work queue.
- Corpora: `generated` (in-repo, exact expectations, run by
  `tools/conformance.py`), `upstream` (pinned public test suites) and
  `realworld` (a pinned, hash-listed sample of public real-world PDFs kept
  outside Git). Expanding a corpus is an explicit, reviewed network step.
- Scoreboard reports are committed as summaries with input digests; raw
  per-file output stays in ignored `benchmarks/results/local/`.
- **Benchmarks**: the release gate of ADR 0002 is unchanged. Until M2,
  cross-engine work is limited to the **conversion track** on real
  documents, plus a Pagura-only regression benchmark against the previous
  commit. Raw benchmark samples are no longer committed.
- The conversion track measures what the engine contributes to a
  PDF-to-image conversion. Each adapter times its own stages with a
  monotonic clock — `open` (load and parse), `render` (rasterize to an
  in-memory buffer) and `emit` (encode and write) — and the harness also
  measures the whole process. The engine contribution is `open + render`;
  `emit` and process start-up are reported but excluded from it, so the
  output format is irrelevant. Each engine writes whatever raw format is
  cheapest for it (PPM, PAM or raw RGBA); the harness normalizes outputs to
  RGB only to verify pixels, outside the timed regions. PNG output is a
  later, separate concern.

## Consequences

Progress is visible as numbers that move. The 20% target remains a release
gate; intermediate per-workload ratios are reported, never declared as
meeting it.
