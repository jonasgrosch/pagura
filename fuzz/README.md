# Fuzzing

Coverage-guided fuzzing of the product crates with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer). This is test
tooling in its own Cargo workspace and never a product dependency. libFuzzer
is C++ and its entry points need `no_mangle`, so the one-line binaries in
`fuzz_targets/` cannot forbid unsafe code. Everything they run lives in the
drivers of the `pagura-fuzz` crate (`crates/pagura-fuzz/src/`), which do
forbid it, and which its stable replay test
`crates/pagura-fuzz/tests/regressions.rs` runs as well: a stored input
exercises the same code under `cargo test` as under libFuzzer.

Every driver checks the same properties: nothing panics; every error is a
structured `pagura_core::Error` (a fixed message and well-formed `area.item`
tags); strict recovery reports no repairs (ADR 0003); work stays inside the
reduced budgets of `pagura_fuzz::limits()`, so a run that exceeds the time
or memory limit is a finding; and results keep their documented contracts,
listed per target below.

| Target | Input | What it checks |
|---|---|---|
| `syntax` | bytes | object parsing in strict and repair mode |
| `filters` | selector byte (filter, predictor), encoded data | decoders respect their output cap |
| `document` | a PDF file | opening, lazy loading of up to 256 objects and their stream data, `pagura::signatures` (every failure structured), page enumeration and recording of four pages through one render context, each asked for again and returned from the context's store or recorded to the same marks, in both recovery modes |
| `render` | a PDF file | the first page records once and renders to exactly `device_size` |
| `font` | selector byte (Type 1, TrueType, CFF, OpenType), program | `FontProgram::parse`, then outline and advance of every glyph, names, `cmap` and CID lookups |
| `image` | selector byte, then codec data or wrapper parts | `dct`, `ccitt` (strict and reported row recovery), `jbig2::decode`, `jpx::decode`, `decode_xobject` and `decode_inline`; samples match the reported size and bit depth |
| `text` | selector byte, string, wrapper parts | `PdfFont::load` and `decode`; glyph codes cover the string exactly; outlines of embedded programs and Type 3 procedures load |
| `content` | a content stream | `RenderContext::scene` on a fixed 32 × 32 page with one resource of each kind; rendered, a tile equals the same pixels of the whole page (ADR 0004, ADR 0034) |
| `host` | flags, page, scale, tile, search window and query, PDF | hello, open, page_info, v1.1 generations with a current and a stale tile, render_tile, the v1.2 reading requests (page text, a search window and the complete search, page links, outlines, page labels, accounting before and after close), diagnostics and close through `pagura_host::Session`; one response per request in order, each with its documented payload layout, errors included; stale tiles skipped, the same tile identical through v1 and v1.1; matches inside the window and the page text, folding to the query, and a cursor that resumes with exactly the remaining matches; close releases the text cache; v1 and v1.1 sessions answer reading requests with an operational InvalidArgument error and keep serving |
| `navigation` | a PDF file | outlines, named destinations and lookups of every listed name, name and number trees, the OpenAction, links of up to 16 pages and page labels, in both recovery modes; results within one traversal budget, destination pages inside the document, outline and action parents before their children, link quads inside the rectangle, one label per page, and actions read back from their own dictionaries, with a second read returning the same result |
| `extraction` | a content stream, optionally ending in a `%` query line | `text_page` and `RenderContext::text_page` on the `content` page, in both recovery modes: the same text or the same error, and afterwards the context renders the same pixels as `render_page` alone; characters partition the text, lines and words lie inside it, glyph quads are finite; `TextPage::find` of the query and of a slice of the text, with every option, returns ranges of the text that fold to the query |
| `write` | wrapper parts | every object of the wrapped document goes through `pagura::Builder` with its raw stream data; the written file reopens strictly (or exceeds a budget), and every object reads back equal, reals up to their last bits and `/Length` aside |
| `forms` | selector byte (fill values, strict bit), a PDF file | `acro_form` (parents precede children, widgets name existing fields), `form_filler` filling up to 16 fields with text, button and choice values, and `pagura::prepare_form`: one appearance per widget, in widget order |
| `cmap` | wrapper-separated parts: a CMap program, the parent its `usecmap` names, codes | `pagura::provider::PredefinedCMap::parse`, then `decode_code` over the codes, each consuming one to four bytes |
| `security` | selector byte (strict bit), password, separator, a PDF file | opening with the password through the standard security handler, then the `document` walk |

