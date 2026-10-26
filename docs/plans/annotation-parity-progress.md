# Annotation appearance consolidation

This lane extends the consolidated V01 appearance implementation at `424839e7`.
It produces one appearance stream per annotation through `pagura-appearance`;
the existing interpreter, scene and renderer consume all generated artwork.
No local builds, tests, scans or benchmarks were run (ADR 0031).

## Implemented in this checkpoint

- Shared cloudy effects for Square, Circle, Polygon and FreeText. ISO 32000-2
  12.5.4 Table 169 defines convex curved segments and preserves BS width/dashes.
  Parabolic lobes and their spacing are documented reader artwork. Intensity zero
  keeps the original outline. RD, opacity and BBox clipping retain their roles.
- Common RC/DS rich FreeText and rich Line captions through one bounded inert
  XML/CSS parser and one positioned-text builder. The existing reversible font
  encoder selects explicit document or provider fonts. No DOM, scripts, network,
  external-object fetching, platform font lookup or second renderer is added.
- Inherited sizes/colors/bold/italic, paragraphs and line breaks, alignment,
  margins, first-line indent, letter spacing, scaling, baseline offsets and basic
  decorations; XML text streams, entities and whitespace rules. Normative sources
  are ISO 32000-2 Tables 177/178 and XFA 3.3 chapters 5 and 27; source reference
  text is not redistributed.
- Seventeen cloud controls contain thirteen analytic quadratic fill expectations.
  Rich controls use project-owned Type 3 rectangles with independently specified
  positions and colors. Existing valid cloudy/rich-caption source bytes are
  retained when their outcome changes from refusal to rendering.

## Validation and scope

Hosted compilation, lint, native/WASI and pixel validation are pending at this
checkpoint. Source formatting, changed-module Python lint/type checks and diff
checks are the only local checks. Fixture generation is limited to changed
modules/cases and does not run the engine.

This is not full V01 completion. Rich font fallback lists, RTL/complex scripts,
CID appearance encoding, tab stops/lists/pagination, arbitrary styled-font matching
and rich auto sizing remain explicit. Cloud fills have independent accuracy
controls; curved cloud stroke/dash accuracy needs broader evidence. Wider callout
and rounded-border stroke geometry remains an integration gate. Ordinary page
rendering selects AP/N and AS; a host interaction surface must explicitly select
AP/R and AP/D before those can be called interaction support. Print visibility
belongs to the shared V05 intent integration. V03 retains active media/3D content.

The parent consolidates this lane into retained PR #256. No additional PR is
created per subtype or formatting variant, and no merge is authorized here.
