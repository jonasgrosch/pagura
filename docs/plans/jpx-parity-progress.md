# R02 JPEG 2000 integration

The integration bundle is reconstructed directly on `render/one-pass` at
`73e1ab018d9c23f24c374de1f3a4441b119a5365`. Its original incremental source
branch at `842f2be70` remains preserved.
Reviewed source from PR #182 head `e16f5227f1290b51a7447ffd273f7dbf27e9c001`
supplies unsigned 1–16-bit and mixed-precision sYCC plus independent native-size
controls. PR #187 head `80e9aed8f8c7cbb1b23da7b536649582bc0607af` supplies matched
ordinary RGB/soft-mask controls. Those source branches remain untouched.

The bundle retains every colour candidate and selects by signed precedence,
approximation quality and stable file order. JP2 ignores the reserved priority
fields; compatible JPX uses T.801's fields. Unsupported ICC profiles and selected
forward transforms advance to the next candidate. Malformed metadata and budget
failures remain errors. Only exhaustion selects the ordinary-channel device
fallback. Cache identities include the profile candidate and effective image
rendering intent. Embedded colour does not consult PDF Default colour resources.

ICC Lab input now receives its physical Lab ranges. Stored premultiplied samples
are unassociated before mapping those ranges; Indexed samples instead select
preblended base colours before unassociation. Palette-backed explicit PDF Indexed
images retain one source index and independently mapped direct/palette opacity,
including nonzero reference origin and differing opacity precision. Ambiguous
independent source indices fail explicitly. Ignored opacity and ordinary alpha
share parsed channel definitions and the same bounded permutation; the duplicate
PDF-side box walker has been removed.

Inline framing and image decoding share resource resolution in `pagura-image`.
The interpreter no longer substitutes an ICC device name merely to count bytes.
Indirect ICC N and DeviceN names, including five-channel input and embedded EI
sample bytes, are independently exercised. JPX itself remains forbidden inline.

Primary sources reviewed: local ISO 32000-2 EC3 7.4.9, 8.9.7, Table 87 and 11.6;
T.800 (06/2019) I.3.3, I.5.3.3–I.5.3.6 and Table I.10; ICC.1:2010 6.3.4, 10.8,
10.10; IEC 61966-2-1 AMD1 Annex F. T.801 (08/2023) M.3.1 and M.11.7.2/Table M.23
were reviewed in the [ITU publication](https://www.itu.int/epublications/publication/itu-t-t-801-v3-2023-08-information-technology-jpeg-2000-image-coding-system-extensions).
No licensed standard text or third-party implementation was copied.

Independent fixtures cover colour-priority and profile cache disagreement,
malformed/unsupported candidates, inherited and explicit intent, Lab input,
alpha 0/partial/1, PDF palette replacement, groups, colour-key/external masks,
interpolation and resource-backed inline framing. Public image-source tests assert
exact raw index/opacity samples, metadata and output-budget errors. Host tests
reuse the oracles through native, WASI, client, partial-tile and viewer paths.
No tolerance was relaxed. Product runtime validation is pending hosted CI;
only source/format/lint checks and small independent fixture authoring ran locally
under ADR 0031.

Remaining boundaries: source precision above sixteen bits and full Part 2 decoder
features are R03; signed sYCC is invalid under Table I.10; signed ICC samples have
no standardized interpretation. Additional baseline colour models and general
JPX compositing layers are not claimed by this integration bundle. A passed
fixture is not competitive family parity, and no performance claim is made.
