# ADR 0036: Progressive sources and streaming jobs

Status: accepted 2026-10-26 for group 11 of the competitive plan. Source review
starts from `fc09a6b202b53e4faba6d43e96bd20e30623c65d`, including synchronous
document services and complete PNG export; implementation and validation remain
required before the group is complete.

This is the accepted replacement for the progressive-source proposal originally
numbered ADR 0035. The unrelated band-helper draft retains that number, as
documented in section 9; it does not define a second loading contract.

This decision extends the ownership and scheduling boundaries of ADR 0034 while
preserving its interpreter, preparation and exact execution contracts.
The [architecture and implementation plan](../plans/async-pdf-loading-and-image-streaming.md)
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

## Decision

### 1. One immutable source model

Add complete-buffer and sparse implementations of a `ByteSource` in
`pagura-document`. A source has fixed versioned identity and total length;
checked inserts fill holes without changing accepted bytes. Reads return
shared byte leases or bounded required ranges. The engine performs no network
or filesystem I/O. Whole-source APIs and revision prefixes become explicit
materialization/views rather than pretending a sparse file is one slice.

`SourceIdentity` contains an opaque registration ID and a fixed opaque version;
the descriptor also fixes the total length. `ByteRange` is a checked half-open
range with `u64` offsets. A `ByteLease` owns shared immutable backing plus its
extent. Accepted source allocations and assembly windows carry reservations
until their last lease is dropped, including after source close. A duplicate
supply changes neither availability generation nor useful-work counters. A
conflicting overlap poisons that source; an unrelated/stale identity is rejected
without modifying the current source. Transport bytes and accepted source bytes
have separate aggregate accounting at the host boundary.

The current group retains one diagnostic position model: file positions and
`Limits::file_bytes` use target-sized `usize`. Although transport intervals use
checked `u64`, source registration rejects a declared length that exceeds either
the configured file limit or target `usize` before publishing any document or
window. A wasm32 source therefore must stay below 4 GiB. Supporting larger files
on that target requires widening the existing file positions and file-size limit
together; tiny sparse windows do not imply that separate large-file promise.

Full-source materialization returns a checked complete-source lease. It reuses
contiguous backing or explicitly reserves the additional assembly allocation;
it never hides a whole-file copy behind a borrowed-slice accessor. Historical
views restrict the same immutable source to a prefix. Editing and signatures
admit complete-source materialization until their individual range contracts
are implemented, and never treat holes as zero bytes.

Use bounded slice windows with absolute source positions. The parser must
distinguish an open window from actual EOF and malformed syntax, without a
virtual source call on every byte. Existing complete-input convenience APIs
adapt this same model.
Normal stream metadata reads the dictionary and declared terminator in separate
windows; encoded payload is requested only for decoding or extraction. Direct
stream and unusable-Length recovery retain bounded contiguous/terminator scans
and charge them to the same loading allowance. Object waits retain only the
missing-range frontier, suppress unchanged retries, and never hold an object
lock while reading a dependency. Completed object values remain the only
permanent cache entries; source closure is checked before either cache path.

### 2. Availability is control flow

Propagate an explicit `ReadFault::Unavailable(NeedData)` separately from terminal
`Error` through source-dependent operations. Public owned jobs return wait,
yield, ready, cancelled or terminal failure. Unavailability cannot become a
repair, undefined-reference null, terminal cached failure or successful partial
scene. Publish completed cache values and ordered diagnostics only; keep the
Store generic over builder failures without making core depend on document
loading. Slice parsing also keeps typed callback dependencies distinct from
incomplete syntax: a missing indirect stream Length can suspend a parse without
converting an unrelated syntax error, advancing its cursor or publishing repair
diagnostics. The document layer owns those source frontiers; core has no source
or transport dependency.

