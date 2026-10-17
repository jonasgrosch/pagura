# Fuzzing

Coverage-guided fuzzing of the product crates with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer). This is test
tooling in its own Cargo workspace. libFuzzer is C++ and its entry points need
`no_mangle`, so these four target binaries cannot forbid unsafe code; nothing
here is ever a product dependency, and every product crate still forbids it.

| Target | What it checks |
|---|---|
| `syntax` | object parsing in strict and repair mode never panics |
| `filters` | every decoder respects its output cap and never panics |
| `document` | opening, page enumeration and content compilation never panic |
| `render` | rendering returns a surface of the advertised size or an error |

```sh
rustup toolchain install nightly          # cargo-fuzz needs nightly
cargo install --locked cargo-fuzz
python3 fuzz/seed.py                      # seeds from fixtures and pinned corpora
cd fuzz && cargo +nightly fuzz run -O document corpus/document -- -max_total_time=600
```

Seed and evolved corpora (`corpus/`) and crash artifacts (`artifacts/`) stay
local. A crash becomes a minimized, independently explained fixture in the
owning area plus a regression test, then the fix.
