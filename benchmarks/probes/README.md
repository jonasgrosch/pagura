# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
embedded-file bytes and text** through the pinned PDFium and MuPDF Python bindings.
The nine authored cases include populated/empty controls and a non-PDF rejection
control. All six baseline columns are retained: pdf.js, Poppler, hayro and Quartz
currently record `adapter-unavailable` for this slice. That says nothing about
their engine support. G01 remains open.

The [manifest](document-services.json) owns exact expectations. The fixture builder
in [capability_probes.py](../../tools/capability_probes.py) writes small, synthetic
classic-xref PDFs without using Pagura or a competitor writer. The expectations
are not generated from measured output or majority agreement. Text normalization
only replaces CRLF with LF and trims leading/trailing LF; whitespace and character
differences otherwise fail. Attachment names and SHA-256 payload hashes must match.

The APIs come from the official [pypdfium2 API](https://pypdfium2.readthedocs.io/en/stable/python_api.html)
and [PyMuPDF document](https://pymupdf.readthedocs.io/en/latest/document.html) /
[page](https://pymupdf.readthedocs.io/en/latest/page.html) documentation. This uses
their existing `uv.lock` pins, not a new baseline. The binding and backend versions,
native binary hashes and selected lock entries are recorded by the existing
benchmark identity code. No competitor implementation source was consulted.

## Execution and evidence

The hosted [capability workflow](../../.github/workflows/capabilities.yml) runs
source checks and harness controls on Ubuntu, macOS and Windows, then the actual
initial engine probes on Ubuntu. Packages are installed under the runner's
temporary directory, outside the product checkout. No product dependencies change.
While ADR 0031 reserves the local machine, run engines/tests only in hosted CI.

```sh
# Source-only validation, without imports or invocation of external engines:
python tools/capability_probes.py check
# Hosted execution after the pinned bench group has been installed out of tree:
python tools/capability_probes.py run --output /tmp/capability-report.json
```

Each workflow publishes `capability-document-services`, containing a JSON report
with the commit, source/manifest/adapter/lock hashes, fixture hashes, platform,
engine identities and all 54 case/engine results. Absolute workspace/cache paths
are replaced by placeholders. Diagnostics retain byte counts and hashes rather
than raw native logs. The report is preserved even if a selected probe fails.

`pass` requires the independent expectation, or an engine rejection for the
invalid-input control. A wrong result is `incorrect`; a rejection of valid input
is `rejected`. `unsupported` requires a named explicit backend refusal; the initial
adapters do not invent one from exceptions. Timeout, adapter errors, absent APIs
and unavailable engine installations remain distinct outcomes. Selected engines
must pass every case for this workflow to succeed; missing adapters in the other
four columns remain visible and cannot establish release readiness.

The offline harness tests include wrong values, boolean/numeric confusion,
invalid envelopes, missing/duplicate cells, unattested passes, malformed output,
timeouts and missing executables. They use stubs, not competitor libraries.

## Next G01 increments

- Add equivalent public-API adapters for pdf.js, Poppler, hayro and CoreGraphics;
  record API absence separately from wrapper or build-configuration absence.
- Add Pagura observations through its actual public surfaces, including explicit
  missing typed metadata/attachment APIs, before claiming comparative parity.
- Probe tag subvariants, optional XFA/V8 builds, forms/events, annotation/layer
  state, save/reopen, redaction and signatures, each with independent controls.
- Expand platform/configuration coverage and bind current results to each scoped
  capability subvariant. A successful basic fixture never closes an entire
  [capability family](../../docs/CAPABILITIES.md) or a release gate.
