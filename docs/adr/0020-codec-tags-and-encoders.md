# ADR 0020: `pagura-codec` owns the `filter.` tags and keeps its encoders

Status: accepted, 2026-10-18. Refines the `pagura-codec` row of
`docs/ARCHITECTURE.md` and applies ADR 0017's tag rules to it.

## Context

A workspace review found two boundary questions about `pagura-codec`.

- **Tag ownership.** `filter.JPXDecode` was defined twice, as
  `pagura_codec::features::JPX` and as a private constant in
  `pagura-image/src/jpx/mod.rs`. The `filter.` area was shared by the two
  crates without an owner, and the unknown-filter tag was built inline. The
  codec's documentation still described DCT, JPX, JBIG2 and CCITT as "not
  implemented yet", although `pagura-image` decodes all four.
- **Encoders without a consumer.** `flate::compress` (zlib) and
  `png::encode` (eight-bit Gray, RGB and RGBA PNG files) are public, tested
  against independent decoders and called only by the codec's own tests and
  examples. The crate documentation claimed that `compress` provided zlib
  output for stream writing, but `pagura-write` does not depend on the
  codec and writes streams unencoded. `docs/ROADMAP.md` lists "PNG output
  (Flate compressor, PNG encoder) for the conversion use case" under
  *Later*. A PNG file is an output format, so the encoder could belong to
  the crate that writes such files.

The encoders are thin layers over code that has to live in this crate:
`compress` shares the RFC 1951 tables, the canonical Huffman code
assignment, the bit packing and the zlib framing with the decoder. The PNG
filters are the PNG predictors' arithmetic (Predictor 10–15, PNG Second
Edition Table 9.1) run forwards, so the PNG encoder adds only chunk framing
and a CRC on top of the predictor code and `compress`.

## Decision

1. **`pagura-codec` owns the `filter.` and `predictor.` tag areas.** Its
   `features` module (`PREDICTOR_DIMENSIONS`, `UNKNOWN`, `DCT`, `JPX`,
   `JBIG2`, `CCITT_FAX`) and `repairs` module, each with `ALL`, are the only
   definitions. A crate that reports one of these tags uses the codec's
   constant: `pagura-image` reports `features::JPX` for JPEG 2000 modes it
   does not implement instead of defining the string again. Its own tags
   stay in the `image.` area.
2. **The encoders stay in `pagura-codec` as public API,** each with a named
   consumer that will call it:
   - `flate::compress`: opt-in `FlateDecode` compression of streams written
     by `pagura-write`;
   - `png::encode`: PNG output for the conversion use case (the CLI's
     `render` command, which writes PPM today).

   This ADR changes neither consumer. Each is wired in its own crate's
   change.
3. **They stay verified like the decoders.** `tools/verify_encoders.py`
   decodes the encoding of every codec fixture with Python's zlib and a PNG
   reader written from the specification. Cargo tests check round trips,
   determinism and budgets.
4. **Revisit when a consumer is built.** If the writer or the CLI ends up
   producing Flate or PNG output another way, the unused encoder is removed
   rather than kept public.

## Alternatives

- *Move `png` to the facade or a future CLI crate.* The consumer does not
  exist yet, so the code would stay just as unused, in a crate another
  change owns. That crate would also need the PNG filter arithmetic, which
  would then be either duplicated or public API of the codec.
- *A `pagura-png` crate.* A crate of about two hundred lines with one future
  dependent, against ARCHITECTURE.md's rule that a crate needs a substantial,
  independently testable responsibility.
- *Delete both encoders until a consumer exists.* This discards verified
  code, its fixtures and its oracle, and the roadmap needs them again.

## Consequences

Every `filter.` tag has one definition. The facade's `pagura::tags`
re-export and its workspace uniqueness test can use `features::ALL` and
`repairs::ALL`. The codec keeps two public entry points without a product
caller; their documentation says so and names the caller. Binaries that
never call the encoders do not link them.
