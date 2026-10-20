# Exact rational derivation for the image-direct fixtures

This document describes the existing independent oracle
`tools/fixturegen/paint.py::direct_image_oracle`, and its four callers in
`direct_image_cases`. It is a derivation of the
[declared RGB8 policy](image-direct-policy.md), not an assertion that its
resampling choices are uniquely required by ISO 32000-2. The existing painting
requirements distinguish the PDF image rules in clauses 8.9.5.2 and 8.9.5.3
from the sampling policy in ADR 0004.

## Inputs

All pages are 32 by 32 at 72 DPI. Coordinates below use a top-left device origin.
Placement `(a,b,c,d,e,f)` maps unit image coordinates `(u,v)` to
`(a u + c v + e, b u + d v + f)`. Source row zero occupies the top of the image:
cell `(column,row)` spans `u = [column/width,(column+1)/width]` and
`v = [1-(row+1)/height,1-row/height]`. The fixture writer changes the y direction
when encoding the corresponding PDF matrix; it does not derive pixels by
parsing or rendering that PDF.

| Fixture suffix | Samples | Device placement | Source levels |
|---|---|---|---|
| `binary-halves` | 32×32, gray, 1 bit | `(16,0,0,-16,8,24)` | code `((7x+3y+xy) mod 5) mod 2` |
| `gray-quarter-turn` | 20×24, gray, 2 bits | `(0,10,12,0,4,6)` | code `(5x+3y) mod 4`, inverted Decode |
| `rgb-fractional` | 48×36, RGB, 8 bits | `(-20.5,0,0,13.75,26.25,4.5)` | channel code `(31x+71y+113c+xy) mod 256` |
| `bilinear` | 3×2, RGB, 8 bits | `(24,0,0,-16,4,26)` | the 18 literal codes in `direct_image_cases`, with `/Interpolate true` |

The decimal placement values are dyadic rationals, represented exactly by the
Python values passed to `Fraction`. Source codes are integers. The small RGB
codes, in row-major interleaved order, are
`17,85,170,255,0,120,60,200,30,240,90,10,220,5,140,75,180,35`.
All packed source rows end on byte boundaries. The independent `image.Wire`
writer and `pdf.stream`/`pdf.document` helpers encode those inputs; they do not
compute expected rendering output.

## Area and sampling

Each admitted placement maps the full image and every source cell to an
axis-aligned rectangle, possibly reflected or quarter-turned. For interval
`[lo,hi]` and pixel coordinate `p`, exact overlap is
`max(0, min(hi,p+1) - max(lo,p))`. Rectangle intersection area is the product of
its horizontal and vertical overlaps. There is no polygon approximation.

The first three cases minify. For source-cell intersection areas `w_i` and
decoded integer levels `L_i`, the sampled channel is
`V = sum(w_i L_i) / sum(w_i)`. The denominator is positive whenever rounded
coverage is nonzero. Source cells partition the covered image region, so the
weights are nonnegative and exhaust that region. The binary and rotated gray
cases exercise equal-area two-to-one reductions; fractional RGB exercises
reflection, unequal edge-cell weights, and partial image coverage.

The fourth case magnifies eightfold along each axis. Invert the rational
placement at the pixel center `(x+1/2,y+1/2)`, scale to source coordinates, and
subtract one half to obtain the bilinear lattice coordinate. With integer
floors `(l,t)` and fractions `(fx,fy)`, take the sum of the four neighboring
levels weighted by `(1-fx)` or `fx` and `(1-fy)` or `fy`. Clamp each neighbor
index separately to its source axis. The weights sum to one. This reference
does not claim coverage of the helper's unused nearest-sampling branch.

## Coverage, composition and rounding

For full-image/pixel intersection area `A`, calculate
`C = floor(255 A + 1/2)`. If `C` is zero, emit 255. Otherwise, for each sampled
channel `V`, calculate the exact rational
`M = (C V + (255 - C) 255) / 255` and emit `floor(M + 1/2)`.
The bounds of the convex averages guarantee `0 <= M <= 255`.

The implementation uses `fractions.Fraction` for the geometry, areas, weights,
levels and composition, and integer floor/half-up conversion for final bytes.
It counts exact half-level ties independently: binary-halves and bilinear
include ties, while the two other generated cases do not. These checks describe
the expected model; product agreement must still be measured.

The expected P6 values are unchanged. Exact currently declares byte equality
for these fixtures. Competitive retains its prior one-level allowance for
fractional RGB and equality for the other three. Neither declared bound is a
claim that an unexecuted product run passes; the two modes are compared
separately.

## Limits of the evidence

The generator imports no product code and runs no product or competitor
executable. Its source patterns and rational evaluation are the reference;
agreement with another engine is unnecessary. The metadata binds the inputs,
code, policy and this derivation to the expected file, but is not a theorem
checker, authorship certificate, execution attestation or release-gate receipt.

These four cases do not prove arbitrary affine filtering, JPEG reconstruction,
ICC conversion, masks/matte, alpha accuracy, complex clipping, multiple marks,
transparent groups, tiles or accumulated-error bounds. They provide a narrow
real fixture family for testing reference and execution bindings. New renderer
admission still requires the additional independent evidence in the unified
renderer validation plan.
