#!/usr/bin/env python3
"""Fixed native N16 route diagnostic; no timing or release admission claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import stat
import sys

sys.dont_write_bytecode = True
MARKER = "PAGURA_N16_REACH_V1 "
MAX_BYTES, LOG_BYTES = 64 * 2**20, 16 * 2**20
# request, fixture, dimension, storage, chunk side, N16 interval/pixels/calls
REQUESTS = (
    ("opaque64", "opaque64", 2048, "N16", 256, (59904, 61440, 1536, 6)),
    ("opaque-empty", "opaque-empty", 512, "N16", 256, (65536, 0, 0, 0)),
    ("opaque-distant", "opaque-distant", 512, "N16", 256, (0, 65536, 65536, 256)),
    ("opaque-fill", "opaque-fill", 512, "N16", 256, (0, 65536, 65536, 256)),
    ("opaque-tiny", "opaque-tiny", 64, "N16", 256, (2072, 2528, 456, 8)),
    ("sparse64", "sparse64", 2048, "F32", 256, None),
    ("default-knockout64", "default-knockout64", 2048, "F64", 256, None),
    ("image-only", "image-only", 512, "F32", 128, None),
    ("nonopaque-empty", "opaque-empty", 512, "F64", 256, None),
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def bootstrap_read(path, limit):
    """Read preparation before importing the pinned checkout's helpers."""
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(fd, "rb") as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "not a regular preparation file")
        data = stream.read(limit + 1)
    require(len(data) <= limit, "preparation exceeds bound")
    return data


def integer(value):
    require(isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]*", value), "invalid trace integer")
    return int(value)


def fields(line, kind, names):
    parts = line.split(" ")
    require(parts[0] == kind and len(parts) == len(names) + 1, "trace record field count/kind")
    result = {}
    for part in parts[1:]:
        key, separator, value = part.partition("=")
        require(separator and value and key not in result, "malformed/duplicate trace field")
        result[key] = value
    require(set(result) == set(names), "unexpected trace fields")
    return result