The two policies are `CompleteDocument` and `RequestedOperation`. Source identity
and policy belong to a loading context; only finalized authoritative xref entries
define absence/free references. Missing bytes cannot enter permanent result
cells. A semantic unit owns provisional diagnostics and reservations, publishing
them once on completion; successful dependencies survive an outer retry. The
unpublished builder keeps one scratch document across security and catalog
waits. It clears pre-authentication or provisional-table caches only after the
corresponding completed stage changes their authority, then reclaims those
charges before the next stage. Terminal failure drops the construction; no
partially accepted state can resume. Global
validation failures do not replace successful dependency values from a requested
operation. Every retry is charged to the same loading-work allowance. The
bootstrap, replacement parser windows and subsequent document readers share one
loading-work owner. It charges before each read and parse; its synchronization
protects only counter arithmetic, never source reads or recursive dependencies.

Semantic diagnostic attempts use the existing bounded document log's identity
and suppression rules. The attempt publishes on success or terminal rejection,
preserving reports already emitted for a completed failure under the old API.
An unavailable dependency discards only provisional reports; it cannot undo
completed object/stream/cache dependencies. Nested callers share a borrowed
attempt reporter rather than installing a thread-local sink or rolling back
another operation's document diagnostics. Typed document views use one Context
operation batch: historical contexts share pending reports but apply their own
recovery policy, and only the initiating document receives the completed batch.
A nested independent operation has its own batch; it cannot drain an outer
pending buffer. Source lifecycle checks guard entry and successful publication.

### 3. Load requested dependencies and declare validation scope

Ordinary PDFs bootstrap from the header/tail, resolve the newest authoritative
cross-reference chain, then load security, catalog and the selected page.
Linearization provides a validated initial-page optimization and read hints;
it cannot override the source version or appended revisions. Unknown index
entries remain distinguishable from entries proven absent/free.

Provide lazy page lookup with bounded inheritance/count handling and batched
dependency frontiers. The existing interpreter remains the authority for used
resources. Global reconstruction explicitly requests complete input.

`PageJob` and complete enumeration drive one owned page-tree cursor. Its frames
retain source addresses and inherited-attribute owners rather than borrowed PDF
values or a thread-local diagnostic sink. Counts are advisory: preceding leaves
are visited structurally before publishing an exact ordinal, without resolving
unused geometry or resources. The job exposes reported and verified totals
separately and stops before later subtrees. Selected pages own their retained
reservation; complete enumeration commits the same charges to the document.

Preserve complete-document validation as a mode. Requested-operation mode
validates the authoritative index and every consumed dependency while allowing
unrelated object-header checks to remain deferred. Readiness reports the scope;
full/sparse equivalence is tested within one policy. `OpenJob::new` requires an
explicit `Validation`; the immutable `Document::validation` result travels with
readiness and reauthentication retains it. `Document::open` always selects
`CompleteDocument` and drives the same Active/Build states. There is no retained
eager xref traversal. The bounded linearization probe uses source windows;
global reconstruction is a one-time complete-materialization stage. A valid
rendered page does not certify all objects in the file.

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

Asset identity is `(source identity, object generation or inline stream/offset)`;
occurrence identity additionally carries a page occurrence, enclosing definition
path and ordinal. Resource-context identity and normalization options form the
export key. Reused forms and pattern definitions do not expand into fabricated
pixel-instance lists. Direct page resource enumeration remains available with
its existing explicit scope; broader resources and interpreted occurrences are
separate scopes over shared traversal/interpretation primitives.

### 5. Preserve preflight; expose final bands

Extend the existing band driver with an owned render session holding a prepared
scene, scratch and persistent global-band cursor. It performs no source reads
after preparation. Reserve output slots before execution and expose
representation/row metadata. Current collection, callback and stop APIs adapt
the same cursor with their existing per-call semantics; pausing a new owned
session does not restart it.

