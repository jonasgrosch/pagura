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
