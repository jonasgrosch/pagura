#!/usr/bin/env python3
"""Fixed incremental F32 painted-output interval experiment; no release admission."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from paired_render import ROOT, run_experiment, source_identity
from benchkit.artifacts import hash_regular_file, read_bounded_regular
import n16_painted_interval as previous
import f32_painted_interval_fixtures as dynamic

BASELINE_COMMIT = "f9738174757252567730ff62b5d36620c01d1e6c"
CASES = (*previous.CASES, *dynamic.CASES)
CELLS = (*previous.CELLS, *((name, mode, "full") for name in dynamic.CASES
                          for mode in ("exact", "competitive")))
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261109},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261110},
}
TARGET_CASE, TARGET_OPERATION = "sparse64", "full"
u16 = previous.previous
guarded, n16 = u16.previous, u16.previous.previous
opaque = n16.opaque
FIXTURE_SOURCES = (dynamic.__file__, previous.__file__, previous.intervals.__file__,
                   u16.__file__, guarded.__file__, n16.__file__, opaque.__file__,
                   opaque.memo.__file__, opaque.memo.legacy.__file__,
                   opaque.memo.outlines.__file__, opaque.memo.clips.__file__)


def fixture(name):
    return dynamic.fixture(name) if name in dynamic.CASES else previous.fixture(name)


def frozen_plan():
    path = ROOT / "benchmarks/results/f32-painted-interval-plan.json"
    plan = json.loads(read_bounded_regular(path, 2**20))
    cells = [{"case": name, "mode": mode, "operation": op,
              "role": "target" if (name, op) == (TARGET_CASE, TARGET_OPERATION) else "protected"}
             for name, mode, op in CELLS]
    if (plan.get("experiment") != "f32-painted-interval" or plan.get("status") != "frozen-before-pilot"
            or plan.get("phases") != PHASE_PLAN or plan.get("cells") != cells
            or len(CASES) != 20 or len(set(CASES)) != 20
            or len(CELLS) != 56 or len(set(CELLS)) != 56):
        raise ValueError("F32 interval plan is not the complete frozen experiment")
    if plan["identities_to_freeze"]["baseline"]["commit"] != BASELINE_COMMIT:
        raise ValueError("baseline must be the retained N16 interval commit")
    return plan


def baseline_commit():
    return BASELINE_COMMIT


def require_baseline(root):
    def git(*args):
        return subprocess.run(["git", "-C", str(Path(root).resolve()), *args],
                              check=True, capture_output=True, text=True, timeout=30).stdout
    if git("rev-parse", "HEAD").strip() != BASELINE_COMMIT or git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("baseline must be pristine retained f9738174")


def record_driver_start():
    """Record the actual post-exec argv; a start is never completion evidence."""
    from f32_interval_admission import LOCAL, NAME, pin, require
    item = {"path": os.environ.get("PAGURA_F32_LAUNCH_INTENT", ""),
            "sha256": os.environ.get("PAGURA_F32_LAUNCH_SHA256", "")}
    intent = pin(item)
    phase = intent["phase"]
    require(phase in PHASE_PLAN and intent["experiment"] == NAME
            and item["path"] == str(LOCAL / (phase + "-001-launch.json"))
            and intent["status"] == "launch-intent-not-completion" and sys.orig_argv == intent["command"]
            and intent["run_id"] == os.environ.get("PAGURA_RUN_ID")
            and intent["run_receipt"] == os.environ.get("PAGURA_RUN_RECEIPT"), "Missing actual launch/run binding")
    with (LOCAL / (phase + "-001-driver-start.json")).open("x") as handle:
        json.dump({"schema": 1, "experiment": NAME, "phase": phase, "status": "driver-start-not-completion",
                   "config": intent["config"], "intent": item, "argv": sys.orig_argv,
                   "run_id": intent["run_id"], "run_receipt": intent["run_receipt"]}, handle, indent=2)
        handle.write("\n")


def main():
    if not any(arg in ("-h", "--help") for arg in sys.argv[1:]):
        parser = argparse.ArgumentParser(add_help=False)
        for side in ("baseline", "candidate"):
            parser.add_argument("--" + side + "-source", required=True, type=Path)
            parser.add_argument("--" + side, required=True, type=Path)
        args, _ = parser.parse_known_args()
        require_baseline(args.baseline_source)
        plan = frozen_plan()
        for side in ("baseline", "candidate"):
            expected = plan["identities_to_freeze"][side]
            actual = source_identity(getattr(args, side + "_source"))
            binary = getattr(args, side).resolve()
            if any(actual[key] != expected[key] for key in actual) or str(binary) != expected["binary"] \
                    or hash_regular_file(binary) != expected["binary_sha256"]:
                raise ValueError("source/binary differs from frozen plan: " + side)
        record_driver_start()
    run_experiment(__file__, description=__doc__, experiment="f32-painted-interval",
        cases=CASES, cells=CELLS, target_case=TARGET_CASE, target_operation=TARGET_OPERATION,
        fixture=fixture, fixture_sources=FIXTURE_SOURCES, phase_plan=PHASE_PLAN,
        target_scope="F32 painted-output interval on sparse64/full in both modes, incremental to retained N16 interval",
        limitations=("All52 prior cells retained; sparse64/full now targets and opaque64/full protected",
                     "Four dense shading/shading-pattern controls added; existing dense image-only retained",
                     "Default Repair opening and full compile_page; diagnostics not emitted",
                     "Fresh one-thread native samples; hashes and preparation warm filesystem caches",
                     "Expected F32 dynamic routes require separate untimed source/route evidence",
                     "Shading-pattern Exact is protected F64; Competitive is expected F32",
                     "All correctness/resource/layout/portability checks and strict RGB/alpha/parity are mandatory",
                     "No historical or pilot sample reuse, changed gates, automatic retry or competitor claim"))


if __name__ == "__main__":
    main()
