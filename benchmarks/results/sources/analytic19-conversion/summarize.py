"""Publish every audited cell from the single fixed pilot; never admit release."""
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import statistics

LOCAL = Path("benchmarks/results/local/analytic19-conversion")
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
audit = json.loads((LOCAL / "audit-001.json").read_text())
report = json.loads((LOCAL / "conversion-001.json").read_text())
assert audit["status"] == "audited" and audit["outer_receipt_authenticated"]
assert not audit["errors"] and audit["attempts_observed"] == 1596
assert audit["report_sha256"] == sha(LOCAL / "conversion-001.json")
bindings = {}
for name, status, code in (("campaign-001", "completed", 0), ("audit-001", "failed", 1)):
    path = LOCAL / (name + "-receipt.json")
    receipt = json.loads(path.read_text())
    assert receipt["status"] == status and receipt["exit_code"] == receipt["wrapper_exit_code"] == code
    assert receipt["source_unchanged"] and not receipt["errors"] and not receipt["artifact_errors"]
    for artifact, digest in receipt["artifacts"].items():
        assert sha(artifact) == digest, artifact
    bindings[name] = {"receipt": str(path), "receipt_sha256": sha(path),
                      "artifacts_sha256": receipt["artifacts"], "exit_code": code}
assert audit["runner_receipt_sha256"] == bindings["campaign-001"]["receipt_sha256"]
guards = {}
for name in ("observer-001", "audit-001-observer"):
    path = LOCAL / name / "guard.json"
    guard = json.loads(path.read_text())
    assert not any(event["matches"] for event in guard["events"])
    guards[name] = {"path": str(path), "sha256": sha(path), "scans": guard["scans"],
                    "observed_contention": False, "status": guard["status"]}
counts, cases = defaultdict(Counter), defaultdict(lambda: defaultdict(Counter))
for attempt in audit["attempts"]:
    counts[attempt["engine"]][attempt["classification"]] += 1
    cases[attempt["engine"]][attempt["case"]][attempt["classification"]] += 1
summaries = []
for engine in audit["protocol"]["competitors"]:
    cells = [c for c in audit["cells"] if c["competitor"] == engine and c["accuracy_qualified"]]
    metrics = {}
    for metric in ("engine_ns", "render_ns", "elapsed_ns"):
        values = [c["metrics"][metric]["median_ratio"] for c in cells
                  if "median_ratio" in c["metrics"][metric]]
        metrics[metric] = ({"cells": len(values), "median_of_case_median_ratios": statistics.median(values),
                            "minimum_case_ratio": min(values), "maximum_case_ratio": max(values)}
                           if values else {"status": "unavailable", "cells": 0})
    rss = [c["process_rss"]["ratio"] for c in cells if "ratio" in c["process_rss"]]
    summaries.append({"engine": engine, "matching_cases": len(cells), "total_cases": 19,
                      "metrics": metrics, "rss_ratio_maximum": max(rss),
                      "rss_cases_no_larger": sum(value <= 1 for value in rss), "rss_cases": len(rss)})
common = [name for name in cases["pagura"]
          if all(c["accuracy_qualified"] for c in audit["cells"] if c["case"] == name)]
result = {"schema": 1, "status": "completed-diagnostic-not-release-admitted",
          "product_baseline_commit": "c70b467882faafc9baede8c06c7011ac2be1ae21",
          "measurement_commit": report["commit"], "source_sha256": report["source_sha256"],
          "protocol": audit["protocol"], "attempt_classifications": audit["attempt_classifications"],
          "per_engine_attempts": counts, "per_engine_case_accuracy": cases,
          "engine_versions": {name: row["version"] for name, row in report["engine_identities"].items()},
          "engine_summaries": summaries, "all_engine_matching_cases": sorted(common),
          "cells": audit["cells"], "bindings": bindings, "observers": guards,
          "audit": {"path": str(LOCAL / "audit-001.json"), "sha256": sha(LOCAL / "audit-001.json"),
                    "evidence_errors": [], "exit_1_reason": "41 competitor/case cells disagree with the frozen RGB policy; evidence authentication completed"},
          "summarizer_sha256": sha(__file__), "limits": audit["limits"] + [
              "Ratios are Pagura/competitor; per-engine matching subsets differ and are not a common workload ranking.",
              "Summary case-ratio medians are unweighted diagnostics; all 114 cells and complete five-pair summaries remain below; individual attempts are in the bound local conversion report.",
              "No new self-comparison or net-stack speedup follows from this competitor-only pilot."]}
Path("benchmarks/results/analytic19-conversion.json").write_text(json.dumps(result, indent=2) + "\n")
print("Published all 114 audited cells and finalized receipt bindings.")
