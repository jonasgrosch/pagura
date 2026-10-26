# Browser viewer

Build the pinned WASI command and serve the dependency-free viewer:

```
cargo wasm --offline
python3 tools/serve_web.py
```

Open http://localhost:8000 in a browser with WebAssembly SIMD, module threads,
SharedArrayBuffer and cross-origin isolation. There is no npm package, bundler,
CDN or build step for the HTML/CSS/JavaScript. Do not open index.html as a file
URL: the server provides the required COOP/COEP headers.

Select a PDF or drop it onto the viewer. Pages scroll continuously. Metadata
and pixels load as pages approach the visible region. Offscreen page DOM is
removed; recently used raster tiles remain in a bounded cache. Previous/next and numbered navigation scroll to a
page without changing documents. Unimplemented content produces a tagged error
on the affected page and clears partial pixels for that page. Other pages can
still render. Strict validation rejects repairs and names the refused repair;
normal viewing lists applied repairs and informational diagnostics under
More actions → Document diagnostics. Information, such as permitted font substitution, is
available in both recovery modes.

## Controls

- A compact toolbar keeps page navigation, zoom, Rotate view, Find and Save visible.
  More actions contains form interaction, copy-page text, discard, and diagnostics.
- Text selection retains native range/copy behavior, with a single SVG fill over
  selected glyph quads to avoid dark overlaps from per-character highlights.
- The collapsible sidebar switches between thumbnails and the document outline.
  Thumbnail rows are virtualized and keyboard navigable; up to 32 thumbnails
  are retained (under 24 MiB including separate base/draft RGBA canvases). One thumbnail request runs
  at a time, after visible-page work, and stale document results are discarded.
  Thumbnails retain twice CSS density. Complete page tiles are stitched at their
  original integer coordinates before a single downsample, avoiding seams from
  separately filtering fractional tile edges. Temporary RGBA scratch storage is
  capped at 8 MiB and 4096px per axis; copying yields every four tiles so input
  can interrupt it. Larger or incomplete cached pages use idle 256px chunks to
  build a temporary higher-resolution raster (at most 5.68 MiB). Scratch storage
  is released on completion, interruption or failure. Visible pages composite the same cached
  form artwork used on the page after a short input coalescing delay, without
  saving or rendering another document. Offscreen draft previews are marked
  until their page is viewed; saving refreshes the document-wide baseline.
- Rotate view turns the current page left in 90-degree steps, including text,
  links and form hit targets. This is a viewing preference, **not a saved edit**.
  It survives form saves but resets when another document is opened.

- Zoom: fit width, fit page, percentage presets, or Ctrl/Command-wheel. Two-finger
  touch pinch also changes document zoom. Fit presets use the selected page's
  physical dimensions; changing the selected page or resizing recalculates them.
- Keyboard: Page Up/Down select adjacent pages, Home/End select the first/last,
  +/− change zoom and 0 restores 100%. Editing fields and buttons keep their
  normal keyboard behavior. Zoom is limited to 10%–800%. Native arrow-key scrolling works in the focusable
  document region.
- Password-protected PDFs open an accessible modal with retry, Cancel and Escape.
  The field is cleared when submitted, cancelled or replaced by another file;
  temporary password bytes are cleared after each open attempt. A private copy
  remains in memory for automatic recovery until a new file is selected or the
  page closes. Text entry sends
  UTF-8 bytes. Non-ASCII legacy passwords requiring PDFDocEncoding are not
  converted by the UI; callers can use SessionClient.open with explicit raw
  bytes. ISO 32000-2 7.6.4.3.2 and 7.6.4.3.3 distinguish those encodings.
- Controls have labels and standard focus order. Pages have named region roles,
  loading states and explicit failures. This is visual navigation accessibility,
  with a transparent reading-order text layer for screen readers. Structure-tree ordering and bidirectional reconstruction are still engine limitations.

