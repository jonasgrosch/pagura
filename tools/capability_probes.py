"""G01 fixture-scoped competitor probes, separate from performance measurements.

`check` is offline/source-only. `run` invokes explicitly installed competitors;
use hosted CI while ADR 0031 reserves the local machine for performance lanes.
Never installs dependencies or promotes a source-inventory family to supported.
"""
import argparse
import hashlib
import json
import math
import os
import platform
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from capabilities import ROOT, LEDGER, read_json, require
from competitors import CACHE
from benchkit.engines import engines, identify

MANIFEST = Path("benchmarks/probes/document-services.json")
ADAPTER = Path("benchmarks/adapters/capability_python.py")
SELECTED = ("pdfium", "mupdf")
ADAPTER_FILES = [ADAPTER, Path("benchmarks/adapters/capability_pdfjs.mjs"),
                 Path("benchmarks/adapters/capability_pdfjs_services.mjs"),
                 Path("benchmarks/adapters/capability_poppler.py"), Path("benchmarks/adapters/capability_quartz.swift"),
                 Path("benchmarks/adapters/hayro/src/bin/capabilities.rs")]
# Explicit adapter coverage, not a claim of backend absence. These cells stay
# visible and cannot be used to close a capability or release gate.
UNIMPLEMENTED = {
    ("hayro", "attachments"): "no attachment extractor implemented over hayro's public object API",
    ("hayro", "text"): "no text reconstruction implemented over hayro's public interpreter API",
    ("quartz", "text"): "no text reconstruction implemented over CoreGraphics PDF operators",
}
OPERATIONS = {"pages": "document.structure", "metadata": "document.metadata",
              "attachments": "document.attachments", "text": "text.accessibility"}
OUTCOMES = {"pass", "incorrect", "rejected", "unsupported", "timeout", "adapter-error",
            "adapter-unavailable", "engine-unavailable", "not-run"}
