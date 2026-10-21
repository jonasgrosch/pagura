# ADR 0012: Bound retained document caches and diagnostics

Status: accepted, 2026-10-17. Amended 2026-10-21 (one store for render-side
values, ADR 0034).

## Context

Per-object parsing and decoding limits do not constrain the sum of a document's
cached values. Many individually permitted strings, decoded object streams or
page dictionaries can exhaust browser memory. Repeated decoding can also report
the same damage indefinitely. Original data, cache state and rendering state
remain separate (ADR 0001, ADR 0004).

ISO 32000-2:2020 clauses 7.3.10, 7.5.7 and 7.7.3.4 describe indirect identities,
object streams and inherited page attributes. Annex C.1 discusses practical
processor limits. The byte and diagnostic limits below are implementation
policy, not numerical limits imposed by the PDF standard.

## Decision

`Limits::document_bytes` defaults to 128 MiB. A document uses one synchronized
counter for these retained allocations:

- Eager slot and object-stream cache metadata, effective trailer and revisions.
  Slot metadata is checked before constructing the slot map.
- Owned allocations of each parsed indirect object, charged once when published
  in its slot. Inline object storage is already covered by the slot estimate.
- Decoded object-stream buffers and their entry vectors, once per stream cache.
  Reconstructed streams use the same accounting and error policy.
- Cached typed pages and their own dictionary copies. A shared resource
  dictionary and its Arc allocation are charged once per ResourceKey. Pages
  with no Resources share one empty dictionary. The parsed source resource
  dictionary remains a separate owned copy and is charged as such.

`Value::retained_bytes()` and `Dictionary::retained_bytes()` define the core
estimate. They include inline storage, owned vector capacities, heap names,
strings, nested values and dictionary key indexes. Indirect references count
only their identities. Traversal is iterative and linear in the direct value
tree. Arithmetic saturates; an overflowing aggregate addition is a Limit error.
Dictionary and document tree indexes use 64 bytes of estimated bookkeeping per
entry in addition to their keys and values. This is an accounting model rather
than allocator telemetry; allocator layouts and alignment can differ.

Parsing and dependency resolution happen before entering cache initialization.
Publication reserves bytes once inside the cache initializer, preserving the
existing dependency-before-lock discipline. Simultaneous requests for one slot
or decoded stream do not double-charge it. A failed retained-memory admission
is cached as a structured Limit error with the relevant object. No partial
object or decoded stream is published.

Page walks hold tentative page/resource reservations. Failure drops those
reservations and returns no partial page list; successfully loaded indirect
objects remain charged. Success commits the complete page list. Repeated access
to the cached result does not charge again. Reservations and counter updates
are synchronized, including rollback while other objects are being loaded.

Document diagnostics retain the first 1,024 distinct identities in discovery
order. Identity is diagnostic kind/tag, object and position; differing messages do not
create additional entries. Opening diagnostics and lazy/external diagnostics
share this cap. `suppressed_diagnostics()` returns a saturating count of report
occurrences omitted after the cap. Duplicates of retained identities neither
allocate nor increment the count. Omitted identities are not stored: repeatedly
reporting one of them increments the count again. Exact counting of distinct
omitted identities would itself require unbounded storage.

`Document::report` exposes this sink to other crates. It evaluates caller
iterators outside the diagnostic lock. Reporting does not authorize a repair:
callers repairing input first use `Diagnostics::repair` with the document's recovery mode
(ADR 0003), so Strict mode can reject before input is repaired.

## Alternatives considered

Per-object limits alone leave aggregate growth unbounded. Eagerly parsing the
entire document would defeat lazy access. Cache eviction would invalidate the
borrowed object API and require a separate ownership design. An allocator-wide
hard cap would need allocator or host integration beyond safe document APIs.
An unbounded diagnostic identity set would merely move the memory problem.
The selected model adds bounded bookkeeping while preserving cached borrows,
thread safety, lazy parsing and explicit errors.

## Consequences and limits

This is a retained-cache budget, not a peak-memory or process sandbox guarantee.
Original input bytes have their separate `file_bytes` limit. Temporary parser,
decoder, decryption, estimator and page-walk allocations keep their existing
per-operation limits and are released when the operation ends. Security-handler
auxiliary state, caller-owned clones/results and rendering caches are outside
this counter. Diagnostic storage has its independent fixed cap. A caller that
needs a tighter browser envelope must also constrain those other budgets and
its host/render caches.

Values parsed before a rejected parent remain cached and charged; successfully
cached values are not evicted to make an earlier failure succeed. Heap estimates
and budget defaults may change deliberately with the representation. Tests
exercise aggregate failures, shared resources, concurrent cache publication,
rollback, diagnostic truncation and cyclic/deep stream Length dependencies.

## Amendment, 2026-10-21: one store for render-side values (ADR 0034)

Rendering kept its reusable values in separate caches with separate bounds:
fonts and glyph outlines in the `RenderContext` under `document_bytes`, the
planes of the last painted image under `decoded_bytes`, Competitive glyph
masks (16 MiB within `document_bytes`), a recycled surface, per-render clip
and path-hull caches and an unbounded glyph-hull map, while the host cached
compiled pages on its own. Several bounds borrowed fields of other meanings
(ADR 0016), nothing bounded their sum, and direct rendering decoded the same
images again on every call.

1. **`pagura_core::Store` owns every reusable render-side value** under one
   byte capacity: recorded scenes (by page and optional-content
   configuration), decoded images (by source, reduction and region), colour
   transforms, font programs, glyph edge setups (by outline and linear
   device map), and prepared functions and shadings. It replaces the caches
   above. A prepared-scene key waits until a viewer benchmark shows reuse.
2. **`get_or_build(key, build)`** returns the shared value stored for a key,
   or runs `build` outside the store's lock. The builder reports the bytes
   the value retains (`Built`), estimated in the spirit of
   `retained_bytes()` above. The value is retained if it fits, after
   evicting the least recently used values that only the store still holds;
   a value that anyone else holds is never evicted and stays counted. A value
   that does not fit is returned without being retained, and a failed build
   stores nothing. Two threads may build one key at once; the first to
   finish is kept.
3. **Keys are identities, never addresses.** A key names object numbers,
   inline-image positions and every parameter the value depends on
   (reduction, region, the bits of a linear map), never a pointer, which can
   be reused once its value is dropped. Each key type names its value type,
   so keys of different types never collide.
4. **The `RenderContext` owns the store and its capacity.** The capacity is
   set when the context is created, and every render of the context's
   document shares the store; dropping the context releases it. The capacity
   is not a `Limits` field, because it bounds what persists across renders,
   not one operation (ADR 0016, decision 6).
5. **No render's charges or errors depend on the store's state.** A build
   charges the per-value budgets of its own work (`decode_work`,
   `decode_bytes`, `font_work` and the others), so a value that a build
   cannot afford is never retained, and a hit returns what a miss would
   build. Whatever a value costs a render's own budgets, it costs whether it
   was found or built, and values taken from the store are not charged to
   `render_bytes` (ADR 0016, amendment of 2026-10-21). Every render that
   shares a store uses the document's `Limits`.
6. **Document-side caches are unchanged in this pass.** Objects, object
   streams and pages stay under `document_bytes` and are never evicted.
   Objects as shared handles that the store can evict are on the roadmap.

One number now bounds what rendering retains between renders, and a viewer
can size it per document. Like `document_bytes`, the capacity is an
accounting model, not a peak-memory guarantee: a value too large to retain
lives only as long as the render that built it holds it, bounded by the
budgets that limited its build.