The layout uses page geometry from ISO 32000-2 7.7.3.3 (Table 31) and 8.3.2.3.
Compact prefix sums retain discovered heights without creating a DOM element
for every page. Detailed metadata is capped at 256 pages, visible page elements
at 32, and canvases at 128 tiles. Tiles are 256 device pixels, painted at
zoom × renderDensity(zoom, devicePixelRatio). Reduced pages target two raster
pixels per page point, with additional density capped at four pixels per CSS
pixel; native HiDPI is never reduced. Page tiles, button artwork and draft
appearances share that density, avoiding heavy device-snapped borders at low
zoom. Layout extents are limited to 10 million CSS pixels;
exceeding a budget displays an error rather than silently truncating a document.

## Session and caching

Full-size raster canvases share a 64 MiB RGBA backing-store budget (and 512-entry
cap), including attached visible tiles. Visible keys are pinned; least recently
used unpinned tiles are evicted and their backing dimensions cleared. Cache keys
include page, exact raster scale and tile geometry. CSS view rotation reuses the
same pixels. Save/adoption, open, host recovery and page shutdown clear the cache;
zoom/DPR changes use distinct scale keys. This accounts for retained raster
pixels, not total browser RSS, GPU copies or the engine's own caches.

After 500 ms without interaction, up to 16 tiles in a half-viewport strip
(maximum 512 CSS px) are considered ahead of scrolling, on either axis. Only one
speculative request runs at a time, after the foreground queue drains. Direction,
zoom, navigation and input changes cancel the plan; stale results are discarded.
Preloading pauses while form metadata is unknown or form filling is active on a
form document, and while input, saving or draft previews are active. The current
host cannot interrupt a tile already executing; pausing avoids new speculative
work, and generation checks prevent stale queued requests from being sent.
Thumbnails have lower priority than visible tiles and eligible preloading.
On-demand diagnostics show retained bytes, cache reuse and completed preloads.

The browser negotiates protocol v1.7 for form editing and typed diagnostics; existing protocol v1 clients remain
supported. See crates/pagura-host/PROTOCOL.md for the byte layouts. Wire codes and
default budgets come from protocol_constants.mjs, generated from the host's Rust
tables by `cargo run -p pagura-host --features codegen --example protocol_constants`.
A viewport
change invalidates queued old-generation tiles before transmission. The host
also skips a stale generation before recording or painting. The current
synchronous tile cannot be interrupted; its result is discarded if obsolete.
Opening another file replaces the instance and releases its caches.

Each page is recorded once into a scene, independently of zoom or device
pixels, and kept in its document's render store with the decoded images, fonts
and glyph setups that later tiles reuse. Each store has a default capacity of
64 MiB (SessionLimits::store_bytes) and evicts the least recently used values
no render still uses; close releases it. A value larger than the capacity is
used without being retained, and a zero capacity disables retention.

The retained bytes are the engine's own estimates of what each value holds,
not allocator telemetry, and the capacity is not a hard process-memory
ceiling. Per-render limits remain in force. Keeping scenes rather than pixels
uses less memory and needs no keys for zoom and output settings.

The project-written WASI shim exposes no preopened directories, sockets,
environment or arguments. Only the dedicated execution thread blocks on
Atomics.wait; stdin is a 64 KiB shared mailbox and stdout uses transferred byte
chunks. Unknown WASI imports trap by name. PDF interpretation receives no URL
or file capabilities. A user-selected file stays local to the browser.

## Development fixtures

```
python3 tools/serve_web.py --fixtures
```

Then open:

```
http://localhost:8000/?file=/fixtures/classic/two-pages.pdf
```

The development-only flag additionally serves corpus/generated read-only at
/fixtures/, on loopback only, with the same isolation and path protections.
Without the flag those URLs return HTTP 404. There are no upload/write routes
or directory listings; private/reference corpora are never exposed.

