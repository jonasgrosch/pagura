#!/usr/bin/env python3
"""Rectangle reservation: fixed analytic19 preparation matrix, both modes."""
import argparse
from pathlib import Path
import subprocess
import sys

from paired_render import run_experiment
import color_reuse_fixtures as fixtures

BASELINE_COMMIT = "52d8e7f41e2bfc4d38bff394adcb2f4a1783a8c2"
CASES = fixtures.CASES
CELLS = tuple((name, mode, "cold") for name in CASES for mode in ("exact", "competitive"))
TARGET_CASE, TARGET_OPERATION = "classic/many-operands", "cold"
TARGET_CASES = (TARGET_CASE, "classic/many-rectangles")
TARGET_PRIORITY = {TARGET_CASE: "primary", TARGET_CASES[1]: "secondary"}
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261111},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261112},
}
fixture = fixtures.fixture


def require_baseline(root):
    def git(*args):
        return subprocess.run(["git", "-C", str(Path(root).resolve()), *args],
                              check=True, capture_output=True, text=True, timeout=30).stdout
    if git("rev-parse", "HEAD").strip() != BASELINE_COMMIT:
        raise ValueError("baseline differs from the frozen precision-fix commit")
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("baseline must be pristine, including nonignored untracked files")


if __name__ == "__main__":
    if not any(arg in ("-h", "--help") for arg in sys.argv[1:]):
        parser = argparse.ArgumentParser(add_help=False)
        parser.add_argument("--baseline-source", type=Path, required=True)
        args, _ = parser.parse_known_args()
        require_baseline(args.baseline_source)
    run_experiment(__file__, description=__doc__, experiment="rectangle-reserve",
        cases=CASES, cells=CELLS, target_case=TARGET_CASE, target_cases=TARGET_CASES,
        target_operation=TARGET_OPERATION, fixture=fixture,
        fixture_sources=(fixtures.__file__, fixtures.MANIFEST, fixtures.REFERENCES),
        phase_plan=PHASE_PLAN,
        target_scope="Internal open+compile+render: primary many-operands and secondary many-rectangles; both modes",
        limitations=("Cold is the in-memory open+compile+render interval; it excludes file reads, output emission, process startup and destruction",
                     "Fixed independent analytic19 only; not CLI render_context, browser/WASM, competitor or full-corpus evidence",
                     "All76 preflight outputs must pass before timing; every timed and warmup sample requires exact oracle RGB, opaque alpha and same-mode paired bytes",
                     "No pilot, historical, failed or canceled samples enter confirmation",
                     "Marginal percentile-bootstrap intervals do not guarantee p95 precision, temporal independence or simultaneous coverage",
                     "Primary/secondary priorities and partial-retention criteria are frozen in the plan; no automatic retention or release admission"))
