# Main reconciliation validation sources

This package checks the merge of main `d81d0c82` into experimental parent
`237f134a`. It measures no speed or memory benefit. Existing experiment
sources, samples, gates and unsuccessful attempts retain their identities.

- `check.py` wraps one command with the existing owned-child process lifecycle,
  bounded log, timeout, single Cargo job/test thread, and unchanged-source
  check. It refuses to overwrite an existing report or log.
- `run_wasi_tests.mjs` logs the path and SHA-256 of the exact module bytes it
  compiles, passes through Cargo's test arguments, and uses no filesystem
  preopens. The focused tests construct their inputs programmatically.
- `parity.py` builds the unchanged profile example for native, WASI SIMD and
  WASI scalar, serially. The two WASI targets use separate directories and
  explicit `+simd128`/`-simd128` flags with encoded flags removed. It retains
  the Cargo-selected artifacts and checks four frozen analytic image scenes,
  both modes, against strict RGB bytes and literal opaque alpha: 24 outputs.
  Every same-mode platform trio must also be byte-identical. This exercises
  full compilation plus retained-list rendering, not drawing-only validation
  or proof of specialized direct-image dispatch. The strict fractional-image
  check is stronger than its older general Competitive CLI allowance.
- `summarize.py` verifies terminal runner/observer receipts, unchanged sources,
  logs, module hashes and every retained parity artifact. It separately reads
  the stored PPM references, inserts opaque alpha, checks all 24 RGBA outputs,
  and writes the committed reconciliation record. It does not render again.

Root owns all execution under the shared runner and strict five-second
known-process observer. The observer can miss short or unrecognized jobs;
the evidence is cooperative local provenance, not hostile-host attestation.
The producer records both requested and observed Python executable identities,
including the macOS framework/app handoff. Compiler/runtime closure is not
claimed to be complete.

The existing `run_process` helper leaves a provisional `status: failed` on
ordinary zero-exit commands pending caller classification. For these validation
records, `command-passed`, exit zero, unchanged source and a completed outer
receipt establish completion; a timeout, nonzero exit or incomplete receipt
does not. Raw helper fields remain unchanged.

Independent rational sampler tests cover irregular tiles and scratch/work
boundaries. Neither these tests nor the four cross-platform image scenes prove
all-PDF accuracy or a universal tie choice. The four historical tie-byte cases
behind main's sampler correction were not reconstructed from unbound inputs.
