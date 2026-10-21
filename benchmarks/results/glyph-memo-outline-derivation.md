# Independent two-source glyph fixtures

Status: authored and statically reviewed before measurement. A separate retained
structural diagnostic subsequently verified the compiled root/source/group and
ordinal shapes. The frozen pilot and confirmation verified all outputs in both
modes. See [the experiment record](glyph-memo-experiment.md) for measured scope
and limitations. The derivation below is independent of those outputs.

`glyph_memo_fixtures.py` supplies three cases using the existing fixture writer:

- `outline2`: 64 root text runs, each with 64 alternating L/T placements and two
  source outlines; explicitly `/TK false`, quarter-opacity black.
- `outline2-knockout`: identical text and expected pixels, omitting `/TK` so the
  PDF default true wraps each text object in a knockout group. This exercises
  the memo's depth exclusion, not a matched rendering-cost target.
- `outline2-late64`: the same direct text, preceded by 64 nonempty white root
  path paints. The text ordinals start at 64, beyond the fixed memo's capacity.
  The structural diagnostic confirmed one path command per `f` and all glyph
  ordinals outside capacity; matching pixels alone would not establish this.

The font is independently authored, unhinted, orthogonal TrueType. L and T are
single non-self-intersecting polygons, with bbox 500 by 700 font units and
600-unit advance in a 1000-unit em. At 10pt and 72dpi, each glyph is 5 by 7
pixels with a 6-pixel advance. Every coordinate is an integer pixel boundary.
64 glyphs fit each line from x=16 through x=399; 64 lines start at y=16 with
14-pixel spacing, ending at y=905 within a 512 by 1024 page.

The RGB oracle independently partitions L into a 1x7 left stem and 4x1 foot,
and T into a 1x6 stem and 5x1 top bar. Partition interiors never overlap, nor
do neighboring glyphs or lines. Interior coverage is exactly one and exterior
coverage exactly zero; boundaries have zero area. Black at alpha 1/4 over an
opaque white backdrop yields linear blend 3/4, encoded with the existing
renderer policy as RGB8 191. The current Competitive byte-alpha route uses
alpha byte 64, also producing 255-64=191. This oracle was frozen before
measurement. Output alpha
must be exactly 255 separately; an RGB PPM alone cannot prove alpha.

The default-knockout case has only disjoint glyph coverage, so text knockout
does not alter the analytically expected pixels. The 64-prefix control paints
white at full opacity over white, in y=[0,1), away from text, and thus also
leaves expected pixels unchanged. It adds real work and is only a disabled-path
control, not a time-matched baseline against `outline2`.

Integration uses the repaired `benchmarks/paired_render.py` and identical
`scene_index_profile.rs` on both sides. `glyph_memo.py` dispatches these three
cases and legacy fixtures, with all fixture modules explicitly hash-bound.
The source digest also binds tools/fixturegen/font.py, pdf.py and model.py.
The frozen 34-cell matrix retains the old 24 cells, these six new full-render
cells and four clip/image controls. The helper adds no policy or pass thresholds
and does not change the original oracle/tolerances.

The fixture deliberately reduces source count from the legacy stress case's
64 sources to two ordinary outline shapes. It is not a typical-document or
natural-language workload claim. The existing opaque64 and default-knockout64
cases remain important disabled-route protection; one-source64 isolates the
minimum-source guard. Full, cold, first-tile, and fully tiled legacy target
cells separate internal band reuse from compile cost and external tile calls.
