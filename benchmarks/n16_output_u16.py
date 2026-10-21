#!/usr/bin/env python3
"""Exact u16 N16 output quantizer against the committed guarded u32 path."""
import argparse
from pathlib import Path
import subprocess
import sys

from paired_render import run_experiment
import n16_output_guarded as previous

# Replace only after the guarded experiment is complete and committed, then
# freeze this file, the plan and all bindings before any new measurement.
BASELINE_COMMIT = "f98ad8365b46d500cbb866b13fcea0b0cbb28f7f"
CASES, CELLS, fixture = previous.CASES, previous.CELLS, previous.fixture
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261101},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261102},
}


def baseline_commit():
    if (not isinstance(BASELINE_COMMIT, str) or len(BASELINE_COMMIT) != 40
            or any(c not in "0123456789abcdef" for c in BASELINE_COMMIT)):
        raise ValueError("supply the completed guarded experiment's pristine baseline commit before freezing")
    return BASELINE_COMMIT


def require_baseline(root):
    """Read-only launch/audit check; binary correspondence still needs a build receipt."""
    expected = baseline_commit()
    def git(*args):
        return subprocess.run(["git", "-C", str(Path(root).resolve()), *args],
                              check=True, capture_output=True, text=True, timeout=30).stdout
    if git("rev-parse", "HEAD").strip() != expected:
        raise ValueError("baseline checkout does not match the frozen guarded commit")
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("baseline checkout must be pristine, including nonignored untracked files")


if __name__ == "__main__":
    if not any(arg in ("-h", "--help") for arg in sys.argv[1:]):
        preflight = argparse.ArgumentParser(add_help=False)
        preflight.add_argument("--baseline-source", required=True, type=Path)
        args, _ = preflight.parse_known_args()
        require_baseline(args.baseline_source)
    n16 = previous.previous
    opaque = n16.opaque
    run_experiment(__file__, description=__doc__, experiment="n16-output-u16",
        cases=CASES, cells=CELLS, target_case="opaque64", fixture=fixture,
        fixture_sources=(previous.__file__, n16.__file__, opaque.__file__,
                         opaque.memo.__file__, opaque.memo.legacy.__file__,
                         opaque.memo.outlines.__file__, opaque.memo.clips.__file__),
        phase_plan=PHASE_PLAN,
        target_scope="Exact u16 N16 output conversion versus the guarded u32 converter; both accuracy modes",
        limitations=("Baseline is the committed guarded converter; retained gains are incremental to that stack",
                     "Synthetic mechanism fixtures are not representative document text",
                     "Both arms retain the nonopaque-background F64 guard and shared composition",
                     "This matrix uses opaque backgrounds; nonopaque background accuracy remains independently gated",
                     "No historical, pilot or canceled-confirmation samples are reused",
                     "No competitor or release superiority claim follows from this matrix"))
