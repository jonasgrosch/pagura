# ADR 0027: Image codecs in `pagura-imagecodec`

Status: accepted, 2026-10-18. Amends ADR 0010 (where the decoders live).

## Context

`pagura-image` held two responsibilities. About 11.8k of its 12.7k
production lines were image codecs that know nothing about PDF: JPEG 2000
(4.9k), JBIG2 (4.9k), DCT (1.3k) and CCITT fax (0.7k). The rest read image
dictionaries and inline images, applied Table 87 and placed errors in the
document. The codecs reached the PDF side only through three
dictionary helpers and the crate's tags, but nothing kept them from doing
more, and the two halves had grown apart:

- the same coding machinery existed twice inside the crate: two MQ
  arithmetic decoders (T.88 Annex E and T.800 Annex C specify one coder),
  two T.4/T.6 run-length and mode decoders (CCITT and JBIG2 MMR), five
  budget counters and six bit readers;
- `CcittParams` parsed a PDF dictionary inside the fax codec;
- the codecs could be tested and fuzzed only through a crate that depends
  on `pagura-document`, and the fuzz driver reached them through the PDF
  crate's public modules.

ARCHITECTURE.md allows a new crate for "a substantial, independently
testable responsibility with a clear dependency direction".

## Decision

1. **The codecs move to `pagura-imagecodec`,** which depends only on
   `pagura-core` and on `pagura-codec` for the `filter.JPXDecode` feature
   tag that JPEG 2000 reports. Its public modules are `ccitt`, `dct`,
   `jbig2` and `jpx`; they take encoded bytes, typed codec parameters and
   `&Limits`, and return samples with codec-level metadata. Errors carry
   offsets in the codec's input (`Position::Buffer`) or no position.
2. **Shared coding machinery lives once, privately, in the codec crate:**
   `mq` (one MQ decoder for JBIG2 and JPEG 2000), `t4` (one T.4/T.6 row
   decoder over changing-element lists for CCITT and MMR) and `budget`
   (the `decode_work`, `decode_bytes` and `decoded_bytes` meters of one
   decode, ADR 0016). Byte and bit input goes through
   `pagura_core::bytes` (ADR 0017).
3. **`pagura-image` keeps the PDF semantics:** image XObjects and inline
   images (8.9), filter rules (Table 92), masks (8.9.6), `SMaskInData`,
   color spaces and the `DecodedImage` model, with one adapter per codec in
   `adapter/` that maps dictionary parameters onto the codec and checks the
   result against the dictionary. It places codec offsets in the stream
   with `Error::within` and dictionary faults at the image object.
4. **Tag areas follow the split:** `jpeg.`, `ccitt.`, `jbig2.` and `jpx.`
   belong to `pagura-imagecodec`, `image.` to `pagura-image`.

## Alternatives considered

- **Fold the codecs into `pagura-codec`,** which already owns their
  `filter.*` tags. `pagura-document` depends on `pagura-codec` for stream
  filters, so every consumer of documents would build 12k lines of image
  codecs it never calls, and the filter crate would gain a second
  responsibility.
- **Keep one crate with `codec/` and `pdf/` directories.** This fixes the
  layout but not the boundary: module privacy cannot keep the codecs from
  calling PDF helpers again, and testing or fuzzing a codec still builds
  the document stack.

## Consequences

The dependency graph gains one edge chain: `pagura-image` →
`pagura-imagecodec` → `pagura-codec`, `pagura-core`. The codecs are tested
next to their code and can be fuzzed without a document. `pagura-image`
re-exports `JpxColor`, the one codec type in its public API; callers that
need a codec directly depend on `pagura-imagecodec`. A change to the PDF
image layer no longer recompiles the codecs.

The move and the tag modules land first. The decoded image model, the
shared budget, the shared MQ decoder and typed codec parameters, the
shared T.4/T.6 decoder and the bit readers on `pagura_core::bytes` follow
as separate changes, the last two after a benchmark of their hot loops.
