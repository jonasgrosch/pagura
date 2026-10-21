#!/usr/bin/env python3
"""Independent, descriptive audit of the fixed N16 Darwin RSS A/A diagnostic.

Run only after the producer, observer and outer runner receipt have completed.
Supply SHA-256 pins from the coordinator, not from the report being audited.
Imports only the standard library; never renders, launches a child, or imports
the producer. A verified audit does not change any prior performance/RSS gate.
"""
import argparse
import ast
import hashlib
import json
import math
import os
import random
import re
import stat
import statistics
from datetime import datetime
from pathlib import Path, PurePosixPath

CASES = ("outline2-knockout", "default-knockout64", "opaque64")
LIMIT = 64 << 20
WRAPPER_SOURCE = """import os, subprocess, sys
from pathlib import Path
root=Path.cwd();local=root/'benchmarks/results/local/n16-output-u16';src=local/'rss-aa-src'
with (local/'rss-aa-tests-001.log').open('x') as log:
    subprocess.run([sys.executable,'-B','-m','unittest','-v','test_rss_aa'],cwd=src,stdout=log,stderr=subprocess.STDOUT,check=True)
args=[str(src/'rss_aa.py'),'--root',str(root),'--bindings',str(root/'benchmarks/results/n16-output-u16-bindings.json'),'--binary',str(local/'candidate'),'--output',str(local/'rss-aa-001')]
os.execv(sys.executable,[sys.executable,'-B',*args])
"""


def require(ok, message):
    if not ok:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(path, limit=LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, "Unbounded/nonregular file: " + str(path))
        raw = handle.read(limit + 1)
        require(len(raw) <= limit, "File grew beyond bound: " + str(path))
        return raw


def hash_file(path):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= 1 << 30, "Unbounded identity file")
        result, size = hashlib.sha256(), 0
        while block := handle.read(1 << 20):
            size += len(block)
            require(size <= 1 << 30, "Identity file grew beyond bound")
            result.update(block)
        return result.hexdigest()


def unique(items):
    result = {}
    for key, value in items:
        require(key not in result, "Duplicate JSON key: " + key)
        result[key] = value
    return result


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError("Nonfinite JSON: " + value)))


def pinned(path, digest):
    require(isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest), "Invalid external SHA-256")
    raw = read(path)
    require(sha(raw) == digest, "Independent pin mismatch: " + str(path))
    return decode(raw)


def finished(receipt):
    require(receipt.get("schema") == 1 and receipt.get("status") == "completed"
            and receipt.get("source_unchanged") is True
            and receipt.get("source_before") == receipt.get("source_after")
            and receipt.get("errors") == [] and receipt.get("artifact_errors") == {}
            and receipt.get("cancellation_signal") is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0
                    for k in ("exit_code", "wrapper_exit_code")), "Runner did not complete successfully")
    times = [datetime.fromisoformat(receipt[k]) for k in ("started_at", "ended_at", "finalized_at")]
    require(all(t.tzinfo is not None for t in times) and times == sorted(times), "Invalid runner timestamps")


