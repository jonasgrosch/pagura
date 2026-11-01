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

## Amendment, 2026-10-31: bounded packed soft-mask collections

Whole-image decoding retains the original whole-grid `pixels` and byte gates
for every layout and codec. Packed storage alone does not make an oversized
`DecodedImage` admissible. A separate explicit plural-surface operation may
return a collection of bounded decoded tiles with original logical source
coordinates. Each tile obeys `pixels`; aggregate logical sample/work/storage
and live scratch bounds follow ADR 0016's `tiled_images` amendment. No budget
resets per tile and no unchecked whole-surface conversion is exposed.

The initial source form is a one-component, one-bit DeviceGray soft mask
without Matte, nested masks or embedded opacity, with unfiltered rows or one
FlateDecode and no DecodeParms. Every required row must be present at its exact
padded length. General filters are applied strictly using shared document
preparation/decryption and the real document limits; repaired filters, predictor
retries, truncated checksums, trailing binary data, short-row padding and
excess-row trimming cannot certify the new representation. Other images keep
their existing whole-grid admission and refusal rules.

Tiles change storage only. Sampling retains the original mask grid, independent
parent/mask dimensions (11.6.5.2), Decode, interpolation, exact minified area
averages, clipping and orientation. Row pad bits are never samples (8.9.3),
including during direct transpose. A footprint crossing tile boundaries is
computed on the common logical grid, never by averaging tile averages. Native
and wasm32 indexing, aggregate work/capacities, Store publication and original
admission-limit retention require explicit checked accounting.

Qualification includes independent bit/pixel oracles, all existing fixtures and
full reference gates, native/SIMD/scalar and band/partition/store parity, exact
source CI, same-input performance and actual surface memory measurements. The
representation policy alone does not claim a newly admitted public page.


Repeated marks may share an explicitly tiled source owner. ADR 0016 specifies
its charged per-scene held-identity index; equal source bytes in distinct roots
are not aliases. Image/Decode/space/Matte/node occurrence charges and every
ordinary whole-image charge remain unchanged. Shared mask storage therefore
cannot authorize its use as an ordinary oversized main image or stencil.

## Amendment, 2026-11-01: optional exact column index

An unmatted tiled Soft mask may use an optional exact vertical word/prefix index for a separable minified area plan. Its parsed raw Decode must have the exact positive-zero/one bit patterns `[+0,1]` or `[1,+0]`; clipped endpoint equality, signed negative zero and approximate equality do not establish eligibility. Other raw intervals and sampling modes keep the complete old route.

The index counts present logical bits only. Each column's integer interior population fits u32 and is exactly representable in f64. Its first/interior/last expression retains the old separated-row reducer's floating-point operation order, including the two-row and one-row cases. Reverse Decode counts interior zeros explicitly. Horizontal reduction, footprint trimming/normalization, compositing and quantization remain unchanged; this creates no additional ADR0004 error slot. Nonzero source row padding and unfilled final vertical word bits never contribute samples.

Index selection is allocation-free. If construction, retained storage or its work is not worthwhile or cannot fit, the old route remains available under its original caller/global admission. Source decode and allocation errors are not caught to attempt another path; there is no post-allocation fallback. An already selected index remains subject to its cumulative source/caller allowances on later aliases/queries.
