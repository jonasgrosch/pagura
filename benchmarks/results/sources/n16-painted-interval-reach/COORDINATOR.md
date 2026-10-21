# Fixed native reach runner

`run.py` is an unexecuted, experiment-local coordinator helper. It imports only
the prepared candidate's existing fixture, identity, bounded-file and owned
process helpers. The fixed requests and acceptance conditions are in the code
and `README.md`; preparation cannot widen them. Stop on the first failure,
retain its report/log/output, and do not retry into the same directory.

Freeze a preparation JSON before collection:

```json
{
  "schema": 1,
  "instrumented_source": {"root": "/ABS/CANDIDATE", "commit": "40hex", "source_sha256": "64hex"},
  "pre_probe_source": {"root": "/ABS/CANDIDATE", "commit": "40hex", "source_sha256": "64hex"},
  "binary": {"path": "/ABS/ARCHIVED/scene_index_profile", "sha256": "64hex"},
  "patch": {"path": "/ABS/reach.patch", "sha256": "64hex"},
  "build_evidence": [
    {"path": "/ABS/completed-build-report.json", "sha256": "64hex"},
    {"path": "/ABS/final-build-receipt.json", "sha256": "64hex"},
    {"path": "/ABS/final-build-guard.json", "sha256": "64hex"}
  ]
}
```

The source recipe is the existing `paired_render.source_identity`. Build
evidence bytes are checked and copied; their terminal success and source/binary
correspondence remain independently reviewed coordinator obligations. This is
not a replacement build auditor. `build_evidence` may be omitted while drafting,
but the enclosing quality record still needs completed build evidence.

Run the four pure tests serially through the existing exclusive runner before
collection. They cover parser arithmetic, strict rejection and alpha without
fixtures or renderer children:

```sh
/ABS/PYTHON -B /ABS/test_run.py
```

Run collection with absolute paths, from the instrumented candidate, under the
existing runner and strict observer:

```sh
/ABS/PYTHON -B /ABS/run.py \
  --binary /ABS/ARCHIVED/scene_index_profile \
  --preparation /ABS/preparation.json --preparation-sha256 HEX \
  --output /ABS/NEW/reach-001
```

The script generates the eight existing harness fixtures once in its new output
directory; this is not repository corpus regeneration. The ninth request reuses
the empty PDF with an explicit nonopaque background. Each request runs in both
modes, with no retry. Actual output alpha is checked directly; actual RGBA is
never normalized. Trace parsing checks one actual plan, exact ordered chunk
partition, surface, every validated writer, reconstructed flat range and output
conversion counts. All raw combined logs and RGBA files remain available.

The report records loaded repository helper sources, its own source, Python
launcher/app paths, source identities before/after, binary before/after, all
input/oracle hashes and per-request artifact hashes. Imported module paths are
required to belong to the prepared checkout. The final report passes only with
all 18 independent RGBA and route checks. Stage times are retained raw solely
as request identity evidence and must not be summarized as speed measurements.

If `run_process` raises during cancellation or cleanup, its temporary combined
log is not exposed by that existing API. The report explicitly records that
absence, retains any already-written raw output and remains incomplete. No
cleanup uncertainty, missing log, source change or route miss can pass.

The outer final receipt/guard must authenticate the exact command, source pins,
report and raw artifacts. This script records their run ID/path pointer but
does not treat a running receipt snapshot as terminal evidence. After that
receipt, the coordinator removes exactly the probe, checks source restoration
against `pre_probe_source`, and pins a separate restoration record. None of
these instrumented results establish WASI, tile/tiled, all-52-cell reach, timing
benefit, or release eligibility.