def schedule():
    rng, result = random.Random(20261104), []
    for phase, count in (("warmup", 2), ("measured", 12)):
        orders = {}
        for name in CASES:
            orders[name] = ["AB"] * (count // 2) + ["BA"] * (count // 2)
            rng.shuffle(orders[name])
        for pair in range(count):
            names = list(CASES)
            rng.shuffle(names)
            result.extend({"phase": phase, "pair": pair, "case": name,
                           "order": orders[name][pair]} for name in names)
    return result


def oracle(name):
    """Independent pixel occupancy; no PDF/font/fixture parser or producer code."""
    width, height = (512, 1024) if name == "outline2-knockout" else (2048, 2048)
    rgb = bytearray(b"\xff" * (width * height * 3))
    if name == "outline2-knockout":
        # At 10pt, L occupies x=0 at y=0..6 plus x=1..4 at y=0;
        # T occupies x=2 at y=0..5 plus x=0..4 at y=6. Advance is6.
        for line in range(64):
            for glyph in range(64):
                x, y = 16 + 6 * glyph, 16 + 14 * line
                for dy in range(7):
                    full = dy == (0 if glyph % 2 == 0 else 6)
                    left, count = (0, 5) if full else (0 if glyph % 2 == 0 else 2, 1)
                    at = ((height - 1 - y - dy) * width + x + left) * 3
                    rgb[at:at + count * 3] = bytes([191]) * (count * 3)
    else:
        gray = 0 if name == "opaque64" else 191
        for line in range(64):
            x, y = line % 8 * 256, 16 + line // 8 * 256
            for dy in range(6):
                at = ((height - 1 - y - dy) * width + x) * 3
                rgb[at:at + 256 * 3] = bytes([gray]) * (256 * 3)
    rgba = bytearray(b"\xff" * (width * height * 4))
    for channel in range(3):
        rgba[channel::4] = rgb[channel::3]
    ppm = f"P6\n{width} {height}\n255\n".encode() + bytes(rgb)
    raw = f"RGBA8 {width} {height} premultiplied\n".encode() + bytes(rgba)
    return {"width": width, "height": height, "ppm_sha256": sha(ppm), "rgba_sha256": sha(raw)}


def distribution(values):
    ordered = sorted(values)
    def quartile(fraction):
        position = fraction * (len(ordered) - 1)
        low, high = math.floor(position), math.ceil(position)
        return ordered[low] + (ordered[high] - ordered[low]) * (position - low)
    q1, q3 = quartile(.25), quartile(.75)
    return {"count": len(values), "values": values, "minimum": ordered[0],
            "q1": q1, "median": statistics.median(values), "q3": q3,
            "iqr": q3 - q1, "maximum": ordered[-1]}


def summary(pairs):
    a, b = ([p[side]["rss_bytes"] for p in pairs] for side in "AB")
    first, second = ([p[p["order"][position]]["rss_bytes"] for p in pairs] for position in (0, 1))
    return {"complete_measured_pairs": len(pairs),
            "rss_bytes_by_side": {"A": distribution(a), "B": distribution(b)},
            "paired_B_minus_A_bytes": distribution([y - x for x, y in zip(a, b)]),
            "rss_bytes_by_position": {"first": distribution(first), "second": distribution(second)},
            "paired_second_minus_first_bytes": distribution([y - x for x, y in zip(first, second)]),
            "B_minus_A_bytes_by_order": {order: distribution([
                p["B"]["rss_bytes"] - p["A"]["rss_bytes"] for p in pairs if p["order"] == order])
                for order in ("AB", "BA")},
            "maximum_ratios": {"B_over_A": max(b) / max(a), "A_over_B": max(a) / max(b)}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("directory", "receipt", "guard", "driver", "wrapper", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    for name in ("receipt", "guard", "driver", "wrapper", "bindings"):
        parser.add_argument("--" + name + "-sha256", required=True)
    args = parser.parse_args()
    directory, receipt_path, guard_path, driver = (
        p.resolve() for p in (args.directory, args.receipt, args.guard, args.driver))
    wrapper = args.wrapper.resolve()
    receipt, guard = pinned(receipt_path, args.receipt_sha256), pinned(guard_path, args.guard_sha256)
    finished(receipt)
    require(receipt.get("kind") == "benchmark", "Wrong runner kind")
    report_raw = read(directory / "report.json")
    report = decode(report_raw)
    require(receipt["artifacts"].get(str(directory / "report.json")) == sha(report_raw)
            and receipt["artifacts"].get(str(guard_path)) == args.guard_sha256, "Receipt does not bind report/observer")
    require(report.get("schema") == 1 and report.get("diagnostic") == "same-binary-darwin-rss-aa"
            and report.get("status") == "completed-diagnostic" and report.get("admission") is False
            and report.get("thresholds_changed") is False and report.get("verified_processes") == 84
            and "error" not in report and report.get("deadline_seconds") == 600, "Incomplete or altered diagnostic")
    require(report["runner"] == {"run_id": receipt["run_id"], "receipt": str(receipt_path)}
            and receipt["environment_overrides"] == {"PAGURA_RUN_ID": receipt["run_id"],
                "PAGURA_RUN_RECEIPT": str(receipt_path)}, "Runner pointer/environment mismatch")
    require(type(report["duration_seconds"]) in (int, float) and 0 < report["duration_seconds"] <= 600,
            "Completed diagnostic exceeds fixed deadline")
    require(not (directory / "report.pending").exists() and not (directory / "pending.rgba").exists(),
            "Completed diagnostic retains unfinished output")

    sizes, hashes = {}, {}
    def artifact(name):
        part = PurePosixPath(name)
        require(not part.is_absolute() and ".." not in part.parts and str(part) == name, "Unsafe artifact path")
        path = directory / name
        require(path.resolve().is_relative_to(directory), "Artifact escapes evidence directory")
        raw = read(path)
        sizes[name], hashes[name] = len(raw), sha(raw)
        return raw

    plan_raw = artifact("plan.json")
    require(sha(plan_raw) == report["plan_sha256"], "Plan hash mismatch")
    plan = decode(plan_raw)
    binding_raw = artifact("bindings.json")
    require(sha(binding_raw) == args.bindings_sha256 == plan["bindings_sha256"], "Frozen bindings pin mismatch")
    binding = decode(binding_raw)
    identity = binding["identities"]["candidate"]
    root, binary = Path(identity["root"]).resolve(), Path(identity["binary"]).resolve()
    require(receipt["source_root"] == receipt["cwd"] == str(root)
            and receipt["source_before"] == {"commit": identity["commit"],
                "sha256": binding["candidate_runner_source_sha256"]}, "Runner source mismatch")
    coordination = receipt["coordination"]
    require(coordination["scope"] == "cooperative"
            and Path(coordination["lock_file"]).resolve() == Path("/tmp/pagura-bench.lock").resolve()
            and Path(coordination["lock_directory"]).resolve() == Path("/tmp/pagura-bench.lockdir").resolve(),
            "Wrong coordination contract")
    require(hash_file(binary) == identity["binary_sha256"] == plan["binary_sha256"] == report["binary_sha256"]
            and plan["binary"] == str(binary), "Same-binary identity mismatch")
    build_path = root / "benchmarks/results" / binding["candidate_build_receipt"]
    build = pinned(build_path, binding["candidate_build_receipt_sha256"])
    finished(build)
    require(build["source_root"] == str(root) and build["source_before"] == receipt["source_before"]
            and identity["binary_sha256"] in build["artifacts"].values(), "Build/source/binary correspondence mismatch")
    expected_plan = {"schema": 1, "seed": 20261104, "warmup_pairs_per_case": 2,
                     "measured_pairs_per_case": 12, "processes": 84, "deadline_seconds": 600,
                     "mode": "exact", "operation": "full", "dpi": 72,
                     "same_absolute_binary_for_both_labels": True}
    require(all(plan.get(k) == v for k, v in expected_plan.items()) and plan["schedule"] == schedule(), "Fixed plan changed")
    platform_name = plan["runtime"]["platform"]
    require(plan["runtime"]["time_wrapper"] == ["/usr/bin/time", "-l"]
            and isinstance(platform_name, str) and platform_name.startswith(("Darwin-", "macOS-"))
            and plan["rss"]["source"] == "/usr/bin/time -l"
            and plan["rss"]["units"] == "bytes", "Wrong RSS runtime/units")

    observed_command = guard["command"]
    require(isinstance(observed_command, list) and len(observed_command) in (2, 3)
            and all(isinstance(v, str) for v in observed_command), "Unexpected observer wrapper command")
    wrapper_position = 2 if len(observed_command) == 3 and observed_command[1] == "-B" else 1
    require(wrapper_position == len(observed_command) - 1, "Unexpected wrapper options")
    resolve_arg = lambda value: (root / value).resolve()
    interpreter = Path(plan["runtime"]["executable"]).resolve()
    require(observed_command[0] in ("python3", interpreter.name, str(interpreter))
            or resolve_arg(observed_command[0]) == interpreter, "Unexpected wrapper interpreter")
    local = root / "benchmarks/results/local/n16-output-u16"
    require(resolve_arg(observed_command[wrapper_position]) == wrapper == local / "run_rss_aa.py",
            "Observer does not invoke the fixed wrapper")
    wrapper_raw = read(wrapper, 65536)
    require(sha(wrapper_raw) == args.wrapper_sha256, "Independent wrapper source pin mismatch")
    require(ast.dump(ast.parse(wrapper_raw), include_attributes=False)
            == ast.dump(ast.parse(WRAPPER_SOURCE), include_attributes=False),
            "Wrapper differs from reviewed tests-then-exec program")
    # The exact AST fixes cwd=root, the test command/check=True, and execv's
    # arguments. This reconstructs that literal program; it does not execute it.
    command = [str(interpreter), "-B", str(local / "rss-aa-src/rss_aa.py"),
               "--root", str(root), "--bindings", str(root / "benchmarks/results/n16-output-u16-bindings.json"),
               "--binary", str(local / "candidate"), "--output", str(local / "rss-aa-001")]
    position = 2
    require(resolve_arg(command[position]) == driver and hash_file(driver) == args.driver_sha256, "Wrong producer driver")
    tail = command[position + 1:]
    require(len(tail) == 8 and len(set(tail[::2])) == 4, "Duplicate/missing producer options")
    options = dict(zip(tail[::2], tail[1::2]))
    require(set(options) == {"--root", "--bindings", "--binary", "--output"}
            and resolve_arg(options["--root"]) == root and resolve_arg(options["--binary"]) == binary
            and resolve_arg(options["--output"]) == directory, "Producer invocation differs")
    bindings_path = resolve_arg(options["--bindings"])
    require(hash_file(bindings_path) == args.bindings_sha256, "Live frozen bindings differ")
    argv = receipt["argv"]
    require(argv[-len(observed_command):] == observed_command and argv[-len(observed_command) - 1] == "--",
            "Runner does not invoke this observed wrapper command")
    require("--evidence-dir" in argv and resolve_arg(argv[argv.index("--evidence-dir") + 1]) == guard_path.parent,
            "Runner observer directory differs")
    observer = "benchmarks/results/local/glyph-memo/load_observer.py"
    require(any(resolve_arg(value) == root / observer for value in argv)
            and hash_file(root / observer) == binding["bound_files_sha256"][observer],
            "Observer source/invocation is not the frozen helper")
    manifest = plan["bound_files_sha256"]
    required = {driver, bindings_path, Path(plan["runtime"]["executable"]).resolve(),
                Path("/usr/bin/time"), Path("/bin/ps")}
    required.update(root / p for p in ("tools/benchkit/measurement.py", "tools/benchkit/process_group.py",
        "tools/benchkit/artifacts.py", "tools/benchkit/images.py", "tools/fixturegen/font.py",
        "tools/fixturegen/pdf.py", "benchmarks/scene_index.py", "benchmarks/paired_render.py",
        "benchmarks/glyph_memo_fixtures.py", "crates/pagura-render/examples/scene_index_profile.rs"))
    require(all(str(p) in manifest for p in required) and manifest[str(driver)] == args.driver_sha256,
            "Incomplete source/runtime closure")
    for path, digest in manifest.items():
        require(Path(path).is_absolute() and hash_file(Path(path)) == digest, "Bound file changed: " + path)
        relative = str(Path(path).relative_to(root)) if Path(path).is_relative_to(root) else None
        if relative in binding["bound_files_sha256"]:
            require(binding["bound_files_sha256"][relative] == digest, "Frozen helper differs: " + path)
    require(guard.get("status") == "completed-observation-only" and guard.get("observation_only") is True
            and all(type(guard.get(k)) is int and guard[k] == 0 for k in ("exit_code", "child_exit_code")),
            "Observer did not complete")
    events = guard["events"]
    require(isinstance(events, list) and any(e.get("stage") == "preflight" for e in events)
            and any(e.get("stage") == "postflight" for e in events)
            and all(isinstance(e.get("matches"), list) for e in events), "Incomplete observer coverage")
    chronology = [datetime.fromisoformat(receipt["started_at"]).timestamp(),
                  datetime.fromisoformat(guard["started_utc"]).timestamp(),
                  datetime.fromisoformat(guard["launched_utc"]).timestamp(),
                  report["started_unix_ns"] / 1e9, report["finished_unix_ns"] / 1e9,
                  datetime.fromisoformat(guard["finished_utc"]).timestamp(),
                  datetime.fromisoformat(receipt["ended_at"]).timestamp()]
    require(chronology == sorted(chronology), "Observer/producer/runner intervals do not nest")

    expected = {name: oracle(name) for name in CASES}
    for name, value in expected.items():
        pdf = artifact(f"inputs/{name}.pdf")
        ppm = artifact(f"oracles/{name}.ppm")
        info = plan["inputs"][name]
        require(pdf.startswith(b"%PDF-") and sha(pdf) == info["pdf_sha256"]
                and sha(ppm) == info["oracle_sha256"] == value["ppm_sha256"]
                and all(info[k] == value[k] for k in ("width", "height")), "Input/analytic oracle mismatch: " + name)
    require(len(report["records"]) == 42, "Incomplete pair records")
    last_finish, checked = 0, 0
    for sequence, (pair, planned) in enumerate(zip(report["records"], schedule())):
        require(pair["sequence"] == sequence and all(pair[k] == v for k, v in planned.items()), "Schedule mismatch")
        begin, end = pair["started_monotonic_ns"], pair["finished_monotonic_ns"]
        require(type(begin) is int and type(end) is int and last_finish <= begin <= end, "Pair order/timestamps invalid")
        last_sample = begin
        for side in pair["order"]:
            record, name = pair[side], pair["case"]
            require(record["verified"] is True and record["command"] == [str(binary),
                str(directory / f"inputs/{name}.pdf"), "exact", "full", str(directory / "pending.rgba"), "72"],
                "Invocation was not the same frozen binary/request")
            start, finish = record["started_monotonic_ns"], record["finished_monotonic_ns"]
            require(type(start) is int and type(finish) is int and last_sample <= start <= finish <= end,
                    "Sample order/timestamps invalid")
            last_sample = finish
            process = record["process"]
            require(type(process.get("exit_code")) is int and process["exit_code"] == 0
                    and process.get("status") == "failed" and process.get("rss_status") == "available",
                    "Child was not a successful raw run_process result")
            require(type(process.get("elapsed_ns")) is int and process["elapsed_ns"] > 0,
                    "Invalid child elapsed time")
            log_name = f"logs/{sequence:03d}-{side}.stdout-stderr.txt"
            require(record["log"] == log_name, "Wrong per-invocation log")
            raw = artifact(log_name)
            require(len(raw) < 256 << 10 and len(raw) == record["log_bytes"]
                    and sha(raw) == record["log_sha256"], "Raw log hash/size mismatch")
            log = raw.decode("ascii")
            matches = re.findall(r"^[ \t]*([0-9]+)[ \t]+maximum resident set size[ \t]*$", log, re.MULTILINE)
            require(len(matches) == 1 and re.findall(r"([0-9]+)\s+maximum resident set size", log) == matches,
                    "Missing/ambiguous raw Darwin RSS field")
            rss = int(matches[0])
            require(0 < rss < 1 << 64 and type(record["rss_bytes"]) is int
                    and type(process["peak_rss_bytes"]) is int
                    and rss == record["rss_bytes"] == process["peak_rss_bytes"], "RSS bytes disagree")
            stages = [decode(line) for line in log.splitlines() if line.startswith("{")]
            require(len(stages) == 1 and stages[0] == record["stages"], "Stage log/report mismatch")
            stage, image = stages[0], expected[name]
            require(stage["schema"] == 1 and stage["accuracy"] == "exact" and stage["operation"] == "full"
                    and stage["dpi"] == 72 and all(stage[k] == image[k] for k in ("width", "height"))
                    and all(type(stage.get(k)) is int and stage[k] > 0 for k in
                            ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
                    and stage["metric_ns"] == stage["render_ns"]
                    and stage["total_ns"] >= sum(stage[k] for k in ("open_ns", "compile_ns", "render_ns")),
                    "Invalid stage identity/counters")
            blob = "blobs/" + image["rgba_sha256"] + ".rgba"
            require(record["blob"] == blob and record["output_sha256"] == image["rgba_sha256"], "Wrong independent RGBA hash")
            if blob not in hashes:
                require(sha(artifact(blob)) == image["rgba_sha256"], "Raw RGBA/alpha does not match analytic image")
            checked += 1
        require(pair["A"]["output_sha256"] == pair["B"]["output_sha256"], "A/A pixels differ")
        last_finish = end
    require(checked == 84 and sum(sizes.values()) == report["stored_bytes"] <= 128 << 20, "Sample/retention accounting mismatch")
    results = {name: summary([p for p in report["records"] if p["case"] == name and p["phase"] == "measured"])
               for name in CASES}
    require(results == report["summaries"] and all(r["complete_measured_pairs"] == 12 for r in results.values()),
            "Descriptive summaries do not reproduce")
    result = {"schema": 1, "status": "verified-evidence", "diagnostic": report["diagnostic"],
              "run_id": receipt["run_id"], "admission": False, "thresholds_changed": False,
              "prior_failed_gates_unchanged": True, "verified_processes": checked,
              "report_sha256": sha(report_raw), "receipt_sha256": args.receipt_sha256,
              "guard_sha256": args.guard_sha256, "driver_sha256": args.driver_sha256,
              "wrapper_sha256": args.wrapper_sha256, "wrapper_derived_producer_argv": command,
              "bindings_sha256": args.bindings_sha256, "plan_sha256": sha(plan_raw),
              "auditor_sha256": hash_file(Path(__file__).resolve()), "artifacts_sha256": hashes,
              "background_contention_observed": any(e["matches"] for e in events), "summaries": results,
              "decision": "Descriptive same-binary diagnostic only; prior performance/RSS failures remain unchanged.",
              "limitations": ["Recorded cooperative execution, not hostile-host attestation.",
                  "PDF construction trusts the pinned generator; expected pixels are independently derived.",
                  "Observer cannot exclude short or unrecognized competing work.",
                  "OS child high-water, not render-only allocation, total system memory, or a calibrated confidence bound."]}
    raw = (json.dumps(result, indent=2, allow_nan=False) + "\n").encode()
    with args.output.resolve().open("xb") as handle:
        handle.write(raw)
    print(json.dumps({"status": result["status"], "verified_processes": checked,
                      "audit_sha256": sha(raw), "output": str(args.output.resolve())}))


if __name__ == "__main__":
    main()
