# ADR 0012: Bound retained document caches and diagnostics

Status: accepted, 2026-10-17.

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
order. Identity is repair tag, object and position; differing messages do not
create additional entries. Opening diagnostics and lazy/external diagnostics
share this cap. `suppressed_diagnostics()` returns a saturating count of report
occurrences omitted after the cap. Duplicates of retained identities neither
allocate nor increment the count. Omitted identities are not stored: repeatedly
reporting one of them increments the count again. Exact counting of distinct
omitted identities would itself require unbounded storage.

`Document::report` exposes this sink to other crates. It evaluates caller
iterators outside the diagnostic lock. Reporting does not authorize a repair:
callers first use `Diagnostics::repair` with the document's recovery mode
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
