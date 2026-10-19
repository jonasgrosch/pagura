# ADR 0028: Colour management in `pagura-color`

Status: accepted, 2026-10-18. Refines the crate table of
`docs/ARCHITECTURE.md`.

## Context

ICC profile parsing and evaluation (about 1,300 lines) and the CIE
colorimetry of the calibrated spaces lived in `pagura-render`. The profile
code needed nothing from PDF beyond `pagura-core` and MD5, yet it was
reachable only through page rendering. Lab to XYZ, Bradford adaptation and
the XYZ to sRGB matrix existed once for ICC and once for `CalGray`, `CalRGB`
and `Lab`, with different knee tests. ARCHITECTURE already named colour
management as the next crate.

## Decision

1. **`pagura-color` owns colour management without PDF objects.** It holds
   ICC.1:2010 forward transforms (`Profile::parse(bytes, components,
   &Limits)`, `Profile::convert`, `Intent`) and the CIE colorimetry the
   calibrated spaces share (`cie::Calibrated` for `CalGray`, `CalRGB` and
   `Lab`; adaptation; XYZ to sRGB). It depends on `pagura-core` and
   `pagura-crypto` (profile IDs) only, reads profiles through
   `bytes::Cursor` and charges `decode_work` and `decode_bytes` (ADR 0016).
   It owns the `colorspace.` tag area. `pagura-render` resolves colour-space
   objects, caches profiles by object identity in a `ColorContext`, and
   reports evaluation errors in the profile's object.

## Consequences

ICC parsing can be fuzzed and tested from profile bytes alone, and another
crate can adopt it without depending on the renderer. `pagura-render` no
longer depends on `pagura-crypto`.
