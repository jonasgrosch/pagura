"""Release matrix gate. Exploratory evidence cannot override missing scope."""
from .config import ROOT, POLICY_PATH, MATRIX_PATH, read_json, source_digest, sha
from .corpus import manifest_digest, render_cases
from .images import ppm
from .statistics import evaluate_cell
from .runner_evidence import verify_runner_receipt
from .artifact_gate import (validate_artifact_run, verify_cell_artifacts,
                            finish_artifact_verification)
from .artifacts import verify_identity


def gate(report, *, report_path=None, receipt_path=None, receipt_sha256=None):
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
    current_source = source_digest()
    if report.get("source_sha256") != current_source:
        reasons.append("source differs from measured source")
    artifact_state = None
    try:
        verify_runner_receipt(report, report_path=report_path, receipt_path=receipt_path,
                              receipt_sha256=receipt_sha256, source_sha256=current_source)
        artifact_state = validate_artifact_run(report, policy)
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        reasons.append(f"runner/artifact evidence invalid: {error}")
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
        elif artifact_state is not None:
            try:
                verify_identity(identity)
            except (OSError, ValueError, KeyError, TypeError, AttributeError):
                reasons.append(f"missing/changed engine artifact or command: {engine}")
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
        if artifact_state is not None and case:
            try:
                verify_cell_artifacts(report, cell, case, policy, artifact_state)
            except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
                verdict["pass"] = False
                verdict["reasons"].append(f"retained artifact evidence invalid: {error}")
        else:
            verdict["pass"] = False
            verdict["reasons"].append("independently bound artifact evidence missing")
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
    if artifact_state is not None:
        try:
            finish_artifact_verification(report, artifact_state)
        except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
            reasons.append(f"artifact coverage invalid: {error}")
    required = {(op["id"], case, engine) for op in matrix["operations"] for engine in op["engines"]
                for workload in matrix["workloads"] for case in workload["cases"]}
    missing = required - seen
    if missing:
        reasons.append(f"missing {len(missing)} declared comparison cells")
    if not required or not report.get("cells"):
        reasons.append("empty matrix/evidence cannot pass")
    return {"pass": not reasons, "reasons": sorted(set(reasons)), "comparisons": cells}

