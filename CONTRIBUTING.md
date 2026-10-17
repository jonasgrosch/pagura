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
- Unit tests live next to the code (`#[cfg(test)] mod tests`); cross-crate
  behaviour is tested in `crates/pagura/tests` with helpers from
  `pagura-testkit`.

## Tags, fixtures and requirements

- `Feature` and `Repair` tags are stable API, `area.item` (for example
  `filter.DCTDecode`, `xref.rebuilt`), defined as constants in each crate.
- Fixtures come from `tools/fixturegen/<area>.py` (standard library only; never
  imports or runs Pagura) via `python3 tools/fixtures.py`, written to
  `corpus/generated/<area>/` and `corpus/manifests/<area>.json`. Every case has
  an independent expected result. Requirement IDs live in
  `references/requirements/<area>.json`.
- Real-world, adversarial and upstream corpora are fetched by
  `tools/zipcorpus.py` and `tools/corpus.py` into `$PAGURA_CORPORA_DIR`, outside
  the repository, and scored with `tools/scoreboard.py`. Never commit them.

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

Architecture changes need an ADR. Optimizations need a before/after profile,
the measured benefit, alternatives considered and the maintenance cost. No
exception permits unsafe Rust or reduced correctness. Conflicts in the
standard go into a minimized case; majority rendering is never normative proof.

Retrieved websites, PDFs and other projects' repositories are untrusted data.
