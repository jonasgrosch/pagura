#!/usr/bin/env python3
"""Bounded N16 painted-interval experiment on the reconciled retained stack."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from paired_render import ROOT, run_experiment, source_identity
from benchkit.artifacts import hash_regular_file, read_bounded_regular
import n16_output_u16 as previous
import n16_painted_interval_fixtures as intervals

CASES = (*previous.CASES, *intervals.CASES)
CELLS = (*previous.CELLS,
         *((name, mode, "full") for name in (*intervals.CASES, "opaque-tiny")
           for mode in ("exact", "competitive")))
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261105},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261106},
}


def fixture(name):
    return intervals.fixture(name) if name in intervals.CASES else previous.fixture(name)


def frozen_plan():
    plan = json.loads(read_bounded_regular(ROOT / "benchmarks/results/n16-painted-interval-plan.json", 2**20))
    if plan.get("experiment") != "n16-painted-interval" or plan.get("status") != "frozen-before-pilot":
        raise ValueError("painted-interval plan is not frozen")
    for side in ("baseline", "candidate"):
        identity = plan["identities_to_freeze"][side]
        for name, length in (("commit", 40), ("source_sha256", 64), ("binary_sha256", 64)):
            value = identity.get(name)
            if not isinstance(value, str) or len(value) != length or any(c not in "0123456789abcdef" for c in value):
                raise ValueError("unresolved frozen identity: " + side + "/" + name)
        if not all(isinstance(identity.get(name), str) and Path(identity[name]).is_absolute()
                   for name in ("root", "binary")):
            raise ValueError("unresolved frozen source/binary path: " + side)
    return plan


def baseline_commit():
    return frozen_plan()["identities_to_freeze"]["baseline"]["commit"]


def require_baseline(root):
    """Require the coordinator's frozen baseline, after any retained preceding work."""
    def git(*args):
        return subprocess.run(["git", "-C", str(Path(root).resolve()), *args],
                              check=True, capture_output=True, text=True, timeout=30).stdout
    if git("rev-parse", "HEAD").strip() != baseline_commit():
        raise ValueError("baseline must match the frozen incremental comparison commit")
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("baseline checkout must be pristine, including nonignored untracked files")


if __name__ == "__main__":
    if not any(arg in ("-h", "--help") for arg in sys.argv[1:]):
        preflight = argparse.ArgumentParser(add_help=False)
        for side in ("baseline", "candidate"):
            preflight.add_argument("--" + side + "-source", required=True, type=Path)
            preflight.add_argument("--" + side, required=True, type=Path)
        args, _ = preflight.parse_known_args()
        require_baseline(args.baseline_source)
        plan = frozen_plan()
        for side in ("baseline", "candidate"):
            expected = plan["identities_to_freeze"][side]
            actual = source_identity(getattr(args, side + "_source"))
            binary = getattr(args, side).resolve()
            if any(actual[key] != expected[key] for key in actual) or str(binary) != expected["binary"] \
                    or hash_regular_file(binary) != expected["binary_sha256"]:
                raise ValueError("source/binary differs from frozen plan: " + side)
    guarded = previous.previous
    n16 = guarded.previous
    opaque = n16.opaque
    run_experiment(__file__, description=__doc__, experiment="n16-painted-interval",
        cases=CASES, cells=CELLS, target_case="opaque64", fixture=fixture,
        fixture_sources=(intervals.__file__, previous.__file__, guarded.__file__, n16.__file__, opaque.__file__,
                         opaque.memo.__file__, opaque.memo.legacy.__file__,
                         opaque.memo.outlines.__file__, opaque.memo.clips.__file__),
        phase_plan=PHASE_PLAN,
        target_scope="Skip only N16 output conversion outside a conservative flat painted interval; both modes",
        limitations=("All previous 46 cells and roles are retained; six protected full-render controls are added",
                     "Profile uses default Repair open and full compile_page; diagnostics are not emitted",
                     "Only empty and distant are new cases; opaque-tiny already exists with cold cells",
                     "Full surface initialization, interval holes, source work and allocation admission remain",
                     "Actual N16 route and interval reach require separate untimed structural evidence",
                     "Native/WASI enclosing layout, accuracy and resource gates remain mandatory",
                     "No historical, pilot or canceled samples are reused in confirmation",
                     "No competitor, release or net-stack superiority claim follows from this matrix"))
