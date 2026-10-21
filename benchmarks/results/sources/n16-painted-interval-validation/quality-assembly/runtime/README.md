# Frozen N16 quality aggregation

Unexecuted source-only draft. Copy both scripts together to a coordinator-owned
archive before freezing. No crate, benchmark, fixture or test source is changed.
The input manifest is created **after** the underlying checks; it is a pinned
aggregation input, not a retrospective claim that these pins existed before
their original executions. Final original receipts remain the trust boundary.

`freeze.py` reads the actual completed staged-check-001, diagnostics-006,
precision-host-001 and its two builds, reach-r2-build-001, reach-r2-001 and both
restoration records. It binds their final receipts, reports, raw-artifact
inventories, exact recorded argv and observer snapshots. It retains all terminal
unsuccessful top-level attempts, the historical ledger, and R1 separately.
It creates no passing quality artifact. Review the resulting JSON before use.

The `--source-review` argument is an **explicit coordinator attestation** of the
six checks listed in `assemble.REVIEW`, not arbitrary documentation. Supplying
it authorizes recording those checks as reviewed. The current root-authored
`quality-assembly/source-review.md` covers the production writer/layout/error
scope. Host build correspondence is additionally checked against the Cargo
executable paths, final build receipts' actual binary hashes, and the later
host probe's binary hashes.

Use the same exact absolute Python app launcher, cwd and inherited environment
as the staged plan. In particular the plan records `LANG=C.UTF-8`,
`PYTHONDONTWRITEBYTECODE=1`, `CARGO_BUILD_JOBS=1`, `RUST_TEST_THREADS=1` and its full
PATH/environment. `assemble.py` calls the original pinned staged verifier and
refuses environment drift. Do not add `-B` to the assembler command: its precise
argv is stored by the freeze helper. Both actual Python framework executable
and app launcher are pinned. The manifest contains a literal
`{manifest_sha256}` argv slot to avoid a self-hash cycle.

Sequential coordinator commands (replace absolute placeholders):

```text
PYTHON_APP ARCHIVE/freeze.py --root CANDIDATE --output NEW_MANIFEST.json \
  --quality-output NEW_QUALITY.json --source-review SOURCE_REVIEW.md
```

The helper prints the manifest SHA. Review identities and expected job commands,
then run this exact child under the existing runner and strict observer:

```text
PYTHON_APP ARCHIVE/assemble.py --manifest NEW_MANIFEST.json \
  --manifest-sha256 PRINTED_SHA --output NEW_QUALITY.json
```

Declare the new quality JSON as an outer runner artifact. After terminal success,
pin that **final** receipt and the quality JSON in launcher prerequisite
`quality.{receipt,artifact}`. The assembler validates its live runner identity
but cannot embed or verify its own future final receipt; that last check is the
launcher's responsibility. A failed/interrupted assembly writes a new explicit
failed-or-incomplete artifact with every gate pending and raises. It starts no
child processes beyond the shared source identity's read-only Git queries.

The final artifact passes only after revalidating all 14 staged commands and
every attempt; six successful builds/test logs, required test names, 47 ordered
Q4 records per arm and all eight raw types' layout non-growth; two strict
rational native/WASI host exchanges; and the exact fixed R2 24-request raw
pixels and route/count/coverage predictions. Eight enclosing native/WASI layout
rows are emitted only after SIMD/scalar agreement. The historical R1 failure
is never relabelled. Raw log parsers are reused from pinned executed helpers;
this is not an independent reimplementation of their grammar.

Ordinary source review and trusted generator/build correspondence remain explicit
limits. Native route evidence does not establish every timed cell or WASI route.
No timing, RSS, peak-stack, release-admission or engine-superiority result is
created. The **104-output native preflight remains a separate prerequisite**;
this helper does not replace or claim it passed.

Tests, syntax checks, imports and aggregation have not been run by the drafting
agent. Root owns execution and any necessary correction of the unexecuted draft.
