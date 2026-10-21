#!/usr/bin/env python3
"""Incremental shared-opaque composition cost on independently correct outputs.

The separate rational probe measures accuracy-changing cases. This paired
matrix requires both binaries to match independent RGB and opaque alpha, then
each other. It preserves the complete glyph-memo control matrix. Neither a
synthetic target gain nor an accuracy improvement establishes release admission.
"""
from paired_render import run_experiment
import glyph_memo as memo
from fixturegen.pdf import document, opaque_rectangles, stream

EXTRA = ("opaque-clip", "opaque-tiny", "opaque-fill")
CASES = (*memo.CASES, *EXTRA)
MODES = ("exact", "competitive")
CELLS = (*memo.CELLS,
         *(("opaque64", mode, operation) for mode in MODES for operation in ("cold", "tile", "tiled")),
         *((name, mode, "cold" if name == "opaque-tiny" else "full") for name in EXTRA for mode in MODES))
PHASE_PLAN = {
    "pilot": {"pairs": 5, "warmups": 5, "seed": 20261026},
    "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261027},
}


def fixture(name):
    if name == "opaque-clip":
        return memo.clips.fixture("clip-opaque")
    if name in ("opaque-tiny", "opaque-fill"):
        size = 64 if name == "opaque-tiny" else 512
        x, y, width, height = (24, 24, 8, 8) if name == "opaque-tiny" else (0, 0, size, size)
        pdf = document([
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {size} {size}] /Resources << >> /Contents 4 0 R >>".encode(),
            stream(f"0 g {x} {y} {width} {height} re f".encode()),
        ])
        return pdf, opaque_rectangles(size, size, [((x, y, width, height), (0, 0, 0))])
    return memo.fixture(name)


if __name__ == "__main__":
    run_experiment(__file__, description=__doc__, experiment="shared-opaque-composition",
        cases=CASES, cells=CELLS, target_case="opaque64", fixture=fixture,
        fixture_sources=(memo.__file__, memo.legacy.__file__, memo.outlines.__file__, memo.clips.__file__),
        phase_plan=PHASE_PLAN,
        target_scope="Opaque direct-root rectangular text now reaches shared chunks and the retained glyph hull memo",
        limitations=("Synthetic mechanism fixtures are not representative document text",
                     "Rational accuracy-changing cases are evaluated separately, not admitted as equivalent baseline work",
                     "Opaque precision may increase storage, planning and validation cost",
                     "No competitor or release superiority claim follows from this matrix"))
