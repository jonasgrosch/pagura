# Tiled image resource policy and API migration

The packed soft-mask collection work adds `Limits::tiled_images`, an explicit
aggregate `TiledImageLimits { samples, work }` domain (ADR 0010 and ADR 0016).
Existing whole-image decode, numeric limits and custom `usize` domains retain
their original meaning. `Limits` grows by eight bytes per owned record: 264 on
64-bit targets and 156 on wasm32. Memory qualification includes that cost.

`Pixmap::from_premultiplied` now borrows its limits. Change the last argument
from `limits` to `&limits`; it still transfers the original pixel allocation
and enforces the same pixel, length and retained-capacity checks. The returned
pixmap does not retain the limit record. Internal non-storing surface/text/fuzz
helpers likewise borrow it. Constructors retaining limits and ownership-based
`From<Limits>` conversions keep their ownership semantics.

`Document::image_stream_with_strict_filters(object, decoded_bytes)` applies
only that image stream's general-purpose filters in Strict mode. It shares
original parameters, source bytes, decryption and document limits with the
existing image stream API, which keeps its behavior. `decoded_bytes` bounds
general-filter output and is clamped to the document's own stream cap. It is
also an allocation hint. Callers must separately establish image-codec eligibility
and exact complete sample rows; source-metadata recovery does not establish
data completeness. This operation never authorizes a filter repair or retries
a predictor.

The tile collection renderer/API and its charged per-scene source index are
implemented on this branch. Only identical held tiled source roots share
source-owner charges within a scene; distinct roots, ordinary images and every
per-occurrence model/Decode/space/Matte/node keep their charges. No
compatibility, performance or memory qualification is claimed by this migration
note.
