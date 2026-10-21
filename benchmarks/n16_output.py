#!/usr/bin/env python3
"""Measure exact N16 output conversion on the shared opaque stack."""
from paired_render import run_experiment
import opaque_composition as opaque

CASES, CELLS, fixture = opaque.CASES, opaque.CELLS, opaque.fixture
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261028},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261029},
}

if __name__ == "__main__":
    run_experiment(__file__, description=__doc__, experiment="n16-output",
        cases=CASES, cells=CELLS, target_case="opaque64", fixture=fixture,
        fixture_sources=(opaque.__file__, opaque.memo.__file__, opaque.memo.legacy.__file__,
                         opaque.memo.outlines.__file__, opaque.memo.clips.__file__),
        phase_plan=PHASE_PLAN,
        target_scope="Exact N16 output quantization of opaque text; both arms include shared precision and glyph memo",
        limitations=("Synthetic mechanism fixtures are not representative document text",
                     "Both arms retain the experimental shared-composition cost relative to the legacy route",
                     "No competitor or release superiority claim follows from this matrix"))
