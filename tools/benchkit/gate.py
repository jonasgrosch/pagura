"""Release matrix gate. Exploratory evidence cannot override missing scope."""
from pathlib import Path
from .config import ROOT, POLICY_PATH, MATRIX_PATH, read_json, source_digest, sha
from .corpus import manifest_digest, render_cases
from .images import ppm
from .statistics import evaluate_cell


def gate(report):
    """Fail closed on missing scope. Per-cell success cannot override release coverage."""
    policy, matrix = read_json(POLICY_PATH), read_json(MATRIX_PATH)
    reasons = []
    if report.get("operation") in ("conversion", "regression"):
        reasons.append("conversion/regression evidence is exploratory and ineligible for release")
    if report.get("schema") != 1 or report.get("suite") != "release":
        reasons.append("exploratory smoke evidence is not a release suite")
    for field, digest in [("policy_sha256", sha(POLICY_PATH.read_bytes())), ("matrix_sha256", sha(MATRIX_PATH.read_bytes())),
                          ("manifest_sha256", manifest_digest())]:
        if report.get(field) != digest:
            reasons.append(f"missing/changed {field}")
    if report.get("source_sha256") != source_digest():
        reasons.append("source differs from measured source")
    if report.get("settings") != policy["settings"]:
        reasons.append("incomparable rendering settings")
    if report.get("warmups", 0) < policy["minimum_warmups"]:
        reasons.append("insufficient warmups")
    if not report.get("runner", {}).get("controlled"):
        reasons.append("controlled runner evidence missing")
    # Aggregation across independently attested platform/scale/concurrency runs is future work.
    reasons.append("cross-platform, resolution and concurrency aggregation is not implemented")
    for operation in matrix["operations"]:
        if not operation["implemented"]:
            reasons.append(f"operation not benchmarked: {operation['id']}")
    for workload in matrix["workloads"]:
        if not workload["cases"]:
            reasons.append(f"unbound release workload: {workload['id']}")
    identities = report.get("engine_identities", {})
    for engine in ["pagura"] + policy["required_engines"]:
        identity = identities.get(engine, {})
        if identity.get("status") != "available" or not identity.get("version") or not identity.get("artifacts"):
            reasons.append(f"missing engine identity: {engine}")
        else:
            for path, digest in identity["artifacts"].items():
                try:
                    if sha(Path(path).read_bytes()) != digest:
                        reasons.append(f"stale engine artifact: {engine}")
                except OSError:
                    reasons.append(f"missing engine artifact: {engine}")
    cells, seen = [], set()
    manifest = {c["id"]: c for c in render_cases()}
    for cell in report.get("cells", []):
        key = (cell.get("operation"), cell.get("case"), cell.get("engine"))
        if key in seen:
            reasons.append("duplicate comparison cell")
        seen.add(key)
        case = manifest.get(cell.get("case"))
        if not case or cell.get("pdf_sha256") != case["sha256"] or cell.get("oracle_pixels_sha256") != sha(ppm((ROOT / case["oracle"]).read_bytes())[2]):
            reasons.append("unknown/changed case or oracle")
        verdict = evaluate_cell(cell, policy)
        if case and any(s.get(k, {}).get("width") != case["width"] or s.get(k, {}).get("height") != case["height"]
                        for s in cell.get("samples", []) for k in ["pagura", "baseline"]):
            verdict["pass"] = False
            verdict["reasons"].append("wrong output dimensions")
        if cell.get("warmup_failures"):
            verdict["pass"] = False
            verdict["reasons"].append("warmup correctness failure")
        cells.append({"case": key[1], "engine": key[2], **verdict})
        if not verdict["pass"]:
            reasons.append(f"comparison did not pass: {key[1]} / {key[2]}")
    required = {(op["id"], case, engine) for op in matrix["operations"] for engine in op["engines"]
                for workload in matrix["workloads"] for case in workload["cases"]}
    missing = required - seen
    if missing:
        reasons.append(f"missing {len(missing)} declared comparison cells")
    if not required or not report.get("cells"):
        reasons.append("empty matrix/evidence cannot pass")
    return {"pass": not reasons, "reasons": sorted(set(reasons)), "comparisons": cells}

