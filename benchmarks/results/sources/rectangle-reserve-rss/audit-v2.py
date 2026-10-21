#!/usr/bin/env python3
"""Independent retained-evidence audit of the fixed rectangle RSS A/A trial.

Standard library only. No producer imports, subprocesses, rendering, resampling,
or admission decisions. The coordinator independently pins final evidence.
"""
import argparse
from datetime import datetime
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import random
import re
import stat
import statistics

LIMIT = 64 << 20
MANIFEST_SHA = "278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3"
REFERENCES_SHA = "924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf"
CASES = (
    "classic/blank", "classic/rectangle", "classic/graphics-state",
    "classic/negative-rectangle", "classic/inherited-box", "classic/clipped-to-page",
    "classic/many-rectangles", "classic/content-array-split", "classic/many-operands",
    "paint/text-heavy-page", "paint/text-mode-4", "paint/type3-d0-colour",
    "paint/text-knockout-true", "paint/text-knockout-false", "paint/image-soft-mask",
    "paint/image-direct-binary-halves", "paint/image-direct-gray-quarter-turn",
    "paint/image-direct-rgb-fractional", "paint/image-direct-bilinear",
)
CELLS = (
    ("classic/graphics-state", "exact"),
    ("paint/text-knockout-true", "exact"),
    ("paint/text-knockout-true", "competitive"),
    ("paint/image-direct-gray-quarter-turn", "exact"),
    ("classic/many-rectangles", "competitive"),
    ("classic/many-operands", "exact"),
    ("classic/many-operands", "competitive"),
)
ROLES = ("baseline", "candidate")
STRATA = tuple((role, name, mode) for role in ROLES for name, mode in CELLS)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def read(path, limit=LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit,
                "unbounded/nonregular evidence: " + str(path))
        raw = handle.read(limit + 1)
    require(len(raw) <= limit, "evidence grew beyond bound")
    return raw


def file_sha(path):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(fd, "rb") as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= 1 << 30, "unbounded identity file")
        digest, size = hashlib.sha256(), 0
        while block := handle.read(1 << 20):
            size += len(block)
            require(size <= 1 << 30, "identity file grew beyond bound")
            digest.update(block)
    return digest.hexdigest()


def unique(items):
    result = {}
    for key, value in items:
        require(key not in result, "duplicate JSON key: " + key)
        result[key] = value
    return result


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique,
                      parse_constant=lambda x: (_ for _ in ()).throw(ValueError("nonfinite JSON: " + x)))


def bound(pin):
    require(set(pin) == {"path", "sha256"} and Path(pin["path"]).is_absolute()
            and re.fullmatch(r"[0-9a-f]{64}", pin["sha256"]), "invalid independent pin")
    raw = read(pin["path"])
    require(sha(raw) == pin["sha256"], "pin mismatch: " + pin["path"])
    return raw


def within(root, relative):
    pure = PurePosixPath(relative)
    require(not pure.is_absolute() and ".." not in pure.parts and str(pure) == relative,
            "unsafe artifact path")
    path = root / relative
    require(path.resolve().is_relative_to(root), "artifact escapes root")
    return path


def positive(value):
    return type(value) is int and 0 < value < 1 << 64


def timestamp(value):
    result = datetime.fromisoformat(value)
    require(result.tzinfo is not None, "timestamp lacks timezone")
    return result.timestamp()


def completed(receipt):
    require(receipt.get("schema") == 1 and receipt.get("status") == "completed"
            and receipt.get("source_unchanged") is True
            and receipt.get("source_before") == receipt.get("source_after")
            and receipt.get("errors") == [] and receipt.get("artifact_errors") == {}
            and receipt.get("cancellation_signal") is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0
                    for k in ("exit_code", "wrapper_exit_code")), "runner failed or unfinished")
    times = [timestamp(receipt[k]) for k in ("started_at", "ended_at", "finalized_at")]
    require(times == sorted(times), "runner timestamps unordered")