Startup loading accepts one same-origin /fixtures/ URL. Other origins or paths,
parent traversal, credentials, nested query/fragment and duplicate file
parameters are rejected before fetching. Redirects are refused, credentials
are omitted, and received bytes are capped at 512 MiB. Picking/dropping another
file takes precedence over a pending startup load.

Automation can wait for #status = "Visible tiles rendered" and #error hidden,
then inspect .tile canvases inside section[data-page="0"] or another zero-based
page section. Both pages of two-pages.pdf coexist when visible. Its second page
is white except for the blue square at device x=32..63, y=0..31 at scale 1.
text-unsupported.pdf exercises the failure banner; trailing-bytes.pdf exercises
repair display. Existing real-browser verification covered isolation, exact
DPR-2 pixels, navigation and the explicit failure banner. The continuous layout,
new gestures and modal also have offline DOM tests and need browser interaction
checks on the target devices.

## Verification and exploratory profiling

```
cargo build --locked --offline -p pagura-host
cargo wasm --offline
node tools/host_test.mjs
```

Tests preserve native/WASI document-response identity and independent classic
PPM oracles, plus page-2 full/partial geometry oracles, generation skips, cache
reuse/eviction, continuous lazy layout, navigation, fit presets, gestures,
password retry/cancel, diagnostics and explicit failures. In-memory HTTP tests
check isolation headers, MIME types and path containment without a socket bind.
The DOM harness does not establish real-browser layout or touch behavior.

The hosted `thumbnail-pixels` job runs `python3 tools/thumbnail_pixels.py` with
Chrome in a temporary headless profile. It compares every thumbnail RGBA pixel
with a single-image downsample of independently drawn artwork, including dark
backgrounds, text and strokes across tile edges, transparency, partial tiles and
portrait/landscape sizes. A negative control must detect seams in the original
per-tile scaling path. It also checks bounded fallback chunks, foreground
interruption, cache eviction and scratch cleanup. JSON results and before/after
PNG evidence are uploaded as the `thumbnail-pixels` artifact. No product build
or additional Python/browser-driver dependencies are needed.

Build release binaries for each revision and run the same workload against each:

```
cargo build --locked --offline --release -p pagura-host
cargo wasm --offline
node web/tile_benchmark.mjs target/release/pagura-host target/wasm32-wasip1/release/pagura-host.wasm > /tmp/tiles.json
cargo run --locked --offline --release -p pagura-host --example tile_profile -- corpus/generated/classic/many-rectangles.pdf
```

The benchmark emits binary/input/output hashes, hardware/runtime identities and
raw seconds/tiles-per-second samples. It uses two warmups and five alternating
samples per runtime for 64 fixed tiles of many-rectangles.pdf, verifying every
pixel against its independent PPM after timing. Native samples include process
startup; WASI samples include _start/open and exclude module instantiation.
Compare revisions within each runtime, not native and WASI timings to each
other. The profile separately measures 64 recordings and 64 tile paints from
a stored scene. These are exploratory diagnostics, not the sample count, confidence
intervals, memory measurements or controlled runner required by the release
policy in benchmarks/policy.json. They make no cross-engine speed claim.

## Failure recovery

A module trap, allocation failure, Limit response, unexpected exit, or a
30-second startup/request deadline terminates the current execution thread.
The viewer reports the failure, restarts the host, and reopens the retained
local bytes at the same page, zoom and relative scroll position. It retains
the recovery policy and a private password copy for that document's lifetime.
The recovery notice stays visible after successful rendering. Only one
automatic attempt is allowed per selected file; another failure stops rendering
and asks the user to reopen explicitly. Selecting another file takes precedence
over an ongoing recovery. Syntax, Unsupported and password errors retain their
usual behavior. The shim detects linear memory over 512 MiB at I/O boundaries;
this does not constrain transient peak allocations inside an engine call.

For local development only, open `http://localhost:8000/?debug=1` and call
`window.paguraDebug.fail('trap')`, `'memory'`, `'budget'` or `'deadline'` from the
console. This injects a client failure and follows the same termination and
reopen path without changing the engine. The hook is absent without that flag
or on a non-loopback hostname. Tests also cover an actual client deadline and
a structured Limit response; injected failures are not evidence of physical
browser OOM behavior.

