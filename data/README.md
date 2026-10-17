# Bundled data

Third-party data the engine needs but a PDF does not carry: font data for fonts a
PDF names but does not embed (ADR 0009), and Unicode 3.2.0 normalization data for
SASLprep (RFC 3454 section 4, ISO 32000-2 7.6.4.3.3). Everything here is
produced by `python3 tools/bundled_data.py build` from pinned upstream archives and
verified against `lock.json` by `python3 tools/bundled_data.py verify`, which
`tools/check.py` runs. Files are unmodified except three projections, which keep
selected original lines byte for byte: `cid2unicode.txt` (described in its own
header), `UnicodeData-3.2.0-normalization.txt` (code points with a nonzero
combining class or a decomposition; Hangul syllables decompose algorithmically)
and `DerivedNormalizationProps-3.2.0-nfkc.txt` (`Full_Composition_Exclusion`,
`NFKC_NO`, `NFKC_MAYBE`). Do not edit by hand.

| Path | Contents | Source | Licence |
|---|---|---|---|
| `afm/` | Metrics of the standard 14 fonts (9.6.2.2) | Adobe Core 14 AFM files | Adobe's notice in `afm/MustRead.html`, which must accompany them |
| `fonts/` | Arimo, Tinos and Cousine: outlines metric-compatible with Helvetica, Times and Courier | Google `croscorefonts` 1.31.0 | Apache-2.0 (`fonts/LICENSE-APACHE`, `fonts/NOTICE`) |
| `cmaps/<collection>/` | The predefined CMaps of ISO 32000-2 Table 116, and `cid2unicode.txt` for 9.10.2 | Adobe `cmap-resources` 20231115 | BSD-3-Clause (`cmaps/LICENSE.md`) |
| `unicode/` | Unicode 3.2.0 normalization data for NFKC in SASLprep; `NormalizationTest-3.2.0.txt` is test data only and is never linked into a binary | Unicode Character Database 3.2.0 | Unicode License V3 (`unicode/LICENSE`) |

`lock.json` maps each standard 14 font to its metrics and, where one exists,
its substitute outline. Symbol and ZapfDingbats have metrics but no
permissively licensed metric-compatible outline, so painting them without an
embedded program fails explicitly. A substituted outline uses the AFM advance
widths, and the substitution is reported as a diagnostic. A document's own
embedded program always takes precedence.

The data reaches the engine only through a provider, never by a lookup from
inside the PDF interpreter, so hosts can supply their own or load it lazily.

A binary that links this data (the `pagura-data` features `standard-fonts`
and `cmaps`) must reproduce its licence notices with its documentation;
`pagura_data::notices()` returns exactly the notices of the enabled features.
