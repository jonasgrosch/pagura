#!/usr/bin/env python3
"""Audit the separately frozen background-guarded N16 output experiment."""
import sys

import n16_output_guarded as experiment
from evaluate_n16_output import audit, require


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_n16_output_guarded.py pilot-001|confirmation-001")
    audit(experiment_module=experiment, experiment_name="n16-output-guarded",
          stem="n16-output-guarded", local_name="n16-output-guarded", run=sys.argv[1],
          expected_harness_sources={
              "benchmarks/n16_output_guarded.py", "benchmarks/n16_output.py",
              "benchmarks/opaque_composition.py", "benchmarks/paired_render.py",
              "benchmarks/glyph_memo.py", "benchmarks/glyph_memo_fixtures.py",
              "benchmarks/scene_index.py", "benchmarks/clip_viewport_fixtures.py",
          }, audit_entrypoint="benchmarks/evaluate_n16_output_guarded.py")


if __name__ == "__main__":
    main()
