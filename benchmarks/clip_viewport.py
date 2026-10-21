#!/usr/bin/env python3
"""Fixed clipped-rectangle experiment for lazy viewport Path construction.

Use the coordinator's exclusive runner and contention guard. Both binaries use
the unchanged scene_index_profile public-API example. This is a native synthetic
comparison, not a release or competitor benchmark. The compiler may already
remove unused construction; this experiment makes no allocation-count claim.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_render import run_experiment
import clip_viewport_fixtures as fixtures

CASES = fixtures.CASES
MODES = ("exact", "competitive")
OPERATIONS = (
    ("clip-churn", "full"),
    ("clip-churn", "cold"),
    ("clip-churn", "tile"),
    ("clip-churn", "tiled"),
    ("clip-shared", "full"),
    ("clip-dense", "full"),
    ("clip-soft-source", "full"),
    ("clip-opaque", "full"),
    ("clip-tiny", "full"),
    ("clip-tiny", "cold"),
)
CELLS = tuple((name, mode, operation) for mode in MODES for name, operation in OPERATIONS)
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261022},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261023},
}


def main():
    run_experiment(__file__, description=__doc__, experiment="lazy-viewport-path-construction",
                   cases=CASES, cells=CELLS, target_case="clip-churn", fixture=fixtures.fixture,
                   fixture_sources=(fixtures.__file__,), phase_plan=PHASE_PLAN,
                   target_scope="distinct hard clips on disjoint integer rectangles; ordinary clip preparation",
                   limitations=("compiler may already eliminate unused viewport construction",
                                "no runtime allocation claim without separate evidence",
                                "dense control is pixel-dominated, not an all-overlap scene"))


if __name__ == "__main__":
    main()
