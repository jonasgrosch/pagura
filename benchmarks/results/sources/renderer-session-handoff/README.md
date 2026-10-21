# Renderer session preservation

The entry point is [ADR 0033](../../../../docs/adr/0033-prepared-renderer-and-session-handoff.md).
The [inventory](../../renderer-session-archive.json) records the original heads,
source overlays and raw-evidence export hashes. Published source branches are:

- `codex/renderer-n16-interval`: combined experimental renderer, all linked
  measurements, final diagnosis, ADR and handoff entry points.
- `codex/unified-renderer-plan`: original proposal and first implementation/evidence history.
- `codex/renderer-stacked-gains`: earlier stacked experiments and their history.
- `codex/archive-renderer-baseline`: starting control `676a872e`.

Other listed control commits are reachable from those branches. Their IDs remain
in the inventory; detached worktrees need not be reset or converted to branches.
Parked candidate patches and failed attempts remain in source/evidence rather
than being applied to the combined product.

## Large evidence assets

GitHub draft release **renderer-session-handoff-2026-10-21** contains one gzip tar
per attached session worktree, archive-summary.json and SHA256SUMS. The release
is an archive container, not a renderer product release or admission. Owner
authentication is needed to access a draft. Its annotated tag identifies the
published handoff source. Fetch source normally and download evidence with:

```sh
git fetch origin codex/renderer-n16-interval codex/unified-renderer-plan codex/renderer-stacked-gains codex/archive-renderer-baseline
gh release download renderer-session-handoff-2026-10-21 --repo jonasgrosch/pagura --dir renderer-session-evidence
cd renderer-session-evidence
shasum -a 256 -c SHA256SUMS
mkdir combined
tar -xzf renderer-n16-interval.tar.gz -C combined
```

For a full payload check without extraction, run this directory's verify.py
with the download directory as its argument. It checks the archive checksums,
safe regular member paths, embedded inventory hashes, inclusion/exclusion sets,
sizes, control heads and every included file's bytes. The publication export
passed this check for all 85,573 included payloads. Export and validation
receipts are retained beside these instructions. attempt-001 preserves the
initial packaging/verification failure and superseded helper; only the corrected,
fully verified archive set is published as release assets.

Each archive has inventory.json, raw-local/ (the original ignored local evidence
tree), and, for controls, overlay/tracked.patch plus overlay/untracked/. Apply
overlays only to a fresh worktree at that archive's pinned head, never to a
measured control or latest main. Some overlays are superseded copies of changes
already integrated elsewhere, not additional accepted improvements.

Restore raw-local to a separately created historical checkout's
benchmarks/results/local only if that location is absent. Original records use
machine-specific absolute paths and binary locations. Restoring bytes does not
make old launch scripts portable; inspecting evidence and rerunning measurements
are different operations. A rerun needs new paths, input/settings/source pins,
binary identities and receipts. Never rewrite the old records for convenience.

The source repository is public. Full unrelated process-command inventories,
guard records containing foreign command matches, and credential-like material
are excluded. Their original hashes, sizes and omission reasons are inventoried.
No original evidence is modified or deleted. Public exports cannot support a
complete historical raw-receipt audit if that audit needs omitted private files.
Private/licensed reference caches and unrelated repositories/worktrees are out
of scope. Competitor source/dependency caches are not redistributed.

preserve.py created these exports serially under the cooperative runner and
known-process observer. It hashes included files while archiving, scans textual
material for credential patterns, refuses files changing during capture, never
follows symlinks, and preserves original control working trees. This is a
publication export with a stated scope, not a guarantee against every possible
secret format or a new benchmark result.

A fresh session should read the ADR and source reports before downloading all
assets. No multi-gigabyte evidence download or heavy test is needed merely to
understand the proposed architecture.
