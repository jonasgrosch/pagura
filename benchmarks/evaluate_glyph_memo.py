#!/usr/bin/env python3
"""Recheck frozen glyph-memo artifacts and summarize all declared cells."""
import hashlib
import json
from pathlib import Path
import statistics
import sys

import glyph_memo as experiment
from paired_render import ROOT, expected_image
from benchkit.images import normalize


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    run = sys.argv[1]
    if run not in ("pilot-001", "confirmation-001"):
        raise ValueError("expected the single declared pilot or confirmation")
    results = ROOT / "benchmarks/results"
    local = results / "local/glyph-memo"
    directory = local / run
    report_path = directory / "report.json"
    report = json.loads(report_path.read_text())
    receipt = json.loads((local / (run + "-receipt.json")).read_text())
    guard_path = local / (run + "-observer/guard.json")
    guard = json.loads(guard_path.read_text())
    bindings = json.loads((results / "glyph-memo-bindings.json").read_text())
    plan = json.loads((results / "glyph-memo-plan.json").read_text())
    assert report["status"] == "complete-exploratory"
    assert receipt["status"] == "completed" and receipt["source_unchanged"]
    assert receipt["exit_code"] == receipt["wrapper_exit_code"] == 0
    assert not receipt["errors"] and not receipt["artifact_errors"]
    assert guard["status"] == "completed-observation-only" and guard["exit_code"] == 0
    for path in (report_path, guard_path):
        assert receipt["artifacts"][str(path)] == digest(path)
    assert report["identities"] == bindings["identities"]
    for name, expected in bindings["bound_files_sha256"].items():
        assert digest(ROOT / name) == expected, name
    for name, expected in report["harness_sources_sha256"].items():
        assert bindings["bound_files_sha256"][name] == expected
    assert report["thresholds"] == plan["thresholds"]
    phase = plan["phases"][report["phase"]]
    assert {name: report[name] for name in phase} == phase
    key = lambda cell: (cell["case"], cell["mode"], cell["operation"])
    expected_cells = {key(cell): cell["role"] for cell in plan["cells"]}
    assert len(expected_cells) == len(plan["cells"]) == len(report["cells"])
    assert {key(cell): cell["role"] for cell in report["declared_cells"]} == expected_cells
    assert {key(cell): cell["role"] for cell in report["cells"]} == expected_cells
    assert set(report["inputs"]) == set(experiment.CASES)
    # Re-derive independent oracles from their bound source, not report flags.
    for name in experiment.CASES:
        pdf, oracle = experiment.fixture(name)
        for kind, data, extension in (("inputs", pdf, "pdf"), ("oracles", oracle, "ppm")):
            stored = directory / kind / (name + "." + extension)
            assert stored.read_bytes() == data
        assert hashlib.sha256(pdf).hexdigest() == report["inputs"][name]["pdf_sha256"]
        assert hashlib.sha256(oracle).hexdigest() == report["inputs"][name]["oracle_sha256"]
    rows, blobs, outputs = [], set(), 0
    for cell in report["cells"]:
        pairs = cell["pairs"]
        assert len(pairs) == phase["pairs"] + phase["warmups"]
        assert sorted(p["pair"] for p in pairs) == list(range(-phase["warmups"], phase["pairs"]))
        expected = expected_image((directory / "oracles" / (cell["case"] + ".ppm")).read_bytes(), cell["operation"])
        checked = set()
        for pair in pairs:
            assert pair["order"] in (["baseline", "candidate"], ["candidate", "baseline"])
            assert pair["baseline"]["output_sha256"] == pair["candidate"]["output_sha256"]
            for side in ("baseline", "candidate"):
                record = pair[side]
                assert record["oracle_equal"] and record["process"]["exit_code"] == 0
                assert record["process"]["status"] != "timeout"
                blob = record["blob"]
                assert blob == "blobs/" + record["output_sha256"] + ".rgba"
                if blob not in checked:
                    raw = (directory / blob).read_bytes()
                    assert hashlib.sha256(raw).hexdigest() == record["output_sha256"]
                    assert normalize(raw) == expected
                    header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
                    assert raw.startswith(header)
                    assert raw[len(header) + 3::4] == b"\xff" * (expected[0] * expected[1])
                    checked.add(blob)
                    blobs.add(blob)
                outputs += 1
        measured = [p for p in pairs if p["pair"] >= 0]
        latency, thresholds = cell["latency"], plan["thresholds"]
        prefix = "target" if cell["role"] == "target" else "protected"
        peaks = {side: max(p[side]["process"]["peak_rss_bytes"] for p in measured)
                 for side in ("baseline", "candidate")}
        assert all(type(value) is int and value > 0 for value in peaks.values())
        rss_ratio = peaks["candidate"] / peaks["baseline"]
        assert rss_ratio == cell["peak_rss_ratio"]
        row = {name: cell[name] for name in ("case", "mode", "operation", "role")}
        row.update(latency=latency, peak_rss_ratio=rss_ratio,
                   peak_rss_mib={side: value / 2**20 for side, value in peaks.items()},
                   median_pass=latency["median_upper"] <= thresholds[prefix + "_median_upper"],
                   p95_pass=latency["p95_upper"] <= thresholds[prefix + "_p95_upper"],
                   memory_pass=rss_ratio <= thresholds["peak_rss_ratio_max"])
        for side in ("baseline", "candidate"):
            row[side + "_median_ms"] = statistics.median(p[side]["stages"]["metric_ns"] for p in measured) / 1e6
        rows.append(row)
    summary = {"schema": 1, "run": run, "phase": report["phase"], "admission": False,
               "background_contention_observed": any(event["matches"] for event in guard["events"]),
               "report_sha256": digest(report_path), "independently_verified_outputs": outputs,
               "distinct_output_blobs_rehashed": len(blobs),
               "numeric_gates_pass": all(c["median_pass"] and c["p95_pass"] and c["memory_pass"] for c in rows),
               "target_latency_passes": sum(c["median_pass"] and c["p95_pass"] for c in rows if c["role"] == "target"),
               "protected_latency_passes": sum(c["median_pass"] and c["p95_pass"] for c in rows if c["role"] == "protected"),
               "memory_passes": sum(c["memory_pass"] for c in rows), "cells": rows}
    (local / (run + "-analysis.json")).write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({name: value for name, value in summary.items() if name != "cells"}, indent=2))
    for cell in rows:
        if cell["role"] == "target" or cell["case"].startswith("outline2"):
            print(cell["case"], cell["mode"], cell["operation"], "median ratio", round(cell["latency"]["median_ratio"], 4),
                  "p95 upper", round(cell["latency"]["p95_upper"], 4), "RSS ratio", round(cell["peak_rss_ratio"], 4))


if __name__ == "__main__":
    main()
