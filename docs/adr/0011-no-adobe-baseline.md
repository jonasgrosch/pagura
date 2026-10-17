# ADR 0011: Adobe is not a benchmark baseline

Status: accepted, 2026-10-17. Amends ADR 0002.

## Context

ADR 0002 listed Adobe Acrobat among the engines every release comparison
must include. Adobe's terms do not permit driving Acrobat or Acrobat Reader
from scripts or robotic process automation, apart from Acrobat Pro's
user-initiated Action Wizard on a client machine, and Reader has no
command-line page-to-image export. A benchmark harness therefore cannot run
Adobe's renderer without a separate commercial agreement.

## Decision

Adobe is removed from the required engines of the release gate and from the
release matrix. The required baselines are PDFium, MuPDF, pdf.js, Poppler,
hayro and Apple CoreGraphics. Manual visual comparison in Acrobat Reader
remains a legitimate investigation aid; it is never automated and never
produces timing evidence.

## Consequences

The gate no longer reports an unsatisfiable Adobe blocker. Should a licensed,
automation-permitting Adobe integration become available, adding it back
requires a new ADR.