def source_digest(root, runner=False):
    paths = set()
    for directory in ("crates", "tools", "benchmarks/adapters"):
        paths.update(p for p in (root / directory).rglob("*") if p.is_file()
                     and "target" not in p.parts and "__pycache__" not in p.parts)
    paths.update((root / "corpus/manifests").glob("*.json"))
    names = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"]
    if runner:
        names += ["benchmarks/release-matrix.json", "benchmarks/requirements.txt",
                  "benchmarks/node-package.json", "benchmarks/node-package-lock.json"]
    paths.update(root / name for name in names if not runner or (root / name).exists())
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + read(path))
    return digest.hexdigest()


def schedule():
    rng, result = random.Random(20261113), []
    for phase, count in (("warmup", 2), ("measured", 200)):
        orders = []
        for _ in STRATA:
            values = ["AB"] * (count // 2) + ["BA"] * (count // 2)
            rng.shuffle(values)
            orders.append(values)
        for pair in range(count):
            indices = list(range(len(STRATA)))
            rng.shuffle(indices)
            for index in indices:
                role, name, mode = STRATA[index]
                result.append({"phase": phase, "pair": pair, "binary_role": role,
                               "case": name, "mode": mode, "operation": "cold",
                               "order": orders[index][pair]})
    return result


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
    first, second = ([p[p["order"][i]]["rss_bytes"] for p in pairs] for i in (0, 1))
    return {"complete_measured_pairs": len(pairs),
            "rss_bytes_by_side": {"A": distribution(a), "B": distribution(b)},
            "paired_B_minus_A_bytes": distribution([y - x for x, y in zip(a, b)]),
            "rss_bytes_by_position": {"first": distribution(first), "second": distribution(second)},
            "paired_second_minus_first_bytes": distribution([y - x for x, y in zip(first, second)]),
            "B_minus_A_bytes_by_order": {order: distribution([
                p["B"]["rss_bytes"] - p["A"]["rss_bytes"] for p in pairs if p["order"] == order])
                for order in ("AB", "BA")},
            "maximum_ratios": {"B_over_A": max(b) / max(a), "A_over_B": max(a) / max(b)}}


def expected_rgba(raw, width, height):
    match = re.match(rb"P6\n([1-9][0-9]*) ([1-9][0-9]*)\n255\n", raw)
    require(match is not None and tuple(map(int, match.groups())) == (width, height),
            "expected frozen canonical RGB8 P6 dimensions")
    rgb = raw[match.end():]
    require(len(rgb) == width * height * 3, "reference payload length mismatch")
    rgba = bytearray(b"\xff" * (width * height * 4))
    for channel in range(3):
        rgba[channel::4] = rgb[channel::3]
    return f"RGBA8 {width} {height} premultiplied\n".encode() + bytes(rgba)


def references(root):
    directory = root / "benchmarks/results/sources/analytic19-conversion"
    manifest = decode(bound({"path": str(directory / "manifest.json"), "sha256": MANIFEST_SHA}))
    refs = decode(bound({"path": str(directory / "references.json"), "sha256": REFERENCES_SHA}))
    require(manifest["schema"] == refs["schema"] == 1
            and [d["id"] for d in manifest["documents"]] == list(CASES)
            and [r["id"] for r in refs["cases"]] == refs["selector"]["case_ids"] == list(CASES),
            "frozen analytic19 identity mismatch")
    expected = {}
    for document, ref in zip(manifest["documents"], refs["cases"]):
        require(document["pages"] == 1 and document["selected_pages"] == [0]
                and len(document["oracles"]) == 1, "wrong independent page contract")
        oracle = document["oracles"][0]
        require((oracle["page"], oracle["dpi"], oracle["tolerance"]) == (0, 72, 0)
                and ref["expected"] == "render" and ref["dpi"] == 72
                and (ref["path"], ref["sha256"], ref["oracle"], ref["oracle_sha256"])
                == (document["path"], document["sha256"], oracle["path"], oracle["sha256"]),
                "reference settings/identity mismatch")
        if document["id"] not in {name for name, _ in CELLS}:
            continue
        pdf = bound({"path": str(within(root, document["path"])), "sha256": document["sha256"]})
        raw = bound({"path": str(within(root, oracle["path"])), "sha256": oracle["sha256"]})
        width, height = ref["width"], ref["height"]
        require(pdf.startswith(b"%PDF-"), "reference PDF header mismatch")
        actual = expected_rgba(raw, width, height)
        expected[document["id"]] = {"pdf": pdf, "oracle": raw, "rgba": actual,
                                    "width": width, "height": height}
    return expected


def build_binding(bindings):
    identities = bindings["identities"]
    require(bindings["schema"] == 1 and bindings["experiment"] == "rectangle-reserve"
            and set(identities) == set(ROLES), "wrong original rectangle binding")
    require(len({i["binary"] for i in identities.values()}) == 2
            and len({i["binary_sha256"] for i in identities.values()}) == 2, "binary strata collapsed")
    build_pin = bindings["build_report"]
    build = decode(bound(build_pin))
    require(build["status"] == "built-both-arms" and build["identities"] == identities, "build identities differ")
    require(build["build_environment"] == {"CARGO_BUILD_JOBS": "1", "CARGO_BUILD_TARGET": None,
        "RUSTFLAGS": None, "CARGO_ENCODED_RUSTFLAGS": None, "RUSTC_WRAPPER": None,
        "RUSTC_WORKSPACE_WRAPPER": None, "CARGO_PROFILE_RELEASE_DEBUG": None,
        "CARGO_PROFILE_RELEASE_LTO": None}, "historical build environment differs")
    for path, digest in build["config_sha256"].items():
        require(Path(path).is_absolute() and file_sha(path) == digest, "historical Cargo configuration changed")
    owners = [entry for entry in bindings["completed_evidence"]
              if entry["artifacts"].get(build_pin["path"]) == build_pin["sha256"]]
    require(len(owners) == 1, "missing/ambiguous final build receipt")
    owner = owners[0]
    receipt = decode(bound(owner["receipt"]))
    completed(receipt)
    require(receipt["run_id"] == build["run_id"]
            and receipt["artifacts"].get(build_pin["path"]) == build_pin["sha256"],
            "build not bound by completed outer receipt")
    for role, identity in identities.items():
        root = Path(identity["root"])
        require(root.is_absolute() and Path(identity["binary"]).is_absolute()
                and file_sha(identity["binary"]) == identity["binary_sha256"]
                and source_digest(root) == identity["source_sha256"], "historical source or binary drift")
        item = build["builds"][role]
        source = {k: identity[k] for k in ("root", "commit", "source_sha256")}
        require(item["source_before"] == item["source_after"] == source
                and type(item["process"]["exit_code"]) is int and item["process"]["exit_code"] == 0
                and item["process"]["status"] == "failed", "build failed or source changed")
        require(item["command"] == ["cargo", "build", "--locked", "--offline", "--release",
                "-p", "pagura-render", "--example", "scene_index_profile", "--manifest-path",
                str(root / "Cargo.toml"), "--target-dir", str(root / "target"),
                "--message-format=json", "-j", "1"], "build request changed")
        log = read(Path(build_pin["path"]).parent / (role + ".log"), 1 << 20)
        require(sha(log) == item["log_sha256"], "build log hash mismatch")
        records = [decode(line) for line in log.splitlines() if line.startswith(b"{")]
        cargo = item["cargo_artifact"]
        require(cargo in records and cargo["reason"] == "compiler-artifact"
                and cargo["target"]["name"] == "scene_index_profile"
                and cargo["target"]["kind"] == ["example"] and cargo["features"] == []
                and cargo["profile"]["opt_level"] == "3" and cargo["profile"]["test"] is False
                and cargo["executable"] == str(root / "target/release/examples/scene_index_profile"),
                "wrong native Cargo artifact")
    return identities


def verify_log(raw, record, mode, image):
    require(len(raw) < 256 << 10, "potentially truncated raw process log")
    log = raw.decode("ascii")
    matches = re.findall(r"^[ \t]*([0-9]+)[ \t]+maximum resident set size[ \t]*$", log, re.MULTILINE)
    require(len(matches) == 1 and re.findall(r"([0-9]+)\s+maximum resident set size", log) == matches,
            "missing/ambiguous Darwin RSS field")
    rss, process = int(matches[0]), record["process"]
    require(positive(rss) and positive(record["rss_bytes"]) and positive(process["peak_rss_bytes"])
            and rss == record["rss_bytes"] == process["peak_rss_bytes"]
            and process["rss_status"] == "available" and process["status"] == "failed"
            and type(process["exit_code"]) is int and process["exit_code"] == 0
            and positive(process["elapsed_ns"]), "invalid child/RSS result")
    stages = [decode(line) for line in log.splitlines() if line.startswith("{")]
    require(len(stages) == 1 and stages[0] == record["stages"], "stage log/report mismatch")
    stage = stages[0]
    require(stage["schema"] == 1 and stage["accuracy"] == mode and stage["operation"] == "cold"
            and stage["dpi"] == 72 and all(stage[k] == image[k] for k in ("width", "height"))
            and all(positive(stage.get(k)) for k in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
            and stage["metric_ns"] == stage["total_ns"]
            and stage["total_ns"] >= sum(stage[k] for k in ("open_ns", "compile_ns", "render_ns")),
            "stage settings/counters invalid")


def verify_python_launch(pins, plan, report):
    """Verify this macOS framework handoff without claiming a pre-run app pin."""
    launch = pins.get("python_launch")
    require(type(launch) is dict and set(launch) == {
        "requested_argv0", "runtime_executable", "actual_argv0", "actual_pin_timing"},
        "missing/invalid independently pinned Python handoff")
    require(launch["actual_pin_timing"] == "post-run", "runtime handoff pin timing changed")
    command, actual = pins["producer_argv"], report["producer_argv"]
    require(type(command) is list and type(actual) is list and len(command) == len(actual) > 0
            and all(type(value) is str for value in command + actual)
            and actual[1:] == command[1:], "actual Python arguments differ")
    require(type(launch["requested_argv0"]) is str and launch["requested_argv0"] == command[0]
            and Path(command[0]).is_absolute(), "requested Python launcher differs")
    paths = {}
    for name in ("runtime_executable", "actual_argv0"):
        pin = launch[name]
        require(type(pin) is dict and set(pin) == {"path", "sha256"}
                and type(pin["path"]) is str and type(pin["sha256"]) is str
                and re.fullmatch(r"[0-9a-f]{64}", pin["sha256"]), "invalid Python executable pin")
        path = Path(pin["path"])
        require(path.is_absolute() and str(path) == pin["path"] and path.resolve() == path,
                "Python executable pin is not canonical absolute path")
        paths[name] = path
    runtime, executable = paths["runtime_executable"], paths["actual_argv0"]
    version = runtime.parent.parent
    require(runtime.name == "python3.14" and runtime.parent.name == "bin"
            and version.name == "3.14" and version.parent.name == "Versions"
            and version.parent.parent.name == "Python.framework"
            and executable == version / "Resources/Python.app/Contents/MacOS/Python",
            "Python handoff crosses framework/version or executable suffix")
    require(Path(command[0]).resolve() == runtime and plan["runtime"]["executable"] == str(runtime)
            and actual[0] == str(executable), "Python launcher/runtime/observed identity differs")
    closure = plan["bound_files_sha256"]
    require(closure.get(str(runtime)) == launch["runtime_executable"]["sha256"]
            and str(executable) not in closure, "recorded launcher/post-run app pin scope differs")
    for name, path in paths.items():
        require(file_sha(path) == launch[name]["sha256"], "current Python executable hash differs: " + name)


def coordination(pins, root, directory, plan, report):
    receipt, guard = (decode(bound(pins[name])) for name in ("receipt", "guard"))
    completed(receipt)
    require(receipt["kind"] == "benchmark" and receipt["runner"]["system"] == "Darwin"
            and receipt["cwd"] == receipt["source_root"] == str(root)
            and receipt["source_before"] == plan["runner_source_before"] == report["runner_source_before"]
            and receipt["source_before"]["sha256"] == source_digest(root, runner=True), "current runner/source mismatch")
    pointer = {"run_id": receipt["run_id"], "receipt": pins["receipt"]["path"]}
    require(report["runner"] == pointer and receipt["environment_overrides"] == {
        "PAGURA_RUN_ID": pointer["run_id"], "PAGURA_RUN_RECEIPT": pointer["receipt"]}, "runner pointer/environment mismatch")
    contract = receipt["coordination"]
    require(contract["scope"] == "cooperative" and all(Path(contract[k]).resolve() == Path(v).resolve()
            for k, v in (("lock_file", "/tmp/pagura-bench.lock"), ("lock_directory", "/tmp/pagura-bench.lockdir"))),
            "wrong cooperative coordination")
    command = pins["producer_argv"]
    require(len(command) == 11 and command[1] == "-B" and Path(command[0]).is_absolute()
            and Path(command[0]).resolve() == Path(plan["runtime"]["executable"])
            and Path(command[2]).is_absolute() and command[2] == pins["producer"]["path"],
            "wrong direct absolute producer invocation")
    tail = command[3:]
    require(len(set(tail[::2])) == 4 and dict(zip(tail[::2], tail[1::2])) == {
        "--root": str(root), "--bindings": pins["bindings"]["path"],
        "--bindings-sha256": pins["bindings"]["sha256"], "--output": str(directory)}, "producer options changed")
    verify_python_launch(pins, plan, report)
    require(guard["schema"] == 1 and guard["status"] == "completed" and guard["observation_only"] is False
            and all(type(guard[k]) is int and guard[k] == 0 for k in ("exit_code", "child_exit_code"))
            and guard["command"] == command and guard["poll_seconds"] == 5 and guard["window_seconds"] == 5400
            and type(guard["scans"]) is int and guard["scans"] >= 2, "observer did not complete strict protocol")
    observer = root / "benchmarks/results/sources/analytic19-conversion/load_observer.py"
    argv = receipt["argv"]
    require((argv[0] == "python3" or (Path(argv[0]).is_absolute()
            and Path(argv[0]).resolve() == Path(plan["runtime"]["executable"])))
            and argv[1] == "-B" and len(argv) == len(command) + 6
            and (root / argv[2]).resolve() == observer and argv[3] == "--evidence-dir"
            and (root / argv[4]).resolve() == Path(pins["guard"]["path"]).parent
            and argv[5:] == ["--", *command], "runner→observer→producer chain differs")
    require(set(pins["coordination_sources"]) == {str(root / "tools/runner.py"), str(observer)}, "incomplete coordination pins")
    for path, digest in pins["coordination_sources"].items():
        require(file_sha(path) == digest, "coordination source changed")
    for name in ("plan", "report", "journal", "guard"):
        item = pins[name]
        require(receipt["artifacts"].get(item["path"]) == item["sha256"], "terminal receipt does not bind " + name)
    events = guard["events"]
    require([event["stage"] for event in events] == ["preflight", "postflight"]
            and all(event["matches"] == [] for event in events), "incomplete/contentious observation")
    paths = {str(within(Path(pins["guard"]["path"]).parent, event["snapshot"])) for event in events}
    require(set(pins["observer_snapshots"]) == paths, "observer snapshots lack independent pins")
    for path, digest in pins["observer_snapshots"].items():
        require(bool(bound({"path": path, "sha256": digest})), "empty observer snapshot")
    times = [timestamp(receipt["started_at"]), timestamp(guard["started_utc"]),
             timestamp(guard["launched_utc"]), report["started_unix_ns"] / 1e9,
             report["finished_unix_ns"] / 1e9, timestamp(guard["finished_utc"]), timestamp(receipt["ended_at"])]
    require(times == sorted(times), "execution intervals do not nest")
    return receipt


def audit(pins):
    root, directory = (Path(pins[k]).resolve() for k in ("root", "directory"))
    for name, basename in (("plan", "plan.json"), ("report", "report.json"), ("journal", "events.jsonl")):
        require(Path(pins[name]["path"]) == directory / basename, "wrong evidence location")
    report, plan, bindings = (decode(bound(pins[k])) for k in ("report", "plan", "bindings"))
    require(report["schema"] == 1 and report["diagnostic"] == "rectangle-reserve-darwin-rss-aa"
            and report["status"] == "completed-diagnostic" and report["admission"] is False
            and report["thresholds_changed"] is False and report["verified_processes"] == 5656
            and report["deadline_seconds"] == 900 and "error" not in report,
            "failed/incomplete/changed diagnostic; retained failures cannot pass")
    require(type(report["duration_seconds"]) in (int, float) and 0 < report["duration_seconds"] <= 900
            and positive(report["started_unix_ns"]) and positive(report["finished_unix_ns"]), "invalid duration")
    identities = build_binding(bindings)
    require(identities["candidate"]["root"] == str(root) and plan["identities"] == report["identities"] == identities,
            "binary/source strata differ")
    expected_plan = {"schema": 1, "seed": 20261113, "warmup_pairs_per_stratum": 2,
        "measured_pairs_per_stratum": 200, "processes": 5656, "deadline_seconds": 900,
        "operation": "cold", "dpi": 72, "cells": [{"case": c, "mode": m} for c, m in CELLS],
        "binary_roles": list(ROLES), "same_absolute_binary_for_both_labels_within_stratum": True,
        "bindings_sha256": pins["bindings"]["sha256"], "schedule": schedule(),
        "journal": {"path": "events.jsonl", "maximum_event_bytes": 1 << 20, "maximum_bytes": 128 << 20}}
    require(all(plan.get(k) == v for k, v in expected_plan.items())
            and report["plan_sha256"] == pins["plan"]["sha256"], "fixed plan changed")
    runtime = plan["runtime"]
    require(runtime["time_wrapper"] == ["/usr/bin/time", "-l"]
            and runtime["platform"].startswith(("Darwin-", "macOS-"))
            and plan["rss"] == {"source": "/usr/bin/time -l", "units": "bytes", "scope": "one fresh renderer child high-water"},
            "wrong RSS platform/units")
    closure = plan["bound_files_sha256"]
    required = {pins["producer"]["path"], pins["bindings"]["path"], runtime["executable"], "/usr/bin/time", "/bin/ps"}
    required.update(str(root / p) for p in ("benchmarks/color_reuse_fixtures.py", "benchmarks/paired_render.py",
        "tools/benchkit/measurement.py", "tools/benchkit/process_group.py", "tools/benchkit/artifacts.py",
        "tools/benchkit/images.py", "tools/benchkit/config.py", "tools/benchkit/timings.py",
        "crates/pagura-render/examples/scene_index_profile.rs",
        "benchmarks/results/sources/analytic19-conversion/manifest.json",
        "benchmarks/results/sources/analytic19-conversion/references.json"))
    require(required <= set(closure) and closure[pins["producer"]["path"]] == pins["producer"]["sha256"],
            "missing recorded producer/helper/launcher runtime file")
    for path, digest in closure.items():
        require(Path(path).is_absolute() and file_sha(path) == digest, "loaded file drift: " + path)
    for relative in ("benchmarks/color_reuse_fixtures.py", "benchmarks/paired_render.py",
                     "tools/benchkit/measurement.py", "crates/pagura-render/examples/scene_index_profile.rs"):
        require(closure[str(root / relative)] == bindings["bound_files_sha256"][relative], "original helper mismatch")
    receipt = coordination(pins, root, directory, plan, report)
    sizes, hashes = {}, {}
    def artifact(name):
        raw = read(within(directory, name))
        sizes[name], hashes[name] = len(raw), sha(raw)
        return raw
    require(artifact("bindings.json") == bound(pins["bindings"])
            and sha(artifact("plan.json")) == pins["plan"]["sha256"], "retained binding/plan differs")
    start_raw = artifact("runner-start.json")
    start = decode(start_raw)
    require(sha(start_raw) == plan["runner_start_sha256"] and start["status"] == "running"
            and all(start[k] == receipt[k] for k in ("run_id", "kind", "source_root", "cwd", "source_before", "argv")),
            "runner-start snapshot differs from final receipt")
    build_pin = bindings["build_report"]
    build_owner = next(e for e in bindings["completed_evidence"] if e["artifacts"].get(build_pin["path"]) == build_pin["sha256"])
    for name, pin in (("historical-build.json", build_pin), ("historical-build-receipt.json", build_owner["receipt"]),
                      ("historical-build-guard.json", build_owner["guard"])):
        require(artifact(name) == bound(pin), "retained historical evidence differs")
    require(sha(artifact("references-manifest.json")) == MANIFEST_SHA
            and sha(artifact("references.json")) == REFERENCES_SHA, "retained reference descriptors differ")
    expected = references(root)
    require(set(plan["inputs"]) == set(expected), "input matrix changed")
    for name, image in expected.items():
        require(artifact("inputs/" + name + ".pdf") == image["pdf"]
                and artifact("oracles/" + name + ".ppm") == image["oracle"]
                and plan["inputs"][name] == {"pdf_sha256": sha(image["pdf"]), "oracle_sha256": sha(image["oracle"]),
                                            "width": image["width"], "height": image["height"]}, "input/reference mismatch")
    journal_raw = bound(pins["journal"])
    require(journal_raw.endswith(b"\n") and len(journal_raw) <= 128 << 20, "incomplete/unbounded journal")
    lines = journal_raw.splitlines()
    require(len(lines) == 16970 and all(len(line) + 1 <= 1 << 20 for line in lines), "journal event matrix differs")
    events = [decode(line) for line in lines]
    require(report["journal"] == {"path": "events.jsonl", "sha256": pins["journal"]["sha256"],
                                  "bytes": len(journal_raw), "events": len(events)}, "journal identity mismatch")
    cursor, last_time = 0, 0
    def event(kind, fields):
        nonlocal cursor, last_time
        item = events[cursor]
        require(item["event"] == cursor and item["type"] == kind and positive(item["monotonic_ns"])
                and item["monotonic_ns"] >= last_time
                and {k: v for k, v in item.items() if k not in ("event", "type", "monotonic_ns")} == fields,
                "journal sequence/payload mismatch at " + str(cursor))
        cursor, last_time = cursor + 1, item["monotonic_ns"]
        return last_time
    event("run-start", {"runner": report["runner"], "source_before": report["runner_source_before"],
                        "producer_argv": report["producer_argv"], "started_unix_ns": report["started_unix_ns"]})
    require(len(report["records"]) == 2828, "incomplete pair records")
    for index, (pair, planned) in enumerate(zip(report["records"], schedule())):
        require(pair["sequence"] == index and all(pair[k] == v for k, v in planned.items())
                and set(pair) == set(planned) | {"sequence", "started_monotonic_ns", "finished_monotonic_ns", "A", "B"},
                "pair schedule/fields differ")
        require(positive(pair["started_monotonic_ns"]) and pair["started_monotonic_ns"] >= last_time, "pair chronology differs")
        event("pair-start", {"pair_sequence": index, "scheduled": planned, "started_monotonic_ns": pair["started_monotonic_ns"]})
        require(last_time >= pair["started_monotonic_ns"], "pair start event precedes sample")
        for label in pair["order"]:
            record, image = pair[label], expected[pair["case"]]
            command = [identities[pair["binary_role"]]["binary"], str(directory / "inputs" / (pair["case"] + ".pdf")),
                       pair["mode"], "cold", str(directory / "pending.rgba"), "72"]
            require(record["verified"] is True and record["command"] == command
                    and not ({"error", "interrupted", "artifact_error"} & set(record))
                    and positive(record["started_monotonic_ns"]) and record["started_monotonic_ns"] >= last_time,
                    "failed or wrong same-binary request")
            event("arm-start", {"pair_sequence": index, "label": label, "command": command,
                                "started_monotonic_ns": record["started_monotonic_ns"]})
            require(record["started_monotonic_ns"] <= last_time <= record["finished_monotonic_ns"]
                    and positive(record["finished_monotonic_ns"]), "arm chronology differs")
            log_name = f"logs/{index:04d}-{label}.stdout-stderr.txt"
            raw = artifact(log_name)
            require(record["log"] == log_name and record["log_sha256"] == sha(raw)
                    and record["log_bytes"] == len(raw), "raw log identity mismatch")
            verify_log(raw, record, pair["mode"], image)
            blob = "blobs/" + sha(image["rgba"]) + ".rgba"
            require(record["blob"] == blob and record["output_sha256"] == sha(image["rgba"]), "independent output hash mismatch")
            if blob not in hashes:
                require(artifact(blob) == image["rgba"], "independent RGB/opaque alpha mismatch")
            event("arm-result", {"pair_sequence": index, "label": label, "record": record})
            require(last_time >= record["finished_monotonic_ns"], "arm result predates completion")
        require(positive(pair["finished_monotonic_ns"]) and pair["finished_monotonic_ns"] >= last_time,
                "pair ends before its arms")
        event("pair-finished", {"pair_sequence": index, "finished_monotonic_ns": pair["finished_monotonic_ns"]})
        require(last_time >= pair["finished_monotonic_ns"], "pair event predates completion")
    event("run-finished", {"status": report["status"], "finished_unix_ns": report["finished_unix_ns"],
                           "duration_seconds": report["duration_seconds"], "error": None})
    require(cursor == len(events) and sum(sizes.values()) == report["stored_bytes"] <= 128 << 20,
            "incomplete journal/retention accounting")
    inventory = {str(p.relative_to(directory)) for p in directory.rglob("*") if p.is_file()}
    require(inventory == set(sizes) | {"report.json", "events.jsonl"}, "untracked/pending diagnostic files")
    results = {}
    for role, name, mode in STRATA:
        pairs = [r for r in report["records"] if (r["binary_role"], r["case"], r["mode"]) == (role, name, mode)
                 and r["phase"] == "measured"]
        require(len(pairs) == 200 and sum(r["order"] == "AB" for r in pairs) == 100, "unbalanced/missing stratum")
        results[f"{role}:{name}:{mode}:cold"] = summary(pairs)
    require(results == report["summaries"], "descriptive RSS summaries do not reproduce")
    return {"schema": 1, "status": "verified-evidence", "diagnostic": report["diagnostic"],
            "auditor_revision": 2, "runtime_handoff_retrospective": True,
            "python_launch": pins["python_launch"],
            "admission": False, "thresholds_changed": False, "prior_failed_gates_unchanged": True,
            "verified_processes": 5656, "pairs": 2828, "journal_events": cursor,
            "run_id": receipt["run_id"], "summaries": results, "artifacts_sha256": hashes,
            "limitations": ["Descriptive same-binary RSS variation only; no performance or release decision.",
                "Frozen independent RGB references plus opaque alpha; no general PDF correctness claim.",
                "Cooperative recorded execution, not hostile-host attestation or guaranteed OS exclusivity.",
                "Whole-process child high-water, not render-only allocation or calibrated uncertainty.",
                "The macOS framework app executable was omitted from the prelaunch runtime closure and is pinned only post-run; current hashes and launcher source explain the handoff but do not prove historical app bytes.",
                "Historical source/build correspondence trusts pinned coordinator records; current source bytes are rechecked."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pins", type=Path, required=True)
    parser.add_argument("--pins-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    pins = decode(bound({"path": str(args.pins.resolve()), "sha256": args.pins_sha256}))
    result = audit(pins)
    result.update(pins_sha256=args.pins_sha256, auditor_sha256=file_sha(Path(__file__).resolve()),
                  evidence_pins={k: pins[k] for k in ("bindings", "plan", "report", "journal", "receipt", "guard", "producer")})
    raw = (json.dumps(result, indent=2, allow_nan=False) + "\n").encode()
    with args.output.resolve().open("xb") as handle:
        handle.write(raw)
    print(json.dumps({"status": result["status"], "verified_processes": result["verified_processes"],
                      "audit_sha256": sha(raw), "output": str(args.output.resolve())}))


if __name__ == "__main__":
    main()
