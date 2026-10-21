#!/usr/bin/env python3
"""Fixed six-case coverage-model diagnostic; no performance admission."""
import argparse
from collections import Counter
from decimal import Decimal, localcontext
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import platform
import re
import signal
import sys
import time

CASES = ("single-tiny", "coincident-tiny", "disjoint-tiny", "compound-tiny",
         "full-pixel", "disjoint-halves")
MODES = ("exact", "competitive")
DEADLINE_SECONDS, CHILD_SECONDS = 180, 60
MAX_FILE, MAX_RETAINED, LOG_BUDGET = 4 << 20, 16 << 20, 256 << 10
PREDICTED = {case: 255 if case in CASES[:3] else 63 if case == CASES[-1] else 0
             for case in CASES}
REFERENCE_NAMES = ("declaredcoveragebyte", "unquantizedpermark", "finalsceneboxfilter")


class DeadlineExceeded(Exception):
    pass


def fixture_shapes(case):
    """Each mark is a tuple of horizontal unit-height intervals, in source order."""
    q = F(1, 1024)
    strip = ((F(0), q),)
    if case == "single-tiny":
        return (strip,)
    if case == "coincident-tiny":
        return (strip,) * 1024
    intervals = tuple((F(i, 1024), F(i + 1, 1024)) for i in range(1024))
    if case == "disjoint-tiny":
        return tuple((interval,) for interval in intervals)
    if case == "compound-tiny":
        return (intervals,)
    if case == "full-pixel":
        return (((F(0), F(1)),),)
    if case == "disjoint-halves":
        return (((F(0), F(1, 2)),), ((F(1, 2), F(1)),))
    raise ValueError("unknown fixed fixture")


def union_area(intervals):
    """Exact union length; all fixture rectangles have height one inside the pixel."""
    total, end = F(0), F(0)
    for left, right in sorted(intervals):
        if not F(0) <= left < right <= F(1):
            raise ValueError("fixture interval outside unit pixel")
        total += max(F(0), right - max(left, end))
        end = max(end, right)
    return total


def round_half_up(value):
    return (2 * value.numerator + value.denominator) // (2 * value.denominator)


def references(shapes):
    areas = [union_area(mark) for mark in shapes]

    def compose(weights):
        value = F(255)
        for weight, count in Counter(weights).items():
            value *= (1 - weight) ** count
        return value

    return {
        "declaredcoveragebyte": compose(F(round_half_up(255 * area), 255) for area in areas),
        "unquantizedpermark": compose(areas),
        "finalsceneboxfilter": 255 * (1 - union_area(interval for mark in shapes for interval in mark)),
    }


def rational_record(value, *, reference=False):
    with localcontext() as context:
        context.prec = 32
        display = str(Decimal(value.numerator) / Decimal(value.denominator))
    record = {"numerator": str(value.numerator), "denominator": str(value.denominator),
              "decimal_display_only": display}
    if reference:
        record["integers_within_one_level"] = [v for v in range(256) if abs(F(v) - value) <= 1]
    return record


def number(value):
    """All coordinates are multiples of 1/1024; serialize exact terminating decimals."""
    scaled = value * 10**10
    if scaled.denominator != 1:
        raise ValueError("coordinate has no fixed exact decimal encoding")
    whole, fractional = divmod(scaled.numerator, 10**10)
    return str(whole) if fractional == 0 else f"{whole}.{fractional:010d}".rstrip("0")


def pdf_bytes(shapes):
    content = ["1 g 0 0 1 1 re f", "0 g"]
    for mark in shapes:
        content.extend(f"{number(left)} 0 {number(right-left)} 1 re" for left, right in mark)
        content.append("f")
    body = ("\n".join(content) + "\n").encode("ascii")
    objects = (
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> /Contents 4 0 R >>",
        b"<< /Length " + str(len(body)).encode("ascii") + b" >>\nstream\n" + body + b"endstream",
    )
    result = bytearray(b"%PDF-1.4\n")
    offsets = []
    for index, obj in enumerate(objects, 1):
        offsets.append(len(result))
        result.extend(str(index).encode("ascii") + b" 0 obj\n" + obj + b"\nendobj\n")
    xref = len(result)
    result.extend(b"xref\n0 5\n0000000000 65535 f \n")
    for offset in offsets:
        result.extend(f"{offset:010d} 00000 n \n".encode("ascii"))
    result.extend(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n" +
                  str(xref).encode("ascii") + b"\n%%EOF\n")
    return bytes(result)


