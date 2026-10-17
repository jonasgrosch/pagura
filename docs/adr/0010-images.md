# ADR 0010: Images and image codecs

Status: accepted, 2026-10-17.

## Decision

- **`pagura-image`** decodes image XObjects and inline images (clause 8.9)
  into a `DecodedImage`: dimensions, bits per component, unpacked samples
  (one byte per value up to 8 bits, two for 16 bits), the resolved colour
  space, `/Decode`, masking (stencil, colour key, explicit and soft masks,
  `/SMaskInData`) and `/Interpolate`. It contains independent decoders for
  DCT (baseline, extended and progressive JPEG), CCITT Group 3/4, JBIG2 and
  JPEG 2000; ADR 0008's threshold keeps them in Pagura.
- `Document::image_stream` applies decryption and every filter except a
  trailing image codec, which it returns with its parameters.
- Every decoder enforces `Limits::pixels` and `Limits::decoded_bytes` before
  allocating, and fails with a syntax error rather than inventing pixels for
  corrupt data; recoverable damage is a reported repair (ADR 0003).
- **`pagura-render`** converts samples through the image's colour space,
  applies decode arrays and masks, and paints the unit square through the
  CTM with documented resampling (area averaging when minifying, nearest or
  interpolated when magnifying per `/Interpolate`).

## Consequences

Large images are decoded whole. Streaming or tiled decoding for very large
scans is a later optimization, measured before it is adopted.
