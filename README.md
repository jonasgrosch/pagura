# Pagura

An independent PDF engine in safe Rust, designed for a maintainable viewer and
editor whose primary target is WebAssembly in the browser. **Early development,
not a complete PDF implementation.** See the live
[capability matrix](docs/STATUS.md) before using it with real documents.

Pagura does not fork, wrap, or depend on hayro, PDFium, MuPDF, or another PDF
engine. Those are external competitors. Product crates currently use only the
Rust standard library and enforce `#![forbid(unsafe_code)]`.

## Start here

```sh
python3 tools/check.py
mkdir -p work
cargo run --release -p pagura -- inspect corpus/generated/classic/rectangle.pdf
cargo run --release -p pagura -- render corpus/generated/classic/rectangle.pdf work/rectangle.ppm
cargo wasm   # pagura-host for wasm32-wasip1
python3 tools/bench.py run --engines pagura,poppler --suite smoke
python3 tools/bench.py gate benchmarks/results/local/latest.json
```

The complete release gate is deliberately unsatisfied until all required
engines, workloads, correctness evidence, and performance comparisons exist.
Passing bootstrap tests does not mean PDF conformance or 20% superiority.

- [Contributing](CONTRIBUTING.md)
- [Architecture and boundaries](docs/ARCHITECTURE.md)
- [Roadmap and milestones](docs/ROADMAP.md)
- [Decision records](docs/adr/)
- [Testing and conformance](docs/TESTING.md)
- [Performance contract](benchmarks/README.md)
- [Initial benchmark findings](benchmarks/results/INTERPRETATION.md)
- [Official references and acquisition](references/README.md)
- [Reference acquisition report](references/ACQUISITION.md)
- [Security model](SECURITY.md)
- [Corpus scoreboards](docs/scoreboard/)

## Reference and corpus setup

```sh
python3 tools/references.py sync
python3 tools/corpus.py fetch
python3 tools/competitors.py setup
python3 tools/bench.py doctor
```

Competitor setup also requires Node/npm, Poppler supplied by the OS, and Swift
on macOS for CoreGraphics. Competitors install outside the repository
(`$PAGURA_COMPETITORS_DIR`, ADR 0006).

Downloads are bounded and recorded with source URLs and SHA-256 hashes. Full
standards, upstream repositories, and competitor installations are cached under
`.cache/`, outside version control. No logins, purchases, license agreements,
or personal-data submissions are automated. Missing access is reported.

Rust 1.99.1 (latest stable) is pinned with the `wasm32-wasip1` target. Python
3.11+ runs development tooling using its standard library; optional competitor
environments have separate locked dependencies.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in Pagura
shall be dual licensed as above, without any additional terms or conditions.

The font metrics, font outlines, CMaps and Unicode data in [`data/`](data/README.md)
are third-party data under their own permissive licences (Adobe's AFM notice,
Apache-2.0, BSD-3-Clause and the Unicode License V3).