Structured inputs split their parts at a line holding only `%%%%`. The
wrapper targets (`text`, `image` modes 4–7, `write`) turn the parts into objects 3,
4, … of a small, valid PDF after a fixed catalog (1) and empty page tree (2),
as in the generated fixtures. A part `<< entries >> stream`, followed by a
line break and the data, becomes a stream whose `/Length` the wrapper sets;
any other part is used as an object body verbatim. The selector bytes are
documented in `crates/pagura-fuzz/src/image.rs`, `text.rs`, `host.rs`,
`forms.rs` and `security.rs`, the search query of `extraction` in
`extraction.rs`. The ICC profile parser is not a target of its own: the
facade does not expose it, and `render` and `image` reach it through
`ICCBased` colour spaces.

## Routine run

```sh
rustup toolchain install nightly          # cargo-fuzz needs nightly
cargo install --locked cargo-fuzz
python3 tools/fuzz_smoke.py               # every target for 30 s, offline
python3 tools/fuzz_smoke.py --target font,image --time 120
```

`tools/fuzz_smoke.py` builds the seed corpora if they are missing, builds the
targets in release mode with debug assertions and overflow checks, and runs
each target with `-rss_limit_mb=2048`, `-timeout=10`, a per-target
`-max_len` and the fixed random seed 1 (`--seed`), one target at a time. New
inputs go to a scratch directory, so routine runs start from the same corpus;
`--keep` adds them to `corpus/<target>` instead. It prints executions, edge
coverage (`cov`), features, corpus size and peak memory per target, and exits
with status 1 when any target fails, saving the input under
`artifacts/smoke/`. It needs nightly, so it is not part of `tools/check.py`.

Longer campaigns use cargo-fuzz directly, for example:

```sh
cd fuzz && cargo +nightly fuzz run -O -a document corpus/document -- \
    -max_total_time=600 -max_len=131072 -rss_limit_mb=2048 -timeout=10
```

## Seeds

`python3 fuzz/seed.py` rebuilds `corpus/<target>/` from the manifests in
`corpus/manifests/`: every generated PDF for `syntax`, `document` and
`render`, the classic and document PDFs (with the navigation fixtures) for
`navigation`; the codec vectors for `filters`; the font vectors under each
format selector; the image, JBIG2 and JPEG 2000 vectors both as raw codec
inputs and wrapped in image XObjects; the text fixtures' objects, and every
font vector embedded in simple and composite font dictionaries; the pages'
content streams for `content` and `extraction`, which also gets the text
extraction fixtures' streams ending in one of their queries and a few streams
showing text on its page; the classic, document and content PDFs behind a
host request header, and the text extraction and navigation fixtures behind
one with the reading requests, the latter in all three protocol versions. Two
review cases join the document and navigation seeds: a stream whose `/Length`
refers to its own object, and two streams whose lengths refer to each other.
When present, the pinned corpora in `$PAGURA_CORPORA_DIR` add real-world
files. Corpora (`corpus/`), crash artifacts (`artifacts/`) and
builds (`target/`) stay local.

## Findings

A failing input is minimized and kept as a regression:

```sh
cd fuzz && cargo +nightly fuzz tmin -O -a <target> artifacts/<target>/<crash-file>
```

The minimized input goes to `regressions/<target>/<short-name>`, with a
one-line note on the defect in `regressions/<target>/<short-name>.txt`.
`cargo test -p pagura-fuzz --test regressions` (part of `tools/check.py`)
replays every stored input on stable Rust and requires it to finish without
a failure. An input whose fix has not landed yet goes on the short
`KNOWN_FAILURES` list in that test, naming the defect; once it passes, the
test fails until the entry is removed. The fix itself comes with a minimized,
independently explained fixture in the owning area. The same test runs the
`navigation`, `extraction` and `host` drivers on the generated navigation and
text extraction fixtures, which they must pass.
