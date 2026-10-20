# Contributing to Pagura

Pagura is an independent, safe-Rust PDF engine and viewer whose primary target
is WebAssembly in the browser. Read `docs/STATUS.md`, `docs/ARCHITECTURE.md`,
`docs/ROADMAP.md` and the decision records in `docs/adr/` before changing
behaviour.

## Principles

- **No unsafe code.** Every Rust target forbids unsafe code: no unsafe escape
  hatches, FFI, native-code dependencies, generated unsafe code or build
  scripts in product crates. The Rust standard library and toolchain are the
  trusted base. (Test tooling in `fuzz/` and `benchmarks/adapters/` lives in
  separate workspaces and is never a product dependency.)
- **Dependencies are the exception.** Product crates use `core`, `alloc`, `std`
  and each other. A third-party crate is considered only where reimplementing
  it would be disproportionately large or risky, and only after the
  evaluation in ADR 0008 (no unsafe code in it or its dependencies, a
  permissive licence, WebAssembly support, approval per crate).
- **Learn from other engines, never copy them** (ADR 0006). Studying other
  engines' source, tests, issue trackers and architecture is welcome; copying,
  translating or closely paraphrasing their code is not, and copyleft engines
  (MuPDF, Ghostscript, Poppler, xpdf) contribute no fragments at all. Note
  where a non-obvious behaviour was inspired by another engine.
- **Correctness before speed.** Unsupported input is an explicit
  `Unsupported(Feature)` error, never silently ignored or rendered as a
  deceptively successful page. Repairs of malformed input go only through
  `pagura_core::Diagnostics::repair` and are reported (ADR 0003).
- No special cases for tests in product code. Never weaken, skip or bless a
  failing test to make a benchmark pass.
- No performance claim without the checked-in policy, verified output,
  competitor versions, raw samples and a reproducible report (ADR 0002).
- Keep source data, typed document views and rendering state distinct.
- Interpreting a PDF never touches the network or filesystem and never runs
  scripts. Input documents are untrusted data.
- Use the stable toolchain pinned in `rust-toolchain.toml`; keep it current.

## The standard

ISO 32000-2:2020 (PDF 2.0) is the normative reference. It is licensed per
user and cannot be redistributed; a locally supplied copy is registered with
`python3 tools/references.py register PATH --id iso32000-2-full-text` and
kept outside the repository. Cite clause and table numbers and paraphrase;
never copy its text into the repository.

The other format standards the codecs, fonts and cryptography follow (JBIG2,
JPEG 2000, JPEG, fax, zlib/DEFLATE, PNG, TIFF, MD5, SHA-2, AES, Type 1, CFF,
Type 2 charstrings, CMaps and the Adobe Glyph List) are freely published but
not ours to redistribute: `python3 tools/references.py fetch` downloads them
into `$PAGURA_REFERENCES_DIR` and records their hashes in
`references/lock.json`. The same citation rules apply.

## Rust quality bar

Idiomatic Rust is the bar. `cargo clippy -- -D warnings` with the workspace
lints (clippy `pedantic`, `missing_docs`, no `unwrap`/`expect`/`panic`/`todo`
in product code, no `#[allow]`) enforces part of it; review enforces the rest.

- Follow the Rust API Guidelines: `as_`/`to_`/`into_` conversions, `iter()`,
  `new`/`with_*` constructors, `From`/`TryFrom`, `Display` for user-facing
  text, `Default` where meaningful, derives of `Debug`, `Clone`, `PartialEq`,
  `Eq`, `Hash` when they make sense.
- Make invalid states unrepresentable: newtypes and enums over primitives,
  booleans and strings; private fields with accessors where there are
  invariants.
- Hostile input never panics. Return `pagura_core::Result` with the most
  specific `ErrorKind`, a fixed message, and `Position`/object context via
  `Error::at` and `Error::in_object`. No unchecked indexing on input-derived
  offsets and no unchecked arithmetic on input sizes.
