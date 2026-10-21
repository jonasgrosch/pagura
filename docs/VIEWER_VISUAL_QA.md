# Viewer visual checks

The viewer uses shared 32px controls, consistent text baselines, neutral pointer
states and visible keyboard focus. Sidebar tabs use a compact 28px height.
Search groups wrap together. The sidebar overlays pages below 640px; a fresh
compact session starts with it closed. Diagnostic details remain in a dialog.
Form fields retain the PDF artwork at rest; text focus adds an inset underline,
and buttons retain their original marks with separate interaction cues.

Benchmarks: Apple's [toolbar guidance](https://developer.apple.com/design/human-interface-guidelines/toolbars)
and [Preview guide](https://support.apple.com/guide/preview/get-started-prvw846b61d3/mac),
plus visual inspection of the local Preview window. This is a scoped visual
comparison, not a claim of feature or rendering parity.

## Repeatable browser checks

Use `python3 tools/serve_web.py --port 8877 --fixtures` with the hosted WASM
artifact, then `http://127.0.0.1:8877/?demo=form`. Keep existing user drafts open;
use a separate temporary tab for tests.

| Widths / state | Check |
| --- | --- |
| 1280, 980, 811 | Toolbar, search, page controls, sidebar alignment; no horizontal overflow |
| 640, 480 | Search wraps by group; sidebar overlays rather than squeezing the page |
| 360, 320 | Navigation and actions wrap; menus, diagnostics and password dialog fit |
| Pointer / keyboard | Hover, pressed, selected, disabled and focus states; Escape restores focus |
| Forms | Focus does not move text; checkbox toggles synchronously; edits survive save/reopen |
| Thumbnails | Match page artwork and drafts, rotate together, remain quiet during input |
| Contents / search | Selection distinct from focus; nested disclosures; disabled arrows with no matches |
| Small page / fit / 200% | Border weight, original marks and draft alignment remain proportional |

These widths and chrome states were manually exercised on 2026-10-21. Search,
outline, cache/thumbnail cancellation and draft alignment also have hosted JS
regressions. Browser screenshots are local evidence under `target/`, not checked
in; private PDFs are never fixtures or uploaded to CI.

## Small-scale rendering

The Anbieterwechsel example enables PDF stroke adjustment on its printed box
outlines. Direct rendering at reduced resolution snaps them to whole device
pixels, disproportionately darkening small boxes. Do not disable PDF stroke
adjustment or strip borders to conceal this. Page tiles and form artwork share
a bounded finer raster, which the browser reduces to CSS size; thumbnail rasters
are likewise downsampled. Retained tile and thumbnail budgets are unchanged.
Extra density increases initial raster work; checkbox input still only switches
prewarmed canvases. Background thumbnail chunks stop between requests when input
starts; an executing host request cannot be interrupted.