def trace(log, request, mode, background):
    name, _, dimension, storage, side, fixed_interval = request
    lines = []
    for line in log.splitlines():
        if MARKER in line:
            require(line.startswith(MARKER) and line.count(MARKER) == 1, "corrupt trace marker")
            lines.append(line[len(MARKER):])
    require(lines, "missing reach trace")
    plan = fields(lines[0], "plan", ("mode", "competitive", "format", "side", "tile", "background"))
    require(plan == {"mode": mode.title(), "competitive": str(mode == "competitive").lower(),
                    "format": storage, "side": str(side), "tile": f"0,0,{dimension},{dimension}",
                    "background": ",".join(map(str, background))}, "unexpected plan/policy/background")
    expected_tiles = [(x, y, min(side, dimension - x), min(side, dimension - y))
                      for y in range(0, dimension, side) for x in range(0, dimension, side)]
    chunks, cursor = [], 1
    for tile in expected_tiles:
        require(cursor < len(lines), "missing chunk")
        chunk = fields(lines[cursor], "chunk", ("tile", "format"))
        require(chunk == {"tile": ",".join(map(str, tile)), "format": storage}, "chunk partition/order")
        cursor += 1
        require(cursor < len(lines), "missing surface")
        surface = fields(lines[cursor], "surface", ("format", "bytes"))
        total = tile[2] * tile[3]
        require(surface == {"format": storage, "bytes": str(total * {"N16": 8, "F32": 16, "F64": 56}[storage])},
                "unexpected surface format/byte ledger or bypass")
        cursor += 1
        lo, hi, writers = total, 0, {"path": 0, "fragment": 0}
        while cursor < len(lines) and lines[cursor].startswith("writer "):
            writer = fields(lines[cursor], "writer", ("route", "offset", "count"))
            require(storage == "N16" and writer["route"] in writers, "unexpected writer route/storage")
            offset, count = integer(writer["offset"]), integer(writer["count"])
            require(count > 0 and offset + count <= total, "empty/out-of-range writer")
            require(offset // tile[2] == (offset + count - 1) // tile[2], "writer crosses its validated row")
            lo, hi = min(lo, offset), max(hi, offset + count)
            writers[writer["route"]] += 1
            cursor += 1
        require(cursor < len(lines), "missing chunk output")
        if storage == "N16":
            output = fields(lines[cursor], "output", ("storage", "start", "end", "pixels", "calls", "total"))
            lengths = [max(0, min(hi, (y + 1) * tile[2]) - max(lo, y * tile[2])) for y in range(tile[3])]
            actual = (lo, hi, sum(lengths), sum(length > 0 for length in lengths))
            require(output == dict(zip(("storage", "start", "end", "pixels", "calls", "total"),
                                       ("N16", *map(str, actual), str(total)))), "output differs from submitted interval")
            require(actual == fixed_interval, "fixed interval reach hypothesis missed")
            route = "fragment" if name == "opaque64" and mode == "competitive" else "path"
            if name == "opaque-empty":
                require(sum(writers.values()) == 0, "empty fixture wrote N16")
            else:
                require(writers[route] > 0 and writers[{"path": "fragment", "fragment": "path"}[route]] == 0,
                        "expected unique N16 writer was not observed")
        else:
            metric = "dense_pixels" if storage == "F32" else "visited_pixels"
            output = fields(lines[cursor], "output", ("storage", metric))
            require(output == {"storage": storage, metric: str(total)}, "unexpected dense/wide output")
        cursor += 1
        chunks.append({"tile": list(tile), "surface": surface, "writers": writers, "output": output})
    require(cursor == len(lines), "extra plan/chunk/writer/output or fallback")
    return plan, chunks, len(lines)


def oracle_pixels(data, dimension):
    header = f"P6\n{dimension} {dimension}\n255\n".encode()
    require(data.startswith(header) and len(data) == len(header) + dimension * dimension * 3,
            "unexpected frozen independent P6 oracle")
    return data[len(header):]


def verify_output(raw, pixels, dimension, nonopaque):
    header = f"RGBA8 {dimension} {dimension} premultiplied\n".encode()
    require(raw.startswith(header) and len(raw) == len(header) + dimension * dimension * 4,
            "invalid raw RGBA header/dimensions/payload")
    rgba = raw[len(header):]
    if nonopaque:
        require(rgba == bytes((9, 43, 85, 128)) * (dimension * dimension), "nonopaque premultiplied background mismatch")
    else:
        require(rgba[3::4] == b"\xff" * (dimension * dimension), "actual alpha is not opaque")
        require(all(rgba[channel::4] == pixels[channel::3] for channel in range(3)), "independent RGB oracle mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "preparation", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--preparation-sha256", required=True)
    args = parser.parse_args()
    preparation_bytes = bootstrap_read(args.preparation, 2**20)
    require(digest(preparation_bytes) == args.preparation_sha256, "preparation pin mismatch")
    preparation = json.loads(preparation_bytes)
    require(preparation.get("schema") == 1, "preparation schema")
    root = Path(preparation["instrumented_source"]["root"]).resolve(strict=True)
    require(root.is_absolute() and str(root) == preparation["instrumented_source"]["root"], "canonical root required")
    sys.path[:0] = [str(root / "benchmarks"), str(root / "tools")]
    from paired_render import ROOT, source_identity
    from n16_painted_interval import fixture
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    from benchkit.measurement import MeasurementCancelled, run_process
    require(ROOT == root, "imported helpers belong to another checkout")
    binary = args.binary.resolve(strict=True)
    require(preparation["binary"] == {"path": str(binary), "sha256": hash_regular_file(binary)}, "binary binding mismatch")
    require(source_identity(root) == preparation["instrumented_source"], "instrumented source binding mismatch")
    require(preparation["pre_probe_source"]["root"] == str(root), "restoration source root mismatch")
    def repository_modules():
        result = {}
        for module in tuple(sys.modules.values()):
            path = getattr(module, "__file__", None)
            if path and Path(path).resolve().is_relative_to(root):
                source = Path(path).resolve()
                require(source.suffix == ".py", "unexpected non-source repository module")
                result[str(source)] = hash_regular_file(source)
        return result

    pinned = {str(Path(__file__).resolve()): hash_regular_file(Path(__file__)), **repository_modules()}
    for path in (Path(sys.executable), Path(sys.orig_argv[0])):
        pinned[str(path.resolve(strict=True))] = hash_regular_file(path)
    evidence = [preparation["patch"], *preparation.get("build_evidence", [])]
    for item in evidence:
        require(Path(item["path"]).is_absolute() and hash_regular_file(item["path"]) == item["sha256"], "preparation artifact pin mismatch")
        pinned[item["path"]] = item["sha256"]
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "validation": "n16-interval-route-reach", "status": "running",
              "expected_outputs": 18, "completed_outputs": 0, "all_independent_rgba": False,
              "all_route_expectations": False, "instrumented_source": preparation["instrumented_source"],
              "pre_probe_source": preparation["pre_probe_source"], "binary": preparation["binary"],
              "patch": preparation["patch"], "build_evidence": preparation.get("build_evidence", []),
              "preparation_sha256": args.preparation_sha256, "harness_runtime_sha256": pinned,
              "producer_argv": sys.orig_argv, "cwd": str(Path.cwd()), "platform": platform.platform(),
              "run_id": os.environ.get("PAGURA_RUN_ID"), "run_receipt": os.environ.get("PAGURA_RUN_RECEIPT"),
              "scope": "18 native full requests only; diagnostic timing unusable; coordinator authenticates terminal build/runner/guard and restoration",
              "inputs": {}, "records": []}

    def retain(relative, data):
        path = directory / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(data)
        return {"path": str(path), "sha256": digest(data), "bytes": len(data)}

    def checkpoint():
        temporary = directory / "report.pending.json"
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        temporary.replace(directory / "report.json")

    def cancelled(signum, _frame):
        report.update(status="incomplete", all_independent_rgba=False, all_route_expectations=False)
        raise MeasurementCancelled(signum)

    previous = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
    try:
        for number in previous:
            signal.signal(number, cancelled)
        checkpoint()
        report["retained_preparation"] = retain("preparation.json", preparation_bytes)
        report["retained_build_and_patch"] = []
        for index, item in enumerate(evidence):
            data = read_bounded_regular(item["path"], MAX_BYTES)
            require(digest(data) == item["sha256"], "preparation artifact changed before archival")
            report["retained_build_and_patch"].append(retain(f"preparation-evidence/{index:02}.raw", data))
        oracles = {}
        for _, name, dimension, *_ in REQUESTS:
            if name not in oracles:
                pdf, oracle = fixture(name)
                oracles[name] = oracle_pixels(oracle, dimension)
                report["inputs"][name] = {"pdf": retain(f"inputs/{name}.pdf", pdf),
                                           "oracle": retain(f"oracles/{name}.ppm", oracle)}
        modules_before = repository_modules()
        require(all(modules_before.get(path, pin) == pin for path, pin in pinned.items()), "helper changed during fixture preparation")
        pinned.update(modules_before)
        checkpoint()
        for request in REQUESTS:
            name, fixture_name, dimension, *_ = request
            inputs = report["inputs"][fixture_name]
            background = [17, 85, 170, 128] if name == "nonopaque-empty" else [255] * 4
            for mode in ("exact", "competitive"):
                output = directory / "outputs" / f"{name}-{mode}.rgba"
                output.parent.mkdir(exist_ok=True)
                command = [str(binary), inputs["pdf"]["path"], mode, "full", str(output), "72"]
                if name == "nonopaque-empty":
                    command.append("17,85,170,128")
                record = {"request": name, "fixture": fixture_name, "mode": mode, "operation": "full", "dpi": 72,
                          "background": background, "pdf_sha256": inputs["pdf"]["sha256"],
                          "oracle_sha256": inputs["oracle"]["sha256"], "command": command,
                          "independent_rgba": False, "reach_passed": False}
                report["records"].append(record)
                checkpoint()
                try:
                    require(hash_regular_file(binary) == preparation["binary"]["sha256"], "binary changed before invocation")
                    require(all(hash_regular_file(item["path"]) == item["sha256"] for item in inputs.values()), "fixture artifact changed")
                    process, log = run_process(command, {"timeout_seconds": 120, "measure_rss": False}, log_budget=LOG_BYTES)
                    record["process"] = process
                    encoded = log.encode()
                    record["log"] = retain(f"logs/{name}-{mode}.log", encoded)
                    require(len(encoded) < LOG_BYTES and "\ufffd" not in log, "truncated/invalid UTF-8 diagnostic log")
                    require(type(process.get("exit_code")) is int and process["exit_code"] == 0 and process.get("status") != "timeout",
                            "renderer failed or timed out")
                    raw = read_bounded_regular(output, MAX_BYTES)
                    record["output"] = {"path": str(output), "sha256": digest(raw), "bytes": len(raw)}
                    verify_output(raw, oracles[fixture_name], dimension, name == "nonopaque-empty")
                    record["independent_rgba"] = True
                    record["plan"], record["chunks"], record["marker_count"] = trace(log, request, mode, background)
                    stages = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
                    require(len(stages) == 1 and isinstance(stages[0], dict), "missing/extra stage identity")
                    require(all(stages[0].get(key) == value for key, value in
                                {"schema": 1, "operation": "full", "accuracy": mode, "dpi": 72,
                                 "width": dimension, "height": dimension}.items()), "stage request identity mismatch")
                    record["raw_stages_do_not_benchmark"] = stages[0]
                    record["reach_passed"] = True
                    report["completed_outputs"] += 1
                except BaseException as error:
                    record["error"] = f"{type(error).__name__}: {error}"
                    # run_process may raise after safe drain without returning its temporary log.
                    if "log" not in record:
                        record["log_unavailable"] = "owned runner raised before returning combined diagnostic"
                    if output.exists() and "output" not in record:
                        try:
                            raw = read_bounded_regular(output, MAX_BYTES)
                            record["output"] = {"path": str(output), "sha256": digest(raw), "bytes": len(raw)}
                        except (OSError, ValueError) as capture_error:
                            record["output_capture_error"] = str(capture_error)
                    raise
                finally:
                    checkpoint()
        report["source_after"] = source_identity(root)
        report["binary_sha256_after"] = hash_regular_file(binary)
        require(report["source_after"] == preparation["instrumented_source"] and
                report["binary_sha256_after"] == preparation["binary"]["sha256"], "source/binary changed during diagnostic")
        require(all(hash_regular_file(path) == pin for path, pin in pinned.items()), "helper/runtime/preparation artifact changed")
        require(repository_modules() == modules_before, "loaded repository helper closure changed")
        require(all(hash_regular_file(item["path"]) == item["sha256"] for inputs in report["inputs"].values()
                    for item in inputs.values()), "retained fixture artifact changed")
        require(all(hash_regular_file(record[kind]["path"]) == record[kind]["sha256"]
                    for record in report["records"] for kind in ("log", "output")), "retained raw artifact changed")
        require(all(hash_regular_file(item["path"]) == item["sha256"]
                    for item in (report["retained_preparation"], *report["retained_build_and_patch"])), "retained preparation changed")
        require(len(report["records"]) == report["completed_outputs"] == 18, "incomplete fixed request matrix")
        report.update(status="verified-route-reach", all_independent_rgba=True, all_route_expectations=True)
    except BaseException as error:
        report.update(status="incomplete", all_independent_rgba=False, all_route_expectations=False,
                      fatal_error=f"{type(error).__name__}: {error}")
        raise
    finally:
        try:
            if "source_after" not in report:
                try:
                    report["source_after"] = source_identity(root)
                    report["binary_sha256_after"] = hash_regular_file(binary)
                except Exception as identity_error:
                    report["final_identity_error"] = str(identity_error)
            checkpoint()
        finally:
            for number, handler in previous.items():
                signal.signal(number, handler)
    print(directory / "report.json", flush=True)


if __name__ == "__main__":
    main()
