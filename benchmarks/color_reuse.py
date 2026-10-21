#!/usr/bin/env python3
"""Color component reuse: fixed analytic preparation matrix, both accuracy modes."""
import argparse
from pathlib import Path
import subprocess
import sys

from paired_render import run_experiment
import color_reuse_fixtures as fixtures

# Integrated product before the component-reuse change; pristine control.
BASELINE_COMMIT = "c70b467882faafc9baede8c06c7011ac2be1ae21"
CASES = fixtures.CASES
CELLS = tuple((name, mode, "cold") for name in CASES for mode in ("exact", "competitive"))
TARGET_CASE, TARGET_OPERATION = "classic/many-operands", "cold"
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261108},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261109},
}
fixture = fixtures.fixture


def baseline_commit():
    if len(BASELINE_COMMIT) != 40 or any(c not in "0123456789abcdef" for c in BASELINE_COMMIT):
        raise ValueError("freeze the exact pristine pre-color-reuse baseline commit before launch")
    return BASELINE_COMMIT


def require_baseline(root):
    expected = baseline_commit()
    def git(*args):
        return subprocess.run(["git", "-C", str(Path(root).resolve()), *args],
                              check=True, capture_output=True, text=True, timeout=30).stdout
    if git("rev-parse", "HEAD").strip() != expected:
        raise ValueError("baseline differs from the frozen pre-change commit")
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("baseline must be pristine, including nonignored untracked files")


if __name__ == "__main__":
    if not any(arg in ("-h", "--help") for arg in sys.argv[1:]):
        parser = argparse.ArgumentParser(add_help=False)
        parser.add_argument("--baseline-source", type=Path, required=True)
        args, _ = parser.parse_known_args()
        require_baseline(args.baseline_source)
    run_experiment(__file__, description=__doc__, experiment="color-reuse",
        cases=CASES, cells=CELLS, target_case=TARGET_CASE, target_operation=TARGET_OPERATION,
        fixture=fixture, fixture_sources=(fixtures.__file__, fixtures.MANIFEST, fixtures.REFERENCES),
        phase_plan=PHASE_PLAN,
        target_scope="Open, compile and render the fixed many-operands page; unchanged full-validation compilation; both accuracy modes",
        limitations=("Cold denotes the internal open+compile+render interval, excluding file reads, output emission, process startup and destruction",
                     "The common public retained-list example is not the CLI render_context path or a competitor experiment",
                     "All19 original analytic PDFs and frozen zero-tolerance RGB oracles are retained, including mode-specific preexisting failures",
                     "No target mode or protected case may be removed or given a new pixel allowance after preflight",
                     "Only component replacement allocation is changed; retained path/color clones and source work remain",
                     "No pilot, historical or canceled samples enter confirmation",
                     "Partial experimental retention must disclose every latency/RSS miss; no release or dominance claim"))