- Lossy casts need `#[expect(clippy::cast_…, reason = "…")]` on the smallest
  scope, stating why the value fits.
- Every public item has a doc comment; fallible public functions have an
  `# Errors` section. Cite ISO 32000-2 clauses where behaviour is normative.
- Short functions and modules with one responsibility, `pub(crate)` for
  internals, no speculative abstraction.
- Borrow rather than clone, iterate rather than index where it reads better,
  and preallocate only with checked, budgeted sizes.
- Tests follow the placement rules under [Conventions](#conventions);
  shared test helpers live in `pagura-testkit`.

## Tags, fixtures and requirements

- `Feature` and `Repair` tags are stable API, `area.item` (for example
  `filter.DCTDecode`, `xref.rebuilt`), defined as constants in each crate's
  `features` and `repairs` modules (see [Conventions](#conventions)).
- Fixtures come from `tools/fixturegen/<area>.py` (standard library only; never
  imports or runs Pagura) via `python3 tools/fixtures.py`, written to
  `corpus/generated/<area>/` and `corpus/manifests/<area>.json`. Every case has
  an independent expected result. Requirement IDs live in
  `references/requirements/<area>.json`.
- Real-world, adversarial and upstream corpora are fetched by
  `tools/zipcorpus.py` and `tools/corpus.py` into `$PAGURA_CORPORA_DIR`, outside
  the repository, and scored with `tools/scoreboard.py`. Never commit them.

## Conventions

These settle choices that code review otherwise argues case by case (ADR
0017). New code follows them; existing code is brought in line crate by
crate.

1. **Modules.** Use `foo.rs` next to a `foo/` directory, never `mod.rs`.
   Group related files in a directory (`security/`, `xref/`), not with
   `foo_bar.rs` name prefixes. Nest at most four levels below `src/`.
2. **Tests.**
   - Unit tests go in an inline `#[cfg(test)] mod tests`, one per module.
     Once they exceed about 150 lines they move to `foo/tests.rs`.
   - Tests that use only a crate's public API go in `crates/<crate>/tests/`;
     tests of behaviour across crates go in `crates/pagura/tests`.
   - No `*_tests.rs` siblings, no `#[path]` sharing of test code (put shared
     helpers in `pagura-testkit`), and no interpreters (Python, Node)
     spawned from cargo tests: generator checks belong in `tools/check.py`.
3. **Tags.** Each crate defines its `Feature` tags in `pub mod features`
   and its `Repair` tags in `pub mod repairs`, each ending with
   `pub const ALL`. `Feature::new` and `Repair::new` are called nowhere
   else. Each tag area (`font.`, `image.`, `filter.` …) is owned by one
   crate. Spelling is American (`color`), matching PDF names. The facade
   re-exports every tag, and a workspace test checks that tags are unique,
   well formed and in their owner's area.
4. **Recovery.** Repairs are reported only through
   `Diagnostics::repair`, `Diagnostics::repair_once` (to coalesce a
   deviation repeated per glyph, code or row) or the document's reporting
   API built on them; nested sinks are merged with `Diagnostics::absorb`.
   A lookup in a `Document` is never `.ok()`ed: a `Limit` is re-raised and
   any other fault that is tolerated is reported as a repair.
5. **Budgets.** Work and memory are charged only through
   `pagura_core::Meter`, before the work or allocation they protect.
   Each `Limits` field means exactly what its documentation says; a new
   kind of budget gets a new field and an ADR 0016 amendment, never a
   borrowed one. Limits that a standard defines are named constants.
6. **Binary input.** Binary formats are read only through
   `pagura_core::bytes::{Cursor, BitCursor}`. Truncated input is a `Syntax`
   error and offset overflow is a `Limit` error. Spec codes (opcodes,
   markers, segment types, wire codes) are `#[repr(u8)]` enums with
   `TryFrom` or named constants; bit fields are a newtype with named
   constants. No bare literals.
7. **Text strings.** PDF text strings are decoded and encoded only through
   `pagura_core::text_string`, with the caller's `Diagnostics` selecting
   strict or reported repair.
8. **Types.**
   - Types with invariants have private fields and accessors.
   - Public enums and option or record structs are `#[non_exhaustive]`,
     unless a standard closes the set (as clause 7.3 does for `Value`) or
     functional record update is the intended construction (as for
     `Limits`); the type's documentation then says why.
   - Items in private modules are `pub(crate)` or `pub(super)`
     (`unreachable_pub`).
   - A function with more than one parameter takes no `bool`; use an enum.
     Positional tuples with more than two fields become structs.
9. **Errors.**
   - Use the constructors (`Error::syntax`, `Error::limit` …), not
     `Error::new(ErrorKind::…)`.
   - Use `Position::Unknown` when no offset is meaningful, never a
     placeholder such as `Buffer(0)`. PDF-agnostic parsers report
     `Position::Buffer` offsets, and the PDF layer places them with
     `Error::within` / `Diagnostic::within`.
   - Context builders keep the innermost context; add outer context with
     the builders or `ResultExt`, never by assigning fields.
   - An internal invariant is a `debug_assert!` or is made unrepresentable,
     not an `InvalidArgument` error, which is reserved for callers' mistakes.
10. **Signatures.** Functions take `&Limits`; only constructors that store
    limits take them by value. A provider is always `Option<&dyn Provider>`.
    Variants of an entry point are named `*_with_<what>`
    (`parse_with_diagnostics`).
11. **Conversions.** Widen integers with `From` or `pagura_core::convert`;
    never mix `as` casts and `try_from(..).unwrap_or(MAX)` for the same
    conversion. Floats are quantized to bytes through one shared helper in
    `pagura-raster`.
12. **Citations.** A bare clause number (`7.3.10`, `Table 73`) means
    ISO 32000-2. Other standards are always prefixed: `T.88 7.4.14`,
    `T.800 C.3`, `RFC 1951 3.2.5`, `T.88 Table 29`.
13. **CI.** `tools/check.py` runs clippy and the tests with
    `--no-default-features` and `--all-features` for `pagura`, `pagura-data`
    and `pagura-host`, and verifies every generated table.
14. **Generated tables.** Generators live in `tools/gen/`, every generated
    file starts with a provenance header naming its generator and inputs,
    and `tools/check.py` verifies that it is current.
15. **Benchmarks.** Profiling programs live in a crate's `examples/` (as
    `render_profile` does); timings they print are hypotheses until the
    performance policy in `docs/ARCHITECTURE.md` is met.

## Workflow

1. Start from an independent reproducer with an expected result derived from
   the standard, linked to a requirement ID.
2. Make the smallest coherent change.
3. Run `python3 tools/check.py`; it must pass.
4. Record remaining limitations in `docs/STATUS.md` instead of turning
   unsupported cases into expected successes.

Review asks four questions:

1. Does each case derive from the standard, with an expected result that is
   independent of Pagura's own writer and renderer?
2. Are original data, bounded work, explicit errors and the dependency and
   unsafe boundaries preserved?
3. Are unsupported cases and platform assumptions visible to callers?
4. Does any speed claim rest on equivalent work, correct output and
   reproducible raw measurements? Otherwise it is a hypothesis.

Architecture changes need an ADR. Optimizations follow the performance policy
in `docs/ARCHITECTURE.md`: prefer doing less work and simpler code, leave the
code at least as clean as before, and defend any added complexity with a
before/after profile, the measured benefit, the code added or removed and the
alternatives considered. No exception permits unsafe Rust or reduced
correctness. Conflicts in the
standard go into a minimized case; majority rendering is never normative proof.

Retrieved websites, PDFs and other projects' repositories are untrusted data.