## Session measurements

```
node web/host_benchmark.mjs target/wasm32-wasip1/release/pagura-host.wasm /absolute/path/to/external-multipage.pdf > /tmp/host.json
```

The dependency-free harness drives identical operations through Node WASI and
the browser shim/mailbox running in Node's execution threads. The Node WASI
harness replaces stdin/stdout imports with in-memory buffers so external PDFs
are read in place, never written into request files or copied into the project.
It measures document open through page geometry and first tile, 16 warm panning
tiles, and five
zoom changes (50%, 100%, 150%, 200%, 100%); two warmups and five samples are
reported with raw timings and medians. Many-rectangles uses 256-pixel tiles;
the two small classic pages use 32-pixel tiles and external pages use 128.
The ready-session timings exclude module compilation, instantiation and
execution-thread startup. A separate fresh-instance series includes instantiation,
thread startup and hello through open/geometry/first tile, with two warmups and
five raw samples per document. It still excludes module compilation, network
loading, file reads and Canvas presentation; Node receives a compiled module.
External Unsupported results are recorded explicitly as latency to failure,
never as successful first tiles or warm rendering. Classic scale-1 pixels use
independent PPM oracles; all successful outputs have identical hashes across
runtimes, including zoom results. Hash parity alone is not an external oracle.

Each memory series starts in a fresh instance, then performs 50 open/render/close
cycles: either the same many-rectangles input or an alternating mix of three
classic inputs and the external document. Every cycle records live source bytes,
render-store bytes, document and scene counts and linear-memory size, and asserts
zero host counters after close. `SessionClient.stats()` exposes protocol kind 9;
`wasmBytes` exposes the last sideband memory sample. Counts exclude document
internals and temporary/transport/JavaScript allocations. All raw cycle records,
medians, input and module digests and runtime/hardware identifiers go to stdout.
These exploratory samples are not release evidence, an RSS leak proof or a
measurement of browser layout/Canvas/GPU costs. No external corpus data is served.

## Reading, search and navigation

Ctrl/Command+F opens document search. Enter/Shift+Enter or the Next/Previous
buttons move between matches; Escape closes the panel. Case-sensitive and
whole-word toggles use the engine's Unicode/word policy (ADR 0014), with
whitespace folding and joined line hyphens. Search requests cover one page at a
time, yielding between pages. A changed query invalidates queued requests and
ignores obsolete replies; executing requests finish. Matches use source quads
transformed through CropBox, Rotate, UserUnit and zoom. The current match scrolls
into view. Search retains at most 1,000 matches and 10,000 highlight quads and
announces when that budget is reached; unavailable pages are counted explicitly.

A transparent DOM text layer places characters in extracted reading order,
using metric selection quads. Native selection and Ctrl/Command+C copy text;
the copy handler writes only plain text, preserving extracted spaces and line
breaks rather than browser-inferred wrapping. Cross-page copy excludes controls
and link titles; mounted page elements stay in page order even when scrolling
upward. Copy page text copies the current page's exact extracted string.
Screen readers can read the text layer; canvases remain aria-hidden. Offscreen
layers are discarded with their pages, so native browser Find only sees mounted
pages; the custom document search covers the entire document.

Selection cells approximate glyph ink, as specified by the extraction API.
An ActualText character with several source quads uses their union for its DOM
cell; search highlights keep every exact quad. Replacement characters can share
cells. This is not bidi/shaping or structure-tree reconstruction. Per page the
layer allows 50,000 characters and 100,000 quads; visible retained text allows
100,000 characters. Excess is explicit and does not truncate copy text: Copy
page text remains available through the extraction API. Zoom recreates layers;
partial native selections are therefore lost across zoom/eviction.

