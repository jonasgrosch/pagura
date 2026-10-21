#!/usr/bin/env python3
"""Revised exact N16 output experiment after the background accuracy guard."""
from paired_render import run_experiment
import n16_output as previous

CASES, CELLS, fixture = previous.CASES, previous.CELLS, previous.fixture
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261030},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261031},
}

if __name__ == "__main__":
    opaque = previous.opaque
    run_experiment(__file__, description=__doc__, experiment="n16-output-guarded",
        cases=CASES, cells=CELLS, target_case="opaque64", fixture=fixture,
        fixture_sources=(previous.__file__, opaque.__file__, opaque.memo.__file__,
                         opaque.memo.legacy.__file__, opaque.memo.outlines.__file__, opaque.memo.clips.__file__),
        phase_plan=PHASE_PLAN,
        target_scope="Exact N16 output quantization with nonopaque backgrounds excluded from narrow storage",
        limitations=("Synthetic mechanism fixtures are not representative document text",
                     "This matrix uses opaque backgrounds; nonopaque background accuracy is independently gated",
                     "Nonopaque backgrounds now pay existing F64 work/storage costs and may reach limits sooner",
                     "No pilot or canceled confirmation samples are reused",
                     "Both arms retain experimental shared-composition cost relative to the legacy route",
                     "No competitor or release superiority claim follows from this matrix"))
