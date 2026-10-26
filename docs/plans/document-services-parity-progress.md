# Synchronous document service completion

The consolidated read/extract bundle preserves one immutable document model and
one EF/RF selection implementation shared by the facade, CLI and host. It covers
catalog/name-tree and catalog/page/object AF sources, page Annots slots, exact RF
pairs, full file declarations, collection schema/sort/item and encrypted-payload
metadata, object/page XMP packets, source image resources, stored thumbnails and
immutable attachment-only reauthentication. Existing protocol records stay
compatible; extended declarations use protocol 1.12 requests 31–40 and responses
159–168. Bytes are explicitly requested, filenames remain data, and output paths
are chosen by callers.

This is a synchronous source-service boundary. Direct page image resource lists
are explicitly not a painted-occurrence inventory. Inline/form/pattern/annotation
occurrences, isolated normalized image rendering with intrinsic masks, streaming
PNG/DEFLATE, persistent output jobs and async backpressure retain P4–P9 ownership
under ADR 0035. XMP schema/date/language/profile validation and Info/XMP
reconciliation are not implied by preserving packets. Collection navigators,
folder/layout customization and automatic nested-document activation are inert.
D02b attachment/metadata/collection writes remain dependent on E01/E02.

Validation follows ADR 0031: no local builds, engine execution, tests, corpus
scans, benchmarks or profiling. Source formatting, syntax checks and independent
small fixture generation are local; runtime checks use explicit GitHub-hosted
Ubuntu, macOS and Windows runners. Results will be recorded by immutable revision.

## Hosted evidence

The consolidated branch starts directly at `73e1ab018`; its bounded source-service
and complete PNG paths exclude appearance generation and annotation render probes.
The unchanged render manifest retains the target's six independent controls.

[Capability run 40331344005](https://github.com/jonasgrosch/pagura/actions/runs/40331344005)
passed at `5def96197e4e8ae3118f5a8f3aacc0e845e2fd3e`. Its combined service report
contains 147 observations: 143 pass and four explicit adapter-unavailable text
controls (two each for hayro and CoreGraphics). Pagura passes all 21 selected
independent service expectations. This does not close the unprobed extended APIs
or promote any entire family to parity.

[Interoperability run 40332452436](https://github.com/jonasgrosch/pagura/actions/runs/40332452436)
passed at `ba8560df1`: Rust linting, public library/CLI/host service tests, native
and WASI byte parity, viewer source panels and password-retry cleanup, independent
saved-file readers and browser thumbnail pixels. Subsequent null-resource and
source-selection-lifetime controls retain the same transfer contracts. Their full
three-platform functional run is
[40333159571](https://github.com/jonasgrosch/pagura/actions/runs/40333159571).

Source remains on `parity/document-services-review`; the earlier
`feat/document-services-complete` checkpoint is preserved for recovery. The
retained review is #213. No additional feature PR or merge was created.
