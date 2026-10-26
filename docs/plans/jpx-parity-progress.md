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
features are R03; signed sYCC is invalid under Table I.10. General signed ICC
sample/profile combinations are not defined by ICC.
T.800 Table I.9 separately permits clipping negative inputs to a restricted
XYZ Gray/RGB matrix/TRC input/display profile. The existing ICC parser identifies
that subset; image metadata preserves the unsigned storage bias per component,
and sampling subtracts it before normalization, opacity and colour conversion.
General signed Lab/LUT profiles remain explicit Unsupported unless a later
restricted candidate applies. Additional baseline colour models and general
JPX compositing layers are not claimed by this integration bundle. A passed
fixture is not competitive family parity, and no performance claim is made.

The signed restricted-profile increment adds native-size Gray/RGB controls
at eight and sixteen bits. Its oracle subtracts the authoring/storage bias,
uses the full precision denominator from I.5.3.3, and clips negative device
inputs before the TRC. Each native-size signed case has both an unsigned JPX control with
the same ICC profile and an ordinary RGB control. The former isolates signed
sample interpretation even when the latter exposes colour or sampling policy.
Mixed signed/unsigned palette channels, opacity modes and candidate fallback
have additional formula-derived controls. Public tests preserve raw signed
samples and independent unsigned opacity. The complete set has 220 render
oracles; current implementation validation remains pending hosted execution.

A diagnostic-only hosted run at `cb444312450e900a5b23b7b344286ee510d63cd6`
retained the old signed rejection to measure the gap before the implementation:
[run 40341090579](https://github.com/jonasgrosch/pagura/actions/runs/40341090579).
Pagura passed 212 controls and returned its expected Unsupported error on the
four signed Gray/RGB 8/16-bit cases. The workflow therefore failed deliberately;
this is diagnostic evidence, not a successful acceptance run. Linux render-report
artifact `12660430830` has SHA-256
`74d52c3d16f0c01406c8708db9c73c18befa5762e5b171a246bc29386af8a743`;
macOS artifact `12660000540` has SHA-256
`213de85810c8ed9aa9befe7d7dccf415291d8aacf590fbdcb065f87c29ecefaa`.

PDF.js 6.4.299 renders each signed case byte-identically to its clipped unsigned
JPX control. In the eight-bit gray case both yield samples
`[0, 0, 0, 0, 64, 127]`, while the independent ICC-managed oracle and ordinary RGB
control yield `[0, 0, 0, 0, 137, 187]`. This supports the signed-sample clipping
policy while exposing a separate colour-policy difference; it does not establish
complete ICC output parity. MuPDF 1.28.5 and Quartz (macOS 26.6.2) pass the unsigned
ICC and ordinary RGB controls but differ on signed inputs. PDFium 156.0.8076.0
and hayro 0.8.0 differ on both signed and unsigned ICC cases, and their eight-bit
signed gray outputs retain the storage-offset sequence. Poppler 24.02.0 also
fails the ordinary RGB controls, so its signed observations remain unadjudicated.
No competitor source or measured pixel became an oracle.