Link areas are focusable, named links activated by Enter or Space. Internal
XYZ/Fit/FitH/FitV/FitR destinations preserve fit and coordinate instructions,
including nullable coordinates and rotation. FitB/FitBH/FitBV use conservative
painted-content bounds from protocol 1.3, transformed through rotation and
UserUnit. FitB centres the bounds within document scroll limits; FitBH/FitBV fit one axis and
honour the destination coordinate on the other. Empty pages use the crop box.
Destination zoom outside 10%–800% is clamped into that range with a short
informational notice; destination coordinates use the clamped zoom.
Bounds and tiles share the page's recorded scene. The page field follows
the page with the greatest visible height, retaining the selected destination
when visible heights tie; a strip of the previous page no longer wins.

URI/Base are inert data until activation. Relative references require an
absolute PDF Base; the viewer URL is never a fallback. Only http, https and
mailto are allowed, with a native confirmation showing the resolved target.
URI credentials are rejected. Links have no href before confirmation, so
middle-click/context menus cannot bypass policy. IsMap (coordinate-appending)
URIs, GoToR, Launch, JavaScript, unknown Named actions and multi-action chains
remain inert with an informational notice, rather than an error. Standard
NextPage, PrevPage, FirstPage and LastPage Named actions navigate. No scripts,
remote files or action chains run.

The outline sidebar uses an ARIA tree with levels, sibling positions, initial
open states, styles and colours. Arrow keys move/expand/collapse; Home/End select
first/last visible items; Enter activates; Space toggles a branch. Typing selects
by title prefix. Mouse activation follows a target; the branch indicator toggles
expansion. A roving tab stop keeps the tree to one keyboard entry point. The
outline DOM is capped at 20,000 items. Page labels display alongside the numeric
index; the page field accepts a label or `#index`. Duplicate labels select the
first occurrence; exact labels take precedence over bare numeric input.

All PDF text, labels, outline titles and URI descriptions enter the DOM as text:
textContent for content, and inert aria-label/title attributes for link names
and tooltips. Links describe their target, use the destination's page label,
and identify external confirmation or inert actions. Focusing or hovering
a link also shows its target in the polite reading status. No PDF string is
HTML, CSS, a script or an event handler.
Tests use markup-bearing titles/text to verify inert nodes. Protocol 1.2 adds
kinds 10–15; older sessions reject them operationally and retain their layouts.
See PROTOCOL.md for the complete bounded layouts and cursor semantics.

## Real-browser navigation check

Build the host and start `python3 tools/serve_web.py`. Start a separate Chrome
profile with a loopback debugging port, for example on macOS:

```
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless --disable-background-networking --remote-debugging-port=9222 --user-data-dir=/tmp/pagura-browser-check
node web/browser_test.mjs http://127.0.0.1:9222 http://127.0.0.1:8000/
```

The dependency-free check opens and closes its own tab. It loads independently
authored geometry through the file input, checks FitB/FitBH/FitBV for painted
and empty pages, waits for rendered tiles and native scroll events, and checks
the page field when a strip of the previous page remains visible. It requires
cross-origin isolation, verifies page labels and zoom, fails on console errors,
and prints its raw observations and browser identity. Use the separate profile
above rather than an ordinary browsing profile. Both supplied URLs must use
loopback HTTP.

## Reading measurements

The text LRU charges the engine's retained-capacity estimate plus 512 bytes of
cache bookkeeping per page. Its counters are target-specific; they are not RSS
or WebAssembly linear-memory measurements.

```
node web/reading_benchmark.mjs target/wasm32-wasip1/release/pagura-host.wasm > reading.json
```

This offline exploratory harness uses an independent ASCII/Type-3 PDF generator:
100 pages with 50 lines per page. The expected copy string and search count are
computed by the generator, not taken from engine output. It reports module byte
size/hash, first extraction, five complete document-search timings after one
warmup, per-page text-cache/linear-memory samples, and zero text retention after
close. Search timing excludes the separate accounting series. No browser layout
or rendering is timed. The Node DOM stand-in construction series includes UTF-8
mapping, node creation and affine transforms; it is explicitly not real-browser
DOM performance or release evidence.

