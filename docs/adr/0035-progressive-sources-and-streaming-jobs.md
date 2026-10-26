# ADR 0035: Progressive sources and streaming jobs

Status: proposed, originally 2026-10-23; reconciled against `render/one-pass` at
`19059e68448288ccad84a3bc2f4c6c69c5142e2f` for the competitive plan.

This is a design proposal, not an accepted change to ADR 0034 or an
implementation dispatch. The [architecture and implementation plan](../plans/async-pdf-loading-and-image-streaming.md)
defines APIs, scope, work packages and validation. It implements the detailed
D01/D02a slice of the [consolidated parity plan](../COMPETITIVE_PARITY.md).

## Context

ADR 0034 establishes one interpreter, complete device-independent scenes,
source-ordered preparation and infallible band execution. The branch already
has a shared rendering Store and lazy image sample decoding. Its document
model still requires complete contiguous bytes, requested page access walks
the complete tree, and some lazy caches permanently retain failures. The band
executor already serves Pixmap collection and public fallible band callbacks.
The CLI streams PPM through that callback; stop callbacks operate between bands,
but each call restarts iteration. Persistent pause/resume is absent. PNG and
DEFLATE encoding exist as whole-buffer APIs.

Applications need async range loading, independent image extraction and image
responses that begin before the whole PDF or final pixel surface exists.
Async function syntax alone does not establish source consistency, make CPU
work yield, or provide streaming output. Those contracts need explicit state
and ownership while preserving the accepted rendering architecture.

## Proposed decision

### 1. One immutable source model

Add complete-buffer and sparse implementations of a `ByteSource` in
`pagura-document`. A source has fixed versioned identity and total length;
checked inserts fill holes without changing accepted bytes. Reads return
shared byte leases or bounded required ranges. The engine performs no network
or filesystem I/O. Whole-source APIs and revision prefixes become explicit
materialization/views rather than pretending a sparse file is one slice.

Use bounded slice windows with absolute source positions. The parser must
distinguish an open window from actual EOF and malformed syntax, without a
virtual source call on every byte. Existing complete-input convenience APIs
adapt this same model.

### 2. Availability is control flow

Propagate an explicit `ReadFault::Unavailable(NeedData)` separately from terminal
`Error` through source-dependent operations. Public owned jobs return wait,
yield, ready, cancelled or terminal failure. Unavailability cannot become a
repair, undefined-reference null, terminal cached failure or successful partial
scene. Publish completed cache values and ordered diagnostics only; keep the
Store generic over builder failures without making core depend on document
loading.

### 3. Load requested dependencies and declare validation scope

Ordinary PDFs bootstrap from the header/tail, resolve the newest authoritative
cross-reference chain, then load security, catalog and the selected page.
Linearization provides a validated initial-page optimization and read hints;
it cannot override the source version or appended revisions. Unknown index
entries remain distinguishable from entries proven absent/free.

Provide lazy page lookup with bounded inheritance/count handling and batched
dependency frontiers. The existing interpreter remains the authority for used
resources. Global reconstruction explicitly requests complete input.

Preserve complete-document validation as a mode. Requested-operation mode
validates the authoritative index and every consumed dependency while allowing
unrelated object-header checks to remain deferred. Readiness reports the scope;
full/sparse equivalence is tested within one policy. A valid rendered page does
not certify all objects in the file.

### 4. Separate extraction meanings

Distinguish source assets, page-resource inventories and interpreted image
occurrences. Export modes are exact stored stream, unwrapped encoded asset,
normalized isolated-image PNG, and actual page-crop PNG. Use source-versioned
asset IDs and resource-context-aware normalized-output keys.

Share image description, colour and mask resolution with the existing
interpreter. Normalized image output uses the same scene/preparer. Standalone
JPEG passthrough has explicit eligibility; a PDF image stream is not presumed
to carry browser-equivalent colour, decode or mask semantics. Inventories emit
independent discoveries and an explicit terminal completeness record.

### 5. Preserve preflight; expose final bands

Extend the existing band driver with an owned render session holding a prepared
scene, scratch and persistent global-band cursor. It performs no source reads
after preparation. Reserve output slots before execution and expose
representation/row metadata. Current collection, callback and stop APIs adapt
the same cursor with their existing per-call semantics; pausing a new owned
session does not restart it.

Keep scene recording atomic and complete preparation before committing image
output. Cancellation/pausing occurs between bands. No new progressive graphics
route or approximation is introduced.

### 6. Extend existing encoders

Make the existing PNG filter and DEFLATE implementations incremental with
bounded rows/history/blocks/chunks. One PNG has one zlib stream across its IDAT
chunks. Canonical block/flush choices are independent of band height and output
demand. Preflight encoding storage/work and a conservative output bound before
headers. The complete-buffer APIs collect the same streaming implementation.

### 7. Async at application boundaries

A native async adapter awaits source ranges and downstream demand and admits
CPU steps to a bounded pool. Input/output waits retain no worker permit or
engine lock. Its stream yields encoded chunks under backpressure and propagates
disconnect cancellation through supported job boundaries.

Place the concrete HTTP/runtime integration in its own native workspace,
depending on the public facade. Runtime, HTTP and TLS dependencies do not enter
engine product crates or the WASI command; `tools/policy.py` is not weakened.
Their integration/deployment review is explicit. No product dependency
exception under ADR 0008 is proposed.

Protocol v2 carries source supply, jobs, NeedData, pull/ack and cancellation.
A wait completes the current request, allowing the serial host to accept its
missing input. Preserve v1 complete-buffer behavior. Browser fetching stays in
JS; this work does not require the separately planned JSPI transport.

### 8. Budgets and compatibility

Give loading, extraction, encoding and aggregate host queues their own
documented budgets. Amend ADR 0016 where required, respecting its record-size
constraint and one-meaning-per-field rule. Account for source leases, active
store values, image copies, scratch and transport buffers together.

This proposal permits a pre-release API migration for source-dependent return
types and whole-file borrowed slices. Keep complete-input convenience entry
points and wire v1 adapters, with one shared implementation underneath.

## Options considered

| Option | Tradeoff |
|---|---|
| Explicit availability and resumable jobs | More deliberate source/error API work; supports native async servers and capability-free WASI through one engine |
| Async through every parser and kernel | Suspension spreads through CPU code, but CPU admission and stable data ownership still need separate solutions |
| Blocking network-backed Read/Seek | Simple experiment; occupies CPU workers during I/O and complicates serial WASI supply and cancellation |
| Complete download and full output buffering | Existing compatibility path; cannot provide early dependency-based work or streamed pixel memory bounds |

The proposed choice is explicit availability with owned jobs and async adapters.

## Consequences

- Useful pages and independent images become available before unrelated data.
- Existing exact rendering and source-ordered semantic failures are preserved
  within an explicitly declared validation scope.
- The source/error API migration reaches document consumers, historical views,
  extraction and complete-source editing/signature callers.
- A streamed image can fail at transport after its headers have been committed;
  success requires its explicit terminal encoding/transfer state.
- Partial codec decoding, incomplete-scene rendering, object-store eviction,
  revision overlays, JSPI and runtime-specific engine dependencies stay outside
  this release.
- The implementation is complete only after the plan's parity, lifecycle,
  resource, HTTP and performance gates pass. The remaining source/job/encoder
  changes are pending; existing band streaming is preserved, not reimplemented.
