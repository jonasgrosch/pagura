# ADR 0014: Text extraction and search

Status: accepted, 2026-10-18. Amended 2026-10-18 (decision 5).

## Context

A reader must let people find, select and copy text. Positions exist only once
a content stream is interpreted: text state and matrices, the CTM, form
XObjects, Type 3 glyphs, clipping and optional content. Pagura already has one
content interpreter, in `pagura-render`. ISO 32000-2 defines what text a page
shows (9.4, 9.10) and how marked content replaces it (14.9.4). It defines no
word, line or reading order for untagged pages, and no search semantics.

## Decision

1. **One interpreter.** The renderer's content interpreter optionally records
   positioned glyphs as a separate output of the same compilation:
   - Unicode;
   - font, size and writing mode;
   - a selection quad in default user space (9.4.4);
   - render mode;
   - optional-content visibility;
   - the marked-content path.
   The display list and rendering output never change, and recording costs
   nothing when it is off. There is no second interpreter.
2. **Layout and search in `pagura-text`**, over immutable glyph records and
   within the document's budgets.
   - **Normative (14.9.4).** ActualText replaces the text of its marked
     content, Alt supplies a word break, and ReversedChars reverses each
     shown string.
   - **Kept as text.** Invisible text (render mode 3, as in OCR layers) and
     clipped text are extracted.
   - **Excluded by default.** Hidden optional content is left out.
   - **Policy.** Words, lines, columns and the reading order of untagged
     content are documented reader policy, with thresholds relative to font
     size. The defaults are 0.2 em for a word gap, 0.35 em of baseline
     tolerance and 1.8 em for a paragraph break.
   - **Order.** Marked content keeps its content order; unmarked blocks
     follow orientation, columns and baselines.
3. **Search** uses the standard library's Unicode case mapping, matches whole
   words on alphanumeric boundaries, folds whitespace and optionally joins
   hyphenated line breaks. A match returns its text range and quads for
   highlighting. Unicode normalization needs data the project owner approves
   first.
4. **API.**
   - `pagura_render::{compile_page_with_text, text_page}` take a document,
     page and optional provider, like `compile_page_with_provider`.
   - `RenderContext::{compile_page_with_text, text_page}` reuse the
     document's fonts.
   - The facade's `text_page` uses the feature-selected data.
   - The CLI has `pagura text` and `pagura find`.
5. **Two views.** Search, selection and copy always use the page text in
   the geometric order of item 2, artifacts included, so tags never change
   what can be found or selected. Tagged pages also get a separate logical
   view for assistive technology, reflow and copying in reading order
   (14.7–14.8):
   - structure order through role maps and the ParentTree;
   - artifacts excluded, structure ActualText and Alt applied (14.8.2.2,
     14.9.3–14.9.4);
   - each character linked to its page glyphs;
   - independent of layout options.

   An unusable or unsupported structure tree leaves the page without a
   logical view and reports `text.structure-fallback`; strict mode rejects
   it. Retained-memory accounting covers both views.

## Consequences

- Text, positions and rendering cannot disagree, because one interpretation
  produces both.
- Extraction needs the page to compile; an unsupported feature on the page
  fails extraction too.
- Bidirectional text, normalization and structure namespaces
  (`text.structure-namespace`) are later work.
- The policy thresholds are tunable; changing them is a documented policy
  change with fixture updates, not a silent tweak.