def evaluate(rgba, refs, predicted):
    if len(rgba) != 4:
        raise ValueError("expected exactly one RGBA pixel")
    errors = {name: [rational_record(F(channel) - reference) for channel in rgba[:3]]
              for name, reference in refs.items()}
    passed = all(abs(F(channel) - refs["declaredcoveragebyte"]) <= 1 for channel in rgba[:3])
    return {"rgba": list(rgba), "reference_signed_errors": errors,
            "declared_gate_pass": passed and rgba[3] == 255,
            "opaque_alpha_pass": rgba[3] == 255,
            "static_predicted_rgba": [predicted, predicted, predicted, 255],
            "prediction_matches": rgba == bytes([predicted, predicted, predicted, 255])}


def parse_stages(diagnostic, mode):
    raw = diagnostic.encode("ascii", errors="strict")
    if len(raw) >= LOG_BUDGET:
        raise ValueError("combined returned log may be truncated")
    records = [json.loads(line) for line in diagnostic.splitlines() if line.lstrip().startswith("{")]
    if len(records) != 1 or type(records[0]) is not dict:
        raise ValueError("expected exactly one driver JSON record")
    record = records[0]
    fixed = {"schema": 1, "operation": "full", "accuracy": mode, "dpi": 72,
             "width": 1, "height": 1}
    if any(record.get(key) != value or type(record.get(key)) is not type(value)
           for key, value in fixed.items()):
        raise ValueError("driver dimensions/request differ")
    counters = ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")
    if (any(type(record.get(key)) is not int or record[key] <= 0 for key in counters)
            or record["metric_ns"] != record["render_ns"]
            or record["total_ns"] < sum(record[key] for key in counters[:3])):
        raise ValueError("invalid driver timing counters")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--bindings", type=Path, required=True, help="frozen candidate build bindings")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory")
    args = parser.parse_args()
    root, binary, binding_path, directory = (p.resolve() for p in
                                             (args.root, args.binary, args.bindings, args.output))
    directory.mkdir(parents=True, exist_ok=False)
    started, started_ns = time.monotonic(), time.time_ns()
    deadline = started + DEADLINE_SECONDS
    report = {"schema": 1, "diagnostic": "subbyte-coverage-three-references", "status": "running",
              "started_unix_ns": started_ns, "records": [], "performance_admission": False,
              "current_gate": "declaredcoveragebyte: exact rational absolute RGB error <=1; alpha=255",
              "research_only_references": ["unquantizedpermark", "finalsceneboxfilter"],
              "normative_pdf_violation_claim": False, "prior_gates_changed": False,
              "limits": {"child_seconds": CHILD_SECONDS, "total_seconds": DEADLINE_SECONDS,
                         "maximum_retained_bytes": MAX_RETAINED},
              "limitations": ["no retries, warmups, confidence interval or speed claim",
                  "timings retained only to describe each diagnostic child",
                  "declared reference predicts input coverage; it does not observe internal coverage bytes",
                  "a declared-reference mismatch alone does not isolate rasterization versus composition",
                  "returned combined stdout/stderr text; valid logs require lossless ASCII",
                  "local file operations cannot be hard-preempted; owned cleanup must finish",
                  "completed outer receipt must bind final report after this child exits",
                  "local artifact pins are not independently reproduced builds or host attestation"]}
    retained, previous = 0, {}
    records_files = {}

    def checkpoint():
        report["retained_files"] = records_files
        report["retained_bytes"] = retained
        temporary = directory / "report.pending"
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        temporary.replace(directory / "report.json")

    def check_deadline():
        if time.monotonic() >= deadline:
            raise DeadlineExceeded("fixed diagnostic deadline reached")

    def sha(raw):
        return hashlib.sha256(raw).hexdigest()

    def retain(name, raw):
        nonlocal retained
        if len(raw) > MAX_FILE or retained + len(raw) > MAX_RETAINED:
            raise ValueError("fixed evidence size budget exceeded")
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(raw)
        path.chmod(0o400)
        retained += len(raw)
        records_files[name] = {"sha256": sha(raw), "bytes": len(raw)}

    try:
        sys.dont_write_bytecode = True
        sys.path.insert(0, str(root / "tools"))
        from benchkit.artifacts import hash_regular_file, read_bounded_regular
        from benchkit.config import ROOT
        from benchkit.measurement import MeasurementCancelled, run_process
        from benchkit.runner_evidence import coordination_pointer
        if Path(ROOT).resolve() != root:
            raise ValueError("imported helper checkout differs")

        def cancel(signum, _frame):
            raise MeasurementCancelled(signum)

        previous = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
        for signum in previous:
            signal.signal(signum, cancel)
        pointer = coordination_pointer()
        if pointer is None:
            raise ValueError("existing coordinator runner is required")
        receipt_raw = read_bounded_regular(Path(pointer["receipt"]), MAX_FILE)
        receipt = json.loads(receipt_raw)
        if (type(receipt) is not dict or type(receipt.get("schema")) is not int or receipt["schema"] != 1
                or receipt.get("run_id") != pointer["run_id"] or receipt.get("status") != "running"
                or receipt.get("kind") != "benchmark" or receipt.get("source_root") != str(root)
                or receipt.get("cwd") != str(root) or receipt.get("errors") != []):
            raise ValueError("runner receipt does not identify this active coordinated job")
        source = receipt.get("source_before")
        if (type(source) is not dict or type(source.get("commit")) is not str
                or re.fullmatch(r"[0-9a-f]{40,64}", source["commit"]) is None
                or type(source.get("sha256")) is not str
                or re.fullmatch(r"[0-9a-f]{64}", source["sha256"]) is None):
            raise ValueError("missing runner source binding")
        report.update(runner={"controlled": False, "coordination": pointer},
                      commit=source["commit"], source_sha256=source["sha256"])
        retain("runner-start.json", receipt_raw)
        binding_raw = read_bounded_regular(binding_path, MAX_FILE)
        binding = json.loads(binding_raw)
        identity = binding["identities"]["candidate"]
        binary_sha = hash_regular_file(binary)
        if (Path(identity["binary"]).resolve() != binary or identity["binary_sha256"] != binary_sha
                or Path(identity["root"]).resolve() != root):
            raise ValueError("candidate binary/root differ from frozen build bindings")
        retain("build-bindings.json", binding_raw)
        report["build_identity"] = identity
        refs_by_case, inputs = {}, {}
        for case in CASES:
            check_deadline()
            shapes = fixture_shapes(case)
            refs_by_case[case] = references(shapes)
            raw = pdf_bytes(shapes)
            retain("inputs/" + case + ".pdf", raw)
            inputs[case] = {"pdf_sha256": sha(raw), "black_marks": len(shapes),
                            "black_subpaths": sum(map(len, shapes)), "white_background_marks": 1,
                            "references": {name: rational_record(value, reference=True)
                                           for name, value in refs_by_case[case].items()},
                            "static_prediction_both_modes": PREDICTED[case]}
        paths = {Path(__file__).resolve(), Path(sys.executable).resolve(), binding_path,
                 root / "crates/pagura-render/examples/scene_index_profile.rs", Path("/bin/ps")}
        for module in tuple(sys.modules.values()):
            path = getattr(module, "__file__", None)
            if path and Path(path).is_file():
                paths.add(Path(path).resolve())
        manifest = {str(path): hash_regular_file(path) for path in sorted(paths)}
        plan = {"schema": 1, "inputs": inputs, "schedule": [(case, mode) for case in CASES for mode in MODES],
                "binary": str(binary), "binary_sha256": binary_sha,
                "build_bindings_sha256": sha(binding_raw), "bound_files_sha256": manifest,
                "runtime": {"python": sys.version, "executable": str(Path(sys.executable).resolve()),
                            "platform": platform.platform(), "machine": platform.machine()},
                "dpi": 72, "operation": "full", "processes": 12,
                "references": {"declaredcoveragebyte": "current gate: per-mark coverage rounded half-up to byte",
                    "unquantizedpermark": "research only: exact scalar area per mark, source-over before one output rounding",
                    "finalsceneboxfilter": "research only: exact union of opaque black geometry, then one box average"}}
        retain("plan.json", (json.dumps(plan, indent=2, allow_nan=False) + "\n").encode())
        report["plan_sha256"] = records_files["plan.json"]["sha256"]
        checkpoint()
        for sequence, (case, mode) in enumerate(plan["schedule"]):
            check_deadline()
            pdf = directory / "inputs" / (case + ".pdf")
            if hash_regular_file(binary) != binary_sha or hash_regular_file(pdf) != inputs[case]["pdf_sha256"]:
                raise ValueError("frozen binary/input changed before launch")
            output_name = f"outputs/{sequence:02d}-{case}-{mode}.rgba"
            output = directory / output_name
            output.parent.mkdir(parents=True, exist_ok=True)
            if output.exists():
                raise ValueError("unexpected existing renderer output")
            command = [str(binary), str(pdf), mode, "full", str(output), "72"]
            item = {"case": case, "mode": mode, "command": command, "verified": False}
            report["records"].append(item)
            checkpoint()
            timeout = min(CHILD_SECONDS, deadline - time.monotonic())
            if timeout <= 0:
                raise DeadlineExceeded("deadline before child launch")
            process, diagnostic = run_process(command, {"measure_rss": False, "timeout_seconds": timeout},
                                              log_budget=LOG_BUDGET)
            item["process"] = process
            log_name = f"logs/{sequence:02d}-{case}-{mode}.stdout-stderr.txt"
            retain(log_name, diagnostic.encode("utf-8"))
            item["log"] = log_name
            if output.exists():
                raw = read_bounded_regular(output, MAX_FILE)
                retained += len(raw)
                if retained > MAX_RETAINED:
                    raise ValueError("fixed evidence size budget exceeded")
                output.chmod(0o400)
                records_files[output_name] = {"sha256": sha(raw), "bytes": len(raw)}
                item["output"] = output_name
            else:
                raw = None
            check_deadline()
            if process.get("exit_code") != 0 or process.get("status") == "timeout":
                raise ValueError("renderer process failed; no retry")
            item["stages"] = parse_stages(diagnostic, mode)
            header = b"RGBA8 1 1 premultiplied\n"
            if raw is None or not raw.startswith(header) or len(raw) != len(header) + 4:
                raise ValueError("renderer did not return exactly one raw RGBA pixel")
            item.update(evaluate(raw[len(header):], refs_by_case[case], PREDICTED[case]), verified=True)
            checkpoint()
            check_deadline()
        if hash_regular_file(binary) != binary_sha:
            raise ValueError("candidate binary changed during diagnostic")
        changed = [path for path, pin in manifest.items() if hash_regular_file(Path(path)) != pin]
        if changed:
            raise ValueError("bound code/runtime changed: " + ", ".join(changed))
        for name, record in records_files.items():
            if hash_regular_file(directory / name) != record["sha256"]:
                raise ValueError("retained evidence changed: " + name)
        if len(report["records"]) != 12 or not all(item["verified"] for item in report["records"]):
            raise ValueError("fixed twelve-output diagnostic is incomplete")
        check_deadline()
        report["unexpected_predictions"] = [{"case": item["case"], "mode": item["mode"], "rgba": item["rgba"]}
                                            for item in report["records"] if not item["prediction_matches"]]
        report["declared_gate_pass"] = all(item["declared_gate_pass"] for item in report["records"])
        report["verified_outputs"] = 12
        report["status"] = "completed-diagnostic" if report["declared_gate_pass"] else "completed-declared-gate-failed"
        exit_code = 0 if report["declared_gate_pass"] else 1
    except BaseException as error:
        report["status"] = ("deadline-exceeded" if isinstance(error, DeadlineExceeded) else
                            "cancelled" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "failed")
        report["error"] = f"{type(error).__name__}: {error}"
        exit_code = (128 + getattr(error, "signum", signal.SIGINT) if report["status"] == "cancelled"
                     else 124 if report["status"] == "deadline-exceeded" else 1)
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        report["finished_unix_ns"] = time.time_ns()
        report["duration_seconds"] = time.monotonic() - started
        checkpoint()
        (directory / "report.json").chmod(0o400)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
