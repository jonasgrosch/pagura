#!/usr/bin/env python3
"""Audit the separately frozen u16 N16 output experiment."""
import sys

import n16_output_u16 as experiment
from evaluate_n16_output import ROOT, audit, load, require


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_n16_output_u16.py pilot-001|confirmation-001")
    expected = experiment.baseline_commit()
    results = ROOT / "benchmarks/results"
    plan = load(results / "n16-output-u16-plan.json")
    bindings = load(results / "n16-output-u16-bindings.json")
    require(plan["baseline_commit"] == expected, "plan baseline commit differs from the frozen driver")
    baseline = bindings["identities"]["baseline"]
    require(baseline["commit"] == expected, "baseline identity differs from the frozen guarded commit")
    experiment.require_baseline(baseline["root"])
    audit(experiment_module=experiment, experiment_name="n16-output-u16",
          stem="n16-output-u16", local_name="n16-output-u16", run=sys.argv[1],
          expected_harness_sources={
              "benchmarks/n16_output_u16.py", "benchmarks/n16_output_guarded.py",
              "benchmarks/n16_output.py", "benchmarks/opaque_composition.py",
              "benchmarks/paired_render.py", "benchmarks/glyph_memo.py",
              "benchmarks/glyph_memo_fixtures.py", "benchmarks/scene_index.py",
              "benchmarks/clip_viewport_fixtures.py",
          }, audit_entrypoint="benchmarks/evaluate_n16_output_u16.py")


if __name__ == "__main__":
    main()
