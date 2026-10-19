# ADR 0024: A document is built before it caches; passwords are typed

Status: accepted, 2026-10-18. Refines ADR 0012 (retained memory) and
invariant 8 of `docs/ARCHITECTURE.md`; protocol change per ADR 0005.

## Context

A review of `pagura-document` found that opening relied on implicit rules:

- `Document::from_table` built a document with no security handler, read
  the encryption dictionary and, after reconstruction, object streams and
  the catalog through it, then mutated it: slots were replaced and the
  trailer rewritten after objects had been cached. An object stream decoded
  with one `/Length` object stayed cached after reconstruction replaced
  that object, so the cache could disagree with `object(N)`.
- The password was cloned into the document's options and later cleared,
  which keeps its bytes in the allocation for the document's lifetime, and
  the security code read it back from there.
- Legacy handlers (revisions 2–4) need PDFDocEncoding passwords
  (7.6.4.3.2, Algorithm 2 step a), but only the CLI encoded text. The
  library, the host protocol and the web viewer passed UTF-8, so a
  non-ASCII password that worked in `pagura render` failed in the browser.
- A failed object load was not cached, so offset correction and terminator
  scans of up to `stream_bytes` repeated on every access.

## Decision

1. **Opening goes through a `DocumentBuilder`** that owns the object table,
   trailer, revisions, diagnostics and security handler. Reading the
   security handler and, after reconstruction, object streams and the
   catalog happens through *scratch* documents frozen from the builder;
   dissolving a scratch document discards its caches and rewinds the
   retained budget to its value before the scratch existed. Reconstruction
   returns plans (new compressed slots, a replacement Root) that the builder
   applies. The opened `Document` is never mutated, so it publishes only
   objects parsed against its final table, trailer and handler. A snapshot
   of an earlier revision (`Document::object_at_revision`) is built the same
   way: strictly, from a prefix of the shared bytes, with a retained budget
   that also charges the source document's.
2. **A top-level object load caches its result, success or failure.** A
   load that starts with no other object in progress depends only on the
   document. Nested failures can depend on the load path (cycles, depth)
   and stay uncached.
3. **The password is passed to the security code explicitly** and never
   stored in the document, which keeps only its limits and recovery mode.
4. **A page carries the identity of its document**, so `Page::links`,
   `Page::label` and `Document::content_streams` refuse a page of another
   document with `InvalidArgument` instead of reading the wrong objects.
5. **Evidence of repair is fixed when it arises.** The document records
   whether opening needed a repair, each object whether its load did, and
   each view whether it applied one. Signature revision coverage, access to
   earlier revisions and permitted-change analysis use this evidence instead
   of the document-wide diagnostic log, which grows as other consumers load
   objects or report information.
6. **Passwords are `Password` values**: bytes reach the handler unchanged;
   text is encoded once the handler revision is known, with PDFDocEncoding
   for revisions 2–4 (`text_string::encode_pdf_doc`; an unrepresentable
   character is `Unsupported` with `security.password-encoding`) and UTF-8
   for 5 and 6, which the handler prepares with SASLprep. A password is
   zeroed on drop on a best-effort basis and redacted in `Debug`. The CLI
   passes `--password` as text and its private encoder is deleted.
7. **Host protocol 1.5** marks a text password with bit 1 of open's
   recovery byte. Older sessions keep treating that bit as an invalid
   recovery byte, which ends the session, as before. The viewer negotiates
   1.5 and sends typed passwords as text; other callers send bytes unless
   they ask for text.

## Consequences

- Objects read while opening are parsed again when the opened document
  first needs them: the encryption dictionary of every encrypted file and,
  for reconstructed files, the objects the catalog scan visited. The
  budget needed to open a reconstructed file can therefore exceed what the
  document retains once open.
- A reconstructed object stream's contents are decoded once for the plan
  and again lazily; if its dependencies were replaced, the opened document
  reports the disagreement instead of serving stale contents.
- Callers construct `OpenOptions::password` with `Password::bytes` or
  `Password::text`; `OpenOptions` no longer implements `PartialEq`.
- A cached failed load is final for the document; retrying requires
  reopening, as for any other cached result.
- A repair reported after opening outside the objects a signature analysis
  reads no longer makes that analysis inconclusive, so some results move
  from `Inconclusive` to `Analyzed`.

## Alternatives considered

- *An `OpenOptions::with_text_password` helper*: the typed `Password` keeps
  bytes and text distinct where the decision is made, and gives the zeroing
  and redaction one home.
- *Clearing every cache after the last mutation*: simpler, but it keeps a
  mutable phase on the public type and depends on finding every cache.
- *Decoding object streams for reconstruction without publishing*: that
  needs a second, non-caching loader for streams and their dependencies;
  scratch documents reuse the one loader.
