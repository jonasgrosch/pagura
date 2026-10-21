"""Release-cell artifact checks, after independent runner receipt validation.

The caller authenticates the containing report first. This module then selects
the corpus, settings and reference pins independently and rereads retained bytes.
It grants neither a controlled-runner claim nor missing release-matrix coverage.
"""
from pathlib import Path

from .artifacts import (DEFAULT_MAXIMUM_ARTIFACT_BYTES, digest,
                        normalizer_sha256, verify_artifact_binding)
from .config import ROOT, sha
from .engines import command_for
from .reference_cases import (current_render_policy_sha256, invocation_settings,
                              requires_reference, validate_case_reference)


def validate_artifact_run(report, policy):
    """Check a completed report store; return its root and byte-accounting state."""
    run = report.get("artifact_run")
    if type(run) is not dict or type(run.get("schema")) is not int or run["schema"] != 1:
        raise ValueError("missing artifact run")
    pointer = report["runner"]["coordination"]
    maximum = policy.get("maximum_artifact_bytes", DEFAULT_MAXIMUM_ARTIFACT_BYTES)
    if (run.get("status") != "complete" or run.get("errors")
            or run.get("run_id") != pointer["run_id"]
            or run.get("receipt_path") != pointer["receipt"]
            or type(run.get("maximum_artifact_bytes")) is not int
            or run["maximum_artifact_bytes"] != maximum
            or type(run.get("stored_bytes")) is not int
            or not 0 < run["stored_bytes"] <= maximum
            or run.get("normalizer_sha256") != normalizer_sha256()):
        raise ValueError("incomplete or changed artifact run")
    root = Path(run["root"])
    if not root.is_absolute() or not root.is_dir():
        raise ValueError("missing artifact store")
    if type(report.get("execution_contexts")) is not dict:
        raise ValueError("missing frozen execution contexts")
    return {"root": root.resolve(), "contexts": set(), "retained": {}}


def verify_cell_artifacts(report, cell, case, policy, state):
    """Validate every measured and warmup invocation for one release cell."""
    if report.get("operation") != "render-e2e" or cell.get("operation") != "render-e2e":
        raise ValueError("artifact release admission supports render-e2e only")
    if sha((ROOT / case["path"]).read_bytes()) != case["sha256"]:
        raise ValueError("source PDF differs from corpus pin")
    if sha((ROOT / case["oracle"]).read_bytes()) != case["oracle_sha256"]:
        raise ValueError("oracle differs from corpus pin")
    page = case.get("page", 0)
    count, warmups = report.get("samples_requested"), report.get("warmups")
    for value in (count, warmups):
        if type(value) is not int or not 0 <= value <= 100_000:
            raise ValueError("invalid requested sample count")
    references, pins = {}, {}
    if requires_reference(case):
        _, pin = validate_case_reference(case, "exact")
        settings = invocation_settings(case["sha256"], page=page, dpi=case["dpi"],
            accuracy="exact", entrypoint="render_page", validation="full",
            render_policy_sha256=current_render_policy_sha256(ROOT))
        references[page] = {"case": case, "mode": "exact", "settings": settings}
        pins[str(page)] = {"sha256": pin, "settings": settings}
    for field, pairs, phase in (("warmup_samples", range(-warmups, 0), "warmup"),
                                ("samples", range(count), "sample")):
        rows = cell.get(field)
        if (type(rows) is not list or len(rows) != len(pairs)
                or any(type(row.get("pair")) is not int for row in rows)
                or sorted(row["pair"] for row in rows) != list(pairs)):
            raise ValueError("missing, duplicated or unexpected retained sample/warmup")
        for row in rows:
            order = row.get("order")
            if order not in (["pagura", cell["engine"]], [cell["engine"], "pagura"]):
                raise ValueError("invalid retained paired order")
            for side, engine in (("pagura", "pagura"), ("baseline", cell["engine"])):
                measurement = row[side]
                binding = measurement["artifact_binding"]
                context_key = binding["context_sha256"]
                context = report["execution_contexts"][context_key]
                identity = report["engine_identities"][engine]
                if context_key in state["contexts"]:
                    raise ValueError("execution context reused by multiple samples")
                expected = {"schema": 1, "run_id": report["artifact_run"]["run_id"],
                    "operation": "render-e2e", "case": case["id"], "source_pdf_sha256": case["sha256"],
                    "pages": [page], "dpi": case["dpi"], "engine": engine,
                    "engine_identity_sha256": digest(identity), "comparison_engine": cell["engine"],
                    "pair": row["pair"], "phase": phase, "order": order,
                    "references": pins if side == "pagura" else {}}
                if any(context.get(key) != value for key, value in expected.items()):
                    raise ValueError("execution context differs from corpus/cell/sample identity")
                request = ({"accuracy": "exact", "entrypoint": "render_page", "validation": "full",
                            "background": "white", "output": "RGB8 PPM", "box": "engine-default",
                            "annotations": True} if side == "pagura" else
                           {"background": "white", "output": "RGB8 over white", "scope": "adapter-request"})
                if context.get("render_settings") != request or binding.get("page") != page:
                    raise ValueError("execution settings/page differ from requested rendering")
                snapshot = state["root"] / f"inputs/{case['sha256']}.pdf"
                paths = context["output_paths"]
                if set(paths) != {str(page)} or not Path(paths[str(page)]).is_absolute():
                    raise ValueError("unexpected invocation output paths")
                command = command_for(engine, identity["command"],
                    {**case, "path": str(snapshot)}, Path(paths[str(page)]))
                if context["command"] != command or context["command_sha256"] != digest(command):
                    raise ValueError("invocation differs from independently constructed command")
                verified = verify_artifact_binding(binding, expected_context=context,
                    claimed_measurement=measurement, artifact_root=state["root"], policy=policy,
                    references=references if side == "pagura" else {})
                actual = verified["outputs"][str(page)]
                if (actual["width"] != case["width"] or actual["height"] != case["height"]
                        or actual["pixels_sha256"] != cell["oracle_pixels_sha256"]
                        or measurement.get("status") != "ok" or measurement.get("exit_code") != 0):
                    raise ValueError("retained output fails exact corpus correctness")
                if references and side == "pagura" and actual["reference"].get("status") != "ok":
                    raise ValueError("independent reference verification did not pass")
                state["contexts"].add(context_key)
                for relative in (binding["receipt"], context["input_snapshot"], actual["blob"]):
                    state["retained"][relative] = (state["root"] / relative).stat().st_size
                if sum(state["retained"].values()) > report["artifact_run"]["maximum_artifact_bytes"]:
                    raise ValueError("retained report exceeds total artifact byte budget")


def finish_artifact_verification(report, state):
    """No hidden contexts or partial byte accounting may be called complete."""
    if state["contexts"] != set(report["execution_contexts"]):
        raise ValueError("unbound execution contexts remain")
    if sum(state["retained"].values()) != report["artifact_run"]["stored_bytes"]:
        raise ValueError("artifact byte accounting differs from retained evidence")