To measure actual browser construction, open a document with loopback
`?debug=1` and call `await window.paguraDebug.readingProfile()` in the console.
It returns seven construction samples after two warmups, character count and
kind-15 accounting for the selected page. This measures DOM construction only,
excluding extraction, attachment/layout, Canvas/GPU work and module startup.
No document is uploaded. The DOM harness does not establish real-browser selection, assistive-technology
or timing behavior.

## Form editing

Open `/?demo=form` for a sample. Use the host built with bundled-data for standard
fonts. Fillable fields are interactive directly on the page. Click to type,
select choices, or toggle checkboxes/radio buttons, then choose **Save PDF**.
Drafts survive zoom and scrolling; **Discard changes** restores saved values.
**Fill forms** hides or shows controls. Normal rendering uses reported repairs;
**Strict rendering** is an opt-in diagnostic mode. Metadata and writes use a
separate strict snapshot, preserving the library's save restrictions.
See [editing guide](../docs/EDITING.md) for current field/appearance limitations.

## GitHub Pages

The [hosted preview](https://jonasgrosch.github.io/pagura/) publishes from main
through `.github/workflows/pages.yml`. `tools/package_pages.py` copies only the
viewer assets, the bundled-data WASM binary, and license notices; it does not
publish source documents, corpus fixtures or local files.

On hosts without response-header configuration, `start.mjs` registers the
viewer-scoped `isolation_worker.mjs` and reloads once before loading documents.
The worker adds COOP/COEP/CORP and a local-only content security policy to
same-origin GET responses in that directory. It uses no persistent cache,
intercepts no uploads or cross-origin requests, and stores no PDF contents.
Local servers already providing isolation skip registration. HTTPS and service
workers are required on static hosts; unsupported browsers show an error rather
than repeatedly reloading. Pages is static hosting, not a document backend.

## Properties and attachments

Open **More → Properties and attachments** to inspect declared Info fields,
document language and document attachments. Page attachments appear separately;
the initial selection follows the current page. Enter a page number and choose
**Show page attachments** to inspect another page. Annotation notes, icon names
and file descriptions stay distinct. All source labels remain inert text.

Listings never decode payloads. Explicit Download buttons select one exact EF
payload. Downloads use `metadata.xmp`, `attachment-N-KEY.bin` or
`page-P-attachment-N-KEY.bin` and an octet-stream MIME type. No XML, URLs or
embedded content execute, and source filenames cannot choose output paths.
**Check size and checksum** decodes the selected payload and compares its size
and PDF CheckSum with their declarations. Missing, matching and mismatching
values remain separate; this does not establish file safety or authenticity.
Sizes retain exact integer precision. Dates remain unvalidated declarations.

Changing the selected page, closing the dialog, opening another PDF or restarting
the host discards obsolete results and downloads. A page-listing failure preserves
available properties and document attachments. Decode operations are serialized
within the current selection. The host listing cap is 1024 attachments, and each
response is at most 32 MiB. Document stream limits bound complete-buffer decoding;
a browser resource-limit failure ends the session and requires host recovery.

Protocol 1.8 provides properties/catalog reads, 1.10 adds parameter checks and
1.11 adds page annotation selection. Host/native/WASI and viewer tests live in
`tools/document_services_test.mjs`, `tools/document_services_view_test.mjs`,
`tools/attachment_checks_test.mjs`, `tools/page_attachments_test.mjs`,
`tools/page_attachments_view_test.mjs` and the corresponding `pagura-host/tests`
files. Catalog properties/check workflows have passing hosted evidence recorded
in [the plan](../docs/COMPETITIVE_PARITY.md); the new page viewer controls pass the hosted browser shim, with real-browser
interaction validation still open. AF/RF selection,
attachment-only password prompts, icon activation, streaming,
metadata validation and editing remain open.
