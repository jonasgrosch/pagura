# ADR 0026: The bundled-data crate implements the provider

Status: accepted, 2026-10-18. Refines ADR 0009 (bundled font data).

## Context

`pagura-data` depends on `pagura-text`, for the `Provider` trait and its
return types, and on `pagura-font`, for `FontProgram`. It turns its pinned
files into those types. It parses the Core14 AFM files and the cid2unicode
projection itself, and hands CMaps and font programs to the `pagura-text`
and `pagura-font` parsers.

The workspace review asked whether to invert this. In the alternative, a
dependency-free data crate would expose `&'static [u8]` lookups, and the
`Provider` adapter would live in the facade behind `bundled-data`. The
argument for it was that any parser change rebuilds about 10 MiB of
embedded data.

That rebuild was measured on an Apple M1 Max at the change that adds this
ADR, with dependencies already built, over three runs of
`cargo build -p pagura-data` after an edit to the crate:

| Profile | Rebuild time | `libpagura_data.rlib` |
|---|---|---|
| dev | 0.2 s | — |
| release | 0.9–1.2 s | 12.4 MiB |

`include_bytes!` produces constant byte slices that the compiler copies
without per-byte work. The embedded data therefore dominates the size of
the library, not its compile time.

## Decision

The direction stays `pagura-data ──► pagura-text, pagura-font`. The crate
owns exactly four things:

1. **The inventory.** This is one constant table per cargo feature. Each
   table maps a lookup key (a standard font's base name, a CMap name or a
   collection ordering) to its pinned file and, for font programs, to the
   format it is parsed as. A table is empty when its feature is off, so
   lookups find nothing and no data bytes are linked.
2. **Parsers for formats no other crate reads.** These are the Core14 AFM
   files and the cid2unicode projection. They charge `font_work` and
   `font_entries` (ADR 0016).
3. **Lazy, per-instance caches and the `Provider` implementation.** Each
   resource is parsed outside the cache lock and published once under its
   inventory key, so a cache never grows beyond the inventory.
4. **The licence notices of exactly the linked data.** `notices()` follows
   the same features that link the files.

It gains no logic that PDF interpretation depends on. Everything it returns
is a public `pagura-text` or `pagura-font` type, so a host provider can
replace it completely.

Reasons:

- Inverting would save about one second per release build, as measured
  above. That is less time than the parsers the rebuild waits on.
- Inverting would move the provider, its caches and both parsers into the
  facade, which is meant to stay a thin re-export layer plus the CLI. The
  alternative home is `pagura-text`, which would then gain formats it does
  not otherwise read.
- Licence obligations stay next to the data that incurs them.

## Consequences

- Changing `pagura-text` or `pagura-font` rebuilds `pagura-data`. This
  costs about a second in release.
- A host that loads data lazily (ADR 0009) cannot reuse the AFM and
  cid2unicode parsers, which are private. The trigger to revisit is a host
  provider that needs them. When one is built, the two parsers move to
  `pagura-text` as public constructors (`StandardMetrics` from an AFM file,
  `CidToUnicode` from a cid2unicode file). `pagura-data` then keeps only
  its tables, caches and notices, and the dependency direction stays.
- Revisit the direction itself if embedded data comes to dominate rebuild
  time, for example if the CJK substitutes ever become a default.
