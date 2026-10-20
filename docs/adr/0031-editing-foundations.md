# ADR 0031: Immutable editing transactions and incremental form saves

Status: implemented on the editing branch, 2026-10-20. Hosted checks are recorded
against each PR head.

## Context and prerequisite review

The editing lane starts at main `752b52e7`. Reviewed prerequisite snapshots:

| PR | Snapshot | Editing decision |
|---|---|---|
| #24 writer | `2908a854` | Reuse the bounded serializer, typed `NewObject`, per-object validation and tests. Integrated locally with its #15/#17 dependencies. |
| #35 document layout | `e1ba847f` | Reuse as the base of #65. No new editing behavior. |
| #65 typed forms | `89557137` | Reuse caller-owned filling, typed flags, widget identity and bounded views. Integrated locally. |
| #66 document opening | `31ac98ea` | Separate opening/cache refactor; not needed for transactions over an already immutable document. Do not duplicate its builder. |
| #67 signature evidence | `ff5b51c4` | Separate security-result change with owner review outstanding. Editing must not depend on the mutable diagnostic log to infer signature permission. |
| #68 password protocol | `a26be8e9` | Separate host/password API migration. Encrypted editing is not admitted here. |
| #69 reconstruction | `95322e2f` | Separate performance-gated reconstruction work. Repaired sources require a future full rewrite. |

This is a source review of editing dependencies, not approval of the entire
document stack or a claim that their checks have passed on this combined head.
No existing PR is merged remotely by this lane. Shared core/testkit changes
remain visible dependency commits, not new editing implementation.

Review findings affecting the design:

- #24's generated stream Length bypassed name/depth bounds on empty stream
  dictionaries. The shared validator now enforces both, with boundary tests.
- #24's serializer is reusable, but its reference validator assumes every
  target is in a new-document builder. Factor syntax validation from reference
  resolution rather than duplicating the serializer.
- A field's `object` may identify its containing object, not the exact nested
  dictionary. Save only directly identified field/widget dictionaries initially;
  refuse nested direct fields rather than overwrite their enclosing object.
- #65 leaves choice-value semantics pending the licensed standard. Table 234
  in the local ISO 32000-2 EC3 text confirms display strings for V and sorted
  indexes for I. Preserve the existing model, with a serialization regression.
- Filled passwords must not be serialized (12.7.5.3), including their generated
  appearances. Refuse a save containing a changed password field atomically.
- #67 changes repair-dependent signature evidence. This lane defaults to
  refusing signed files and does not claim that preserved bytes imply permitted
  modifications or validity.

## Decision

1. `pagura::editing::Transaction` borrows an immutable `Document`. Replacements
   and new objects are caller-owned, budgeted and invisible to its caches.
   Each set is atomic; reset removes a staged replacement; dropping/rollback
   discards everything. Save returns complete new bytes without filesystem IO.
   Reopen those bytes for a new immutable snapshot. Undo/redo is deferred.
2. Allocate new object numbers above every recorded Size and in-use number.
   Never reuse free numbers; replacing preserves the exact generation. Object
   deletion/free-list rewriting is deferred. A no-op returns original bytes
   exactly, including for sources unsupported for modified saves.
3. `pagura-write` owns syntax and append-only revision serialization; it does
   not parse documents. Share its existing object validation and serializer.
   Append sparse classic xref subsections for table sources and sparse,
   unencoded xref streams for stream/hybrid sources. Use absolute offsets and
   Prev. Preserve unknown trailer entries, Root, Info and the permanent ID;
   remove xref-stream transport keys when converting the trailer to a table.
   Hash the original and appended object bytes for deterministic version IDs.
4. The facade owns source validation, object identity and form orchestration.
   Only sources opened strictly, with a usable revision and unencrypted trailer,
   admit modifications. Signed inputs are rejected by default; an explicit
   `PreserveSignedBytes` policy permits an append without asserting trust,
   certification permission or continued signature validity. Malformed discovery
   remains an error. Source bytes and their signed ranges never change.
   Until #67 lands, the default signature-presence check uses a fresh strict
   source snapshot: this bounds an additional input copy and temporary cache by
   the source limits, and prevents rendering diagnostics from changing save
   permission. Replace that bridge with fixed source evidence when #67 is
   accepted; do not relax uncertainty handling in the meantime.
5. `save_filled_form` creates one transaction, writes validated V/I/AS changes,
   and persists existing appearance-generator output as new Form XObjects.
   Preserve rollover/down appearances and original resources. Generate all
   required normal appearances before clearing NeedAppearances. Failed
   generation or serialization returns no partial file; the filler stays intact.
   XFA/rich text/unsupported appearances keep their existing explicit errors.
   Buttons currently need existing normal state dictionaries: generating just
   their selected state would discard the opposite state needed for later use.
6. Output is bounded by file_bytes; staged objects have a separate caller-owned
   document_bytes allowance, matching FormFiller. Count retained capacities and
   map entries. Structural/value/depth/stream/string limits apply before writing.

## Alternatives

Mutating Document would invalidate OnceLock caches and historical views.
Rewriting every object would lose untouched-byte preservation and signatures.
A generic mutable graph or a second appearance engine adds unnecessary policy.
The chosen overlay keeps source semantics and rendering separate and reuses
existing modules. Low-level replacements deliberately do not validate every
PDF dictionary schema; typed form saving supplies the form-specific rules.

## Delivery and validation

1. Review and integrate the minimal prerequisite branches; record provenance.
2. Share value validation; implement bounded sparse incremental serialization.
3. Implement transaction staging, allocation, source policy and commit.
4. Persist text, choice and button values and generated appearances.
5. Independently authored source fixtures; exact prefix/xref/trailer assertions;
   reopen, repeated save, nonzero generations, compressed-object replacement,
   rollback, allocation and output failures, signatures and password refusal.
6. Form save/reopen/render equivalence, Unicode, choice indexes, button states,
   inherited values, merged widgets and refusal of ambiguous direct identities.
7. Cross-reader structural checks on hosted Linux and the authoritative
   `python3 tools/check.py` on hosted Ubuntu/macOS/Windows.

No local builds, tests, corpus scans, profiling or benchmarks while the
performance lanes use this machine. Short source/format/diff checks only.
Hosted CI must use explicit hosted labels; no self-hosted runner dispatch.
Full rewrite, redaction, encryption-on-write, signed-permission enforcement,
undo/redo, file IO, CLI and viewer editing controls are subsequent work.

## Independent-reader baseline

Hosted qpdf 11.9.0 accepts `hybrid-precedence` before and after saving. Hosted
Poppler 24.02.0 rejects both, so changing the new revision representation alone
does not resolve its original-file incompatibility. Keep the source fixture and
Pagura reopen assertions unchanged. The reader check validates every original
and saved file with qpdf, requires successful Poppler reopen and equal page counts
where Poppler accepts the original, and explicitly reports baseline failures.
It does not present those failures as interoperability passes. No reader source
code was used. Full rewrite to repair another reader's input interpretation is
outside an append-only save contract.