PAYLOAD = b"Synthetic attachment.\n"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fixture(variant: str) -> bytes:
    """Hand-authored PDF objects, independent of Pagura and all probed engines.

    ISO 32000-2: 7.5 (xref), 7.7.4 (EmbeddedFiles), 7.11.3 (file spec),
    9.4 (text objects), 14.3.3 (Info). No fonts or private-corpus assets needed.
    """
    if variant == "invalid":
        return b"This is intentionally not a PDF.\n"
    if variant not in ("services", "complete-ef", "empty"):
        raise ValueError("unknown synthetic variant")
    full = variant != "empty"
    content = b"BT /F1 12 Tf 20 50 Td (Parity probe) Tj ET\n" if full else b""
    def stream(value: bytes, extra: bytes = b"") -> bytes:
        return b"<< /Length " + str(len(value)).encode() + extra + b" >>\nstream\n" + value + b"\nendstream"
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R" + (b" /Names << /EmbeddedFiles << /Names [(probe.txt) 7 0 R] >> >>" if full else b"") + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        stream(content), b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    if full:
        objects += [b"<< /Title (Capability probe) /Author (Pagura fixtures) >>",
                    b"<< /Type /Filespec /F (probe.txt) /UF (probe.txt) /EF << /F 8 0 R" +
                    (b" /UF 8 0 R" if variant == "complete-ef" else b"") + b" >> >>",
                    stream(PAYLOAD, b" /Type /EmbeddedFile /Subtype /text#2Fplain")]
    output = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for index, body in enumerate(objects, 1):
        offsets.append(len(output))
        output.extend(f"{index} 0 obj\n".encode() + body + b"\nendobj\n")
    xref = len(output)
    output.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        output.extend(f"{offset:010} 00000 n \n".encode())
    output.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R".encode())
    if full:
        output.extend(b" /Info 6 0 R")
    output.extend(f" >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return bytes(output)


def validate_manifest(data, inventory):
    require(set(data) == {"schema", "scope", "cases"} and type(data["schema"]) is int and data["schema"] == 1, "invalid probe manifest")
    require(isinstance(data["scope"], str) and data["scope"].strip(), "missing probe scope")
    identities = {row["id"] for row in inventory["capabilities"]}
    cases = data["cases"]
    require(isinstance(cases, list) and cases, "empty probe manifest")
    seen = set()
    for row in cases:
        require(set(row) == {"id", "capability", "operation", "fixture", "oracle", "expected"}, "invalid probe case")
        require(isinstance(row["id"], str) and row["id"] not in seen, "duplicate probe case")
        seen.add(row["id"])
        require(row["capability"] in identities and OPERATIONS.get(row["operation"]) == row["capability"], "unassigned probe operation")
        require(row["fixture"] in {"services", "complete-ef", "empty", "invalid"}, "unknown probe fixture")
        require(row["oracle"] in {"exact-json", "reject-invalid"}, "unknown probe oracle")
        require((row["oracle"] == "reject-invalid") == (row["fixture"] == "invalid"), "invalid rejection control")
        require(row["oracle"] != "reject-invalid" or row["expected"] is None, "rejection control must have no value")
    require(any(row["oracle"] == "reject-invalid" for row in cases), "missing invalid-input control")
    for operation in OPERATIONS:
        variants = {row["fixture"] for row in cases if row["operation"] == operation}
        require({"services", "empty"} <= variants, f"missing populated/empty controls for {operation}")


def same_value(left, right):
    """JSON semantics, without Python's True == 1 or nonfinite numbers."""
    if isinstance(left, (int, float)) and type(left) is not bool:
        return (type(right) in (int, float)
                and (type(left) is int or math.isfinite(left))
                and (type(right) is int or math.isfinite(right)) and left == right)
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(same_value(value, right[key]) for key, value in left.items())
    if isinstance(left, list):
        return len(left) == len(right) and all(same_value(a, b) for a, b in zip(left, right))
    return left == right


def adjudicate(case, response):
    """A returned value is compared with an authored expectation, never a peer."""
    if not isinstance(response, dict) or type(response.get("schema")) is not int or response["schema"] != 1:
        return {"outcome": "adapter-error", "reason": "invalid adapter envelope"}
    status = response.get("status")
    if status == "ok":
        if set(response) != {"schema", "status", "value"}:
            return {"outcome": "adapter-error", "reason": "invalid value envelope"}
        passed = case["oracle"] == "exact-json" and same_value(response["value"], case["expected"])
        return {"outcome": "pass" if passed else "incorrect", "observed": response["value"]}
    if status in {"rejected", "adapter-error", "adapter-unavailable", "engine-unavailable"}:
        if set(response) != {"schema", "status", "reason"} or not isinstance(response["reason"], str):
            return {"outcome": "adapter-error", "reason": "invalid error envelope"}
        passed = status == "rejected" and case["oracle"] == "reject-invalid"
        return {"outcome": "pass" if passed else status, "reason": response["reason"]}
    if status == "unsupported" and set(response) == {"schema", "status", "feature", "reason"}:
        if all(isinstance(response[key], str) and response[key].strip() for key in ("feature", "reason")):
            return {"outcome": "unsupported", "feature": response["feature"], "reason": response["reason"]}
    return {"outcome": "adapter-error", "reason": "unknown adapter outcome"}


def invoke(command, case, directory):
    """Keep native output on disk and bound both duration and parsed output."""
    with (directory / "stdout").open("wb") as out, (directory / "stderr").open("wb") as err:
        try:
            result = subprocess.run(command, stdout=out, stderr=err, timeout=20, cwd=ROOT)
        except subprocess.TimeoutExpired:
            return {"outcome": "timeout", "reason": "20 second probe deadline"}
        except OSError:
            return {"outcome": "engine-unavailable", "reason": "adapter interpreter could not start"}
    output, errors = directory / "stdout", directory / "stderr"
    diagnostic = {"stdout_bytes": output.stat().st_size, "stderr_bytes": errors.stat().st_size}
    if max(diagnostic.values()) > 65536:
        return {"outcome": "adapter-error", "reason": "adapter output exceeds 64 KiB", **diagnostic}
    diagnostic.update(stdout_sha256=sha(output.read_bytes()), stderr_sha256=sha(errors.read_bytes()))
    if result.returncode:
        return {"outcome": "adapter-error", "reason": "nonzero adapter exit", "exit_code": result.returncode, **diagnostic}
    try:
        response = json.loads(output.read_bytes())
    except (ValueError, UnicodeError):
        return {"outcome": "adapter-error", "reason": "adapter did not return JSON", **diagnostic}
    return {**adjudicate(case, response), **diagnostic}


def public_identity(identity):
    """Keep reproducible identities without machine-specific absolute paths/logs."""
    if identity["status"] != "available":
        return {"status": "unavailable", "reason": "engine identity could not be established"}
    # The shared benchmark helper concatenates version stdout and stderr. Some
    # bindings emit host/device warnings; never publish those as version data.
    version_text = identity["version"].strip()
    try:
        version, end = json.JSONDecoder().raw_decode(version_text)
        require(isinstance(version, dict) and "engine" in version and
                set(version) <= {"wrapper", "engine", "node", "canvas", "system", "python"}, "invalid version fields")
        require(all(isinstance(value, str) and value.strip() for value in version.values()), "invalid version strings")
    except (ValueError, TypeError):
        return {"status": "unavailable", "reason": "adapter did not supply structured version identity"}
    def locator(value):
        return value.replace(str(ROOT), "$REPO").replace(str(CACHE), "$COMPETITORS")
    result = {"status": "available", "version": version,
            "version_diagnostics_sha256": sha(version_text[end:].encode()),
            "command": [locator(value) for value in identity["command"]],
            "artifacts": {locator(path): digest for path, digest in identity["artifacts"].items()}}
    if "build" in identity:
        result["build"] = identity["build"]
    return result


def service_commands():
    render = engines()
    return {"pdfium": [render["pdfium"][0], str(ROOT / ADAPTER), "pdfium"],
            "mupdf": [render["mupdf"][0], str(ROOT / ADAPTER), "mupdf"],
            "pdfjs": [render["pdfjs"][0], str(ROOT / "benchmarks/adapters/capability_pdfjs.mjs")],
            "poppler": ["/usr/bin/python3", str(ROOT / "benchmarks/adapters/capability_poppler.py")],
            "hayro": [str(CACHE / "hayro-target/release/capabilities")],
            "quartz": [str(CACHE / "capability-quartz")]}


def service_identity(name, command):
    identity = identify(name, command)
    if name == "poppler" and identity["status"] == "available":
        # The OS package is not lock-pinned. Attest the actual GLib/core libraries
        # and GI extension as well as their reported versions, on hosted Ubuntu.
        native = {path.resolve() for pattern in ("*-linux-gnu/libpoppler*.so*", "python3/dist-packages/gi/_gi*.so")
                  for path in Path("/usr/lib").glob(pattern) if path.is_file()}
        if not native:
            return {"status": "unavailable", "reason": "Poppler native artifacts were not found"}
        identity["artifacts"].update({str(path): sha(path.read_bytes()) for path in sorted(native)})
    result = public_identity(identity)
    result["configuration"] = {
        "pdfium": "Pinned default pypdfium2 wheel; optional V8/XFA builds are not probed.",
        "mupdf": "Pinned default PyMuPDF wheel; optional providers are not probed.",
        "pdfjs": "Pinned pdfjs-dist Node legacy build; eval disabled; browser behavior is not probed.",
        "poppler": "Hosted Ubuntu GLib/GI packages; actual version and native hashes recorded, not lock-pinned.",
        "hayro": "Pinned standalone hayro default features; public page/ASCII Info APIs only.",
        "quartz": "Hosted macOS CoreGraphics; OS version and adapter binary hash recorded; no PDFKit.",
    }[name]
    return result


def observation_complete(engine, case, result):
    """CI checks the measuring tool, not whether every competitor is correct.

    Wrong values and explicit refusals are usable observations, still reported
    as failures to meet the independent oracle. An adapter/build failure or a
    timeout cannot establish such an observation and fails the workflow.
    """
    if (engine, case["operation"]) in UNIMPLEMENTED:
        return result["outcome"] == "adapter-unavailable"
    return result["outcome"] in {"pass", "incorrect", "rejected", "unsupported"}


def run(data, inventory, output, selected=SELECTED):
    require(selected and len(set(selected)) == len(selected) and set(selected) <= set(inventory["engines"]), "invalid selected engines")
    commands = service_commands()
    identities = {name: service_identity(name, commands[name]) if name in selected else
                  {"status": "not-run", "reason": "engine is assigned to another hosted platform/job"}
                  for name in inventory["engines"]}
    source_files = [MANIFEST, *ADAPTER_FILES, Path("tools/capability_probes.py"), Path("tools/benchkit/engines.py"),
                    Path("tools/benchkit/config.py"), Path("tools/competitors.py"), LEDGER, Path("uv.lock"),
                    Path("pyproject.toml"), Path(".python-version"), Path(".github/workflows/capabilities.yml"),
                    Path("benchmarks/node-package-lock.json"), Path("benchmarks/adapters/hayro/Cargo.lock"),
                    Path("benchmarks/adapters/hayro/Cargo.toml"), Path("rust-toolchain.toml"), Path(".cargo/config.toml")]
    report = {"schema": 1, "scope": data["scope"], "generated_at": datetime.now(timezone.utc).isoformat(),
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "workflow_run": os.environ.get("GITHUB_RUN_ID"), "selected_engines": list(selected),
              "command": ["python", "tools/capability_probes.py", "run", "--engines", ",".join(selected), "--output", "$OUTPUT"],
              "environment": {"system": platform.system(), "release": platform.release(), "machine": platform.machine(), "python": platform.python_version()},
              "source_sha256": {path.as_posix(): sha((ROOT / path).read_bytes()) for path in source_files},
              "engines": identities, "cases": data["cases"], "fixtures": {}, "results": []}
    with tempfile.TemporaryDirectory(prefix="pagura-capability-probes-") as temporary:
        directory = Path(temporary)
        for variant in sorted({case["fixture"] for case in data["cases"]}):
            content = fixture(variant)
            (directory / f"{variant}.pdf").write_bytes(content)
            report["fixtures"][variant] = {"sha256": sha(content), "bytes": len(content)}
        for name in inventory["engines"]:
            for case in data["cases"]:
                identity = identities[name]
                if name not in selected:
                    result = {"outcome": "not-run", "reason": identity["reason"]}
                elif identity["status"] != "available":
                    result = {"outcome": "engine-unavailable", "reason": identity["reason"]}
                elif (name, case["operation"]) in UNIMPLEMENTED:
                    result = {"outcome": "adapter-unavailable", "reason": UNIMPLEMENTED[name, case["operation"]]}
                else:
                    command = commands[name] + [case["operation"], str(directory / f"{case['fixture']}.pdf")]
                    result = invoke(command, case, directory)
                report["results"].append({"engine": name, "case": case["id"], **result})
    validate_report(report, data, inventory)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    cases = {case["id"]: case for case in data["cases"]}
    return all(observation_complete(row["engine"], cases[row["case"]], row)
               for row in report["results"] if row["engine"] in selected)


def validate_report(report, manifest, inventory):
    """No dropped cells, unattested passes or silently excluded competitors."""
    require(report.get("schema") == 1 and report.get("cases") == manifest["cases"], "probe report/manifest mismatch")
    require(set(report["engines"]) == set(inventory["engines"]), "probe report lost a baseline")
    selected = report.get("selected_engines", [])
    require(selected and len(selected) == len(set(selected)) and set(selected) <= set(inventory["engines"]), "invalid selected engines")
    expected = {(engine, case["id"]) for engine in inventory["engines"] for case in manifest["cases"]}
    seen = set()
    for row in report["results"]:
        key = row["engine"], row["case"]
        require(key in expected and key not in seen, "duplicate or unknown probe cell")
        seen.add(key)
        require(row["outcome"] in OUTCOMES, "unknown probe outcome")
        require((row["outcome"] == "not-run") == (row["engine"] not in selected), "selected engine/result mismatch")
        if row["outcome"] == "pass":
            require(report["engines"][row["engine"]]["status"] == "available", "pass without engine identity")
            case = next(case for case in manifest["cases"] if case["id"] == row["case"])
            require(case["oracle"] == "reject-invalid" or same_value(row.get("observed"), case["expected"]), "pass without matching oracle")
            if case["oracle"] == "reject-invalid":
                require(isinstance(row.get("reason"), str) and row["reason"].strip() and "observed" not in row, "pass without rejection evidence")
    require(seen == expected, "missing probe cells")


def combine(reports, manifest, inventory):
    """Join disjoint platform jobs only when their source and fixture identities match."""
    require(reports, "no probe reports supplied")
    first = reports[0]
    selected = set()
    identities, results, runs = {}, [], []
    for report in reports:
        validate_report(report, manifest, inventory)
        for field in ("revision", "scope", "source_sha256", "fixtures", "workflow_run"):
            require(report.get(field) and report.get(field) == first.get(field), f"probe reports disagree on {field}")
        names = set(report["selected_engines"])
        require(not (selected & names), "overlapping engine reports")
        selected.update(names)
        identities.update({name: report["engines"][name] for name in names})
        results.extend(row for row in report["results"] if row["engine"] in names)
        runs.append({key: report[key] for key in ("environment", "generated_at", "selected_engines", "command")})
    require(selected == set(inventory["engines"]), "combined report is missing baseline engines")
    combined = {key: first[key] for key in ("schema", "scope", "revision", "source_sha256", "fixtures", "workflow_run", "cases")}
    combined.update(generated_at=datetime.now(timezone.utc).isoformat(), runs=runs,
                    selected_engines=list(inventory["engines"]), engines=identities, results=results)
    validate_report(combined, manifest, inventory)
    return combined


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check", help="validate only the authored manifest; no engine calls")
    execute = commands.add_parser("run", help="run installed external engines on synthetic inputs")
    execute.add_argument("--output", type=Path, required=True)
    execute.add_argument("--engines", default=",".join(SELECTED), help="comma-separated installed engines; all columns remain in the report")
    merge = commands.add_parser("combine", help="join hosted platform reports; no engines invoked")
    merge.add_argument("reports", type=Path, nargs="+")
    merge.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    data, inventory = read_json(ROOT / MANIFEST), read_json(ROOT / LEDGER)
    validate_manifest(data, inventory)
    if args.command == "run":
        if not run(data, inventory, args.output, tuple(args.engines.split(","))):
            parser.exit(1, "G01 could not establish all implemented observations; inspect the saved report.\n")
        print("G01 observations recorded. Incorrect/refused results and explicit adapter gaps remain in the report; CI success is not engine correctness or parity.")
    elif args.command == "combine":
        report = combine([read_json(path) for path in args.reports], data, inventory)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print("G01 combined all six baseline columns; per-cell outcomes retain adapter gaps and failures.")
    else:
        print(f"G01 manifest valid: {len(data['cases'])} cases, {len(inventory['engines'])} baseline columns; no engines invoked.")


if __name__ == "__main__":
    main()
