# Retained shading diagnostics

These files preserve the original scratch diagnostic attempts without changing
their absolute paths, metadata or hashes. run-001 failed a fixture-selector
assertion before a renderer was started. run-002 completed three serial profiles.
See [the report](../../shading-architecture-investigation.md) for interpretation.

Original unrelated process inventories `observer/snapshot-*.txt` are omitted
from public source. Original summary/receipt references to those files remain;
do not claim a complete raw observer re-audit from this publication alone.
All other copied regular files are byte-for-byte originals. The session
preservation archive contains a full per-file inclusion/exclusion inventory.

profile.py is a historical, machine-specific orchestration helper. It invokes
the existing CLI, not new renderer code, and is not a portable current-main
runner. Reconstruct inputs/binaries from the bound report and create new paths,
run identity and source bindings for a rerun. Do not overwrite its old evidence.
