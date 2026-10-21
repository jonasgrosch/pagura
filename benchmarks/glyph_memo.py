#!/usr/bin/env python3
"""Frozen incremental glyph-hull memo experiment on the combined renderer stack.

Use the coordinated one-job runner. Independent synthetic pixels and same-mode
bytes are required on every sample, including warmups. No broad text, competitor
or release-performance claim follows from this mechanism experiment.
"""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_render import run_experiment
import scene_index as legacy
import glyph_memo_fixtures as outlines
import clip_viewport_fixtures as clips

CASES = (*legacy.CASES, *outlines.CASES, "clip-churn", "image-only")
CELLS = (*legacy.CELLS, *((name, mode, "full") for name in (*outlines.CASES, "clip-churn", "image-only") for mode in legacy.MODES))
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261024},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261025},
}


def fixture(name):
    if name in outlines.CASES:
        return outlines.fixture(name)
    if name == "clip-churn":
        return clips.fixture(name)
    if name == "image-only":
        return outlines.image_only()
    return legacy.fixture(name)


if __name__ == "__main__":
    run_experiment(__file__, description=__doc__, experiment="stacked-root-glyph-hull-memo",
        cases=CASES, cells=CELLS, target_case="sparse64", fixture=fixture,
        fixture_sources=(legacy.__file__, outlines.__file__, clips.__file__), phase_plan=PHASE_PLAN,
        target_scope="First 64 root command ordinals, explicit TK false, repeated internal regions, at least two sources; primary target has 64 sources per run",
        limitations=("Synthetic integer outlines are not representative document text",
                     "Default-knockout groups and later ordinals are intentionally uncached",
                     "Opaque Exact text may bypass the compositor",
                     "Fixed memo storage remains present when disabled; RSS is measured separately"))