Each job has one terminal state and returns `NeedData`, `Yielded`, an explicit
item, or explicit completion. Cancellation is terminal and idempotent. A prepared
render owns `Arc<PreparedScene>`, scratch and the next global band, constructing a
temporary borrowed execution view for each step. An output lease pins one of a
fixed number of slots; no free slot means no execution. Close drops owned demand,
while independent sources/jobs retain their own leases. Loading, extraction and
encoding limits are domain records; session admission, worker counts and aggregate
queues belong to the host. Amend ADR 0016 without growing its 256-byte `Limits`
record or borrowing unrelated limits.

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
missing input. Preserve v1.0–1.12 complete-buffer behavior. Browser fetching stays in
JS; this work does not require the separately planned JSPI transport.

The host registry uses the same Session ordering/dispatch for native decoded
requests and stdio frames. V2 is 0x00020000; requests 41–52 and responses 169–179
cover immutable source lifecycle, opening/page/render jobs and exact-sequence
output acknowledgements. The wire layouts live in pagura-host/PROTOCOL.md and
canonical code tables. One unacknowledged canonical band/chunk lease prevents
further producer work. A prepared render is source independent; source/document
closure preserves independent jobs and issued output. SourceFinish checks full
coverage without assembling bytes and makes missing coverage terminal.
Aggregate source ceilings use the existing MemoryBudget parent/child admission;
that ownership follows byte and frontier leases after source closure. Active
operation/document domain ceilings follow job/context ownership. These counters
exclude bounded fixed controller/index storage and separate framed transport
copies; they are not RSS evidence. Worker scheduling, browser driving, extraction
and full resource acceptance remain required inside this group.


### 8. Budgets and compatibility

Give loading, extraction, encoding and aggregate host queues their own
documented budgets. Amend ADR 0016 where required, respecting its record-size
constraint and one-meaning-per-field rule. Account for source leases, active
store values, image copies, scratch and transport buffers together.

This decision permits a pre-release API migration for source-dependent return
types and whole-file borrowed slices. Keep complete-input convenience entry
points and wire v1 adapters, with one shared implementation underneath.

### 9. Bounded worker scheduling belongs to this group

Native and browser/WASI scheduling share job priorities, bounded admission and
cancellation semantics. Prepared scenes may be shared; each executing region owns
its scratch and output slots. Worker scheduling never creates another renderer or
changes ordered errors and pixels. Source/output waits release CPU admission.
Native CPU workers and browser worker instances need explicit aggregate memory
and queued-work bounds, starvation controls and shutdown cleanup. Browser transport
may copy immutable supplied data between isolated WASI instances; those copies
must be accounted rather than described as shared memory. P0–P9 delivery alone
does not close group 11 until this D01 scheduling obligation has passed its gates.

## Relationship to the band-thread proposal

[The existing band-thread proposal](https://github.com/jonasgrosch/pagura/pull/156)
retains ADR 0035. At source revision `a10f681b524dda7b580f1c7626e2358f70df1341`
it shares one prepared scene across private per-worker scratch, admits helpers
from the remaining render budget and uses the same executor and output
quantization. Those primitives are compatible with this decision.

Its collected-surface scoped-thread driver, global `Send` change to stop
callbacks and disabled WASM helpers do not provide owned streaming jobs or the
host scheduler. Consolidation must keep one band execution path and fixed
ordered output slots, preserving existing callback compatibility. Cross-job
priorities, aggregate admission, source/output waits and shutdown belong to the
host scheduler. Any parallel bands within one render need a bounded reorder
window over the same slots and executor; they do not justify another renderer
or repeated region preparation. The proposal's exploratory timings are not
validation of this implementation.

## Options considered

| Option | Tradeoff |
|---|---|
| Explicit availability and resumable jobs | More deliberate source/error API work; supports native async servers and capability-free WASI through one engine |
| Async through every parser and kernel | Suspension spreads through CPU code, but CPU admission and stable data ownership still need separate solutions |
| Blocking network-backed Read/Seek | Simple experiment; occupies CPU workers during I/O and complicates serial WASI supply and cancellation |
| Complete download and full output buffering | Existing compatibility path; cannot provide early dependency-based work or streamed pixel memory bounds |

The choice is explicit availability with owned jobs and async adapters.

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
