# ADR 0006: Learn from other engines; never copy them

Status: accepted, 2026-10-17. Refines ADR 0001.

## Decision

Pagura is an independent implementation: it does not fork, vendor, depend on,
copy, translate or closely paraphrase code from another PDF engine. Within that
boundary, other engines are a resource to learn from and to outdo.

**Allowed**

- Reading other engines' source code, tests, commit history, issue trackers
  and design documents to understand real-world edge cases, recovery
  strategies, performance techniques and architecture trade-offs.
- Using their test corpora as input documents, and their outputs as
  differential evidence: where engines disagree, investigate.
- Adopting an *idea* — a recovery heuristic, a data-layout choice, a caching
  strategy — after understanding why it works, then designing Pagura's own
  version that fits its architecture, consolidates what several engines do,
  and improves on them where possible.

**Not allowed**

- Copying code, including small snippets, or translating it into Rust.
- Reproducing code structure closely enough to be a derivative (line-by-line
  ports, mirrored function sequences, copied tables of magic constants without
  an independent source such as the standard).
- Treating another engine's behaviour as normative. The standard decides;
  where it is silent, Pagura documents its own choice (ADR 0003).
- Extra care for copyleft engines — MuPDF and Ghostscript (AGPL), Poppler and
  xpdf (GPL): study behaviour and ideas, never even small fragments of code.

## Practice

1. Understand the behaviour and write it down in Pagura's own terms: a
   requirement, a minimized fixture with an independent expectation, or a note
   in the task file or ADR.
2. Implement from that description inside Pagura's design.
3. Record inspiration where a non-obvious behaviour came from another
   engine's handling, e.g. "most readers rebuild the table when `startxref`
   points into a stream (observed in pdf.js and PDFium)". Provenance protects
   independence and helps reviewers.

Competitor installations still live outside the repository under
`$PAGURA_COMPETITORS_DIR` (default `~/.cache/pagura-competitors`), so the
product build can never pick them up by accident.
