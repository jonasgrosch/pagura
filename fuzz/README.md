# Fuzzing

Coverage-guided fuzzing of the product crates with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer). This is test
tooling in its own Cargo workspace and never a product dependency. libFuzzer
is C++ and its entry points need `no_mangle`, so the one-line binaries in
`fuzz_targets/` cannot forbid unsafe code. Everything they run lives in the
drivers in `src/`, which do forbid it, and which the stable replay test
`crates/pagura/tests/fuzz_regressions.rs` compiles as well: a stored input
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
| `document` | a PDF file | opening, lazy loading of up to 256 objects and their stream data, page enumeration and compilation of four pages, in both recovery modes |
| `render` | a PDF file | the first page renders to exactly `device_size` |
| `font` | selector byte (Type 1, TrueType, CFF, OpenType), program | `FontProgram::parse`, then outline and advance of every glyph, names, `cmap` and CID lookups |
| `image` | selector byte, then codec data or wrapper parts | `dct`, `ccitt` (strict and reported row recovery), `jbig2::decode`, `jpx::decode`, `decode_xobject` and `decode_inline`; samples match the reported size and bit depth |
| `text` | selector byte, string, wrapper parts | `PdfFont::load` and `decode`; glyph codes cover the string exactly; outlines of embedded programs and Type 3 procedures load |
| `content` | a content stream | `compile_page` on a fixed 32 × 32 page with one resource of each kind; a rendered tile equals the same pixels of the whole page (ADR 0004) |
| `host` | flags, page, scale, tile, PDF | hello, open, page_info, v1.1 generations with a current and a stale tile, render_tile, diagnostics and close through `pagura_host::Session`; one well-formed response per request in order, stale tiles skipped, the same tile identical through v1 and v1.1 |

Structured inputs split their parts at a line holding only `%%%%`. The
wrapper targets (`text`, `image` modes 4–7) turn the parts into objects 3,
4, … of a small, valid PDF after a fixed catalog (1) and empty page tree (2),
as in the generated fixtures. A part `<< entries >> stream`, followed by a
line break and the data, becomes a stream whose `/Length` the wrapper sets;
any other part is used as an object body verbatim. The selector bytes are
documented in `src/image.rs`, `src/text.rs` and `src/host.rs`.

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
`render`; the codec vectors for `filters`; the font vectors under each format
selector; the image, JBIG2 and JPEG 2000 vectors both as raw codec inputs and
wrapped in image XObjects; the text fixtures' objects, and every font vector
embedded in simple and composite font dictionaries; the pages' content
streams for `content`; and the classic, document and content PDFs behind a
host request header. Two review cases join the document seeds: a stream whose
`/Length` refers to its own object, and two streams whose lengths refer to
each other. When present, the pinned corpora in `$PAGURA_CORPORA_DIR` add
real-world files. Corpora (`corpus/`), crash artifacts (`artifacts/`) and
builds (`target/`) stay local.

## Findings

A failing input is minimized and kept as a regression:

```sh
cd fuzz && cargo +nightly fuzz tmin -O -a <target> artifacts/<target>/<crash-file>
```

The minimized input goes to `regressions/<target>/<short-name>`, with a
one-line note on the defect in `regressions/<target>/<short-name>.txt`.
`cargo test -p pagura --test fuzz_regressions` (part of `tools/check.py`)
replays every stored input on stable Rust and requires it to finish without
a failure. An input whose fix has not landed yet goes on the short
`KNOWN_FAILURES` list in that test, naming the defect; once it passes, the
test fails until the entry is removed. The fix itself comes with a minimized,
independently explained fixture in the owning area.
