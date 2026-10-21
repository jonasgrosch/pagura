#!/usr/bin/env python3
"""Independent fixed start-to-current evidence audit; never release admission.

No collector/fixture imports, rendering or replacement samples. The coordinator
pins final evidence externally; historical wrong pixels never qualify timings.
"""
import argparse
from datetime import datetime
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path, PurePosixPath
import random
import re
import stat
import statistics
import subprocess
import sys

SIDES = ("baseline", "candidate")
MODES = ("exact", "competitive")
SEED, WARMUPS, PAIRS = 20261114, 5, 200
RESAMPLES, CONFIDENCE = 5000, 0.95
BASELINE = "676a872eb9f6c279cbf1e3929ace3a0c99db9fc3"
START = "359e19dda5b78a23675678b6a153da7ab7fdc2c5"
CANDIDATE = "a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c"
FILE_LIMIT, LOG_LIMIT, EVENT_LIMIT = 64 << 20, 256 << 10, 1 << 20
MANIFEST_SHA = "278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3"
REFERENCES_SHA = "924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf"
LEGACY_CASES = ("sparse64", "dense64", "tiny", "opaque64", "one-source64",
                "overcapacity128", "nested64", "default-knockout64")
OUTLINE_CASES = ("outline2", "outline2-knockout", "outline2-late64", "clip-churn", "image-only")
EXTRA_CASES = ("opaque-clip", "opaque-tiny", "opaque-fill")
ANALYTIC_CASES = (
    "classic/blank", "classic/rectangle", "classic/graphics-state",
    "classic/negative-rectangle", "classic/inherited-box", "classic/clipped-to-page",
    "classic/many-rectangles", "classic/content-array-split", "classic/many-operands",
    "paint/text-heavy-page", "paint/text-mode-4", "paint/type3-d0-colour",
    "paint/text-knockout-true", "paint/text-knockout-false", "paint/image-soft-mask",
    "paint/image-direct-binary-halves", "paint/image-direct-gray-quarter-turn",
    "paint/image-direct-rgb-fractional", "paint/image-direct-bilinear",
)
OPAQUE_CELLS = (
    *((case, mode, operation) for case in LEGACY_CASES for mode in MODES
      for operation in (("cold", "full", "tile", "tiled") if case == "sparse64"
                        else ("cold", "full") if case == "tiny" else ("full",))),
    *((case, mode, "full") for case in OUTLINE_CASES for mode in MODES),
    *(("opaque64", mode, operation) for mode in MODES for operation in ("cold", "tile", "tiled")),
    *((case, mode, "cold" if case == "opaque-tiny" else "full") for case in EXTRA_CASES for mode in MODES),
)
# Namespace spelling is finalized with the independently reviewed collector schema.
CELLS = (*(("opaque", *cell) for cell in OPAQUE_CELLS),
         *(("analytic19", case, mode, "cold") for case in ANALYTIC_CASES for mode in MODES))


def require(value, message):
    if not value:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def positive(value):
    return type(value) is int and 0 < value < 1 << 64


def read(path, maximum=FILE_LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        metadata = os.fstat(fd)
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= maximum,
                "nonregular/oversized evidence: " + str(path))
        with os.fdopen(fd, "rb", closefd=False) as handle:
            raw = handle.read(maximum + 1)
    finally:
        os.close(fd)
    require(len(raw) <= maximum, "evidence grew beyond allowance")
    return raw


def bound(pin, maximum=FILE_LIMIT):
    require(type(pin) is dict and set(pin) == {"path", "sha256"}
            and type(pin["path"]) is str and Path(pin["path"]).is_absolute()
            and type(pin["sha256"]) is str and re.fullmatch(r"[0-9a-f]{64}", pin["sha256"]),
            "invalid external file pin")
    raw = read(pin["path"], maximum)
    require(sha(raw) == pin["sha256"], "pin mismatch: " + pin["path"])
    return raw


def within(root, relative):
    require(type(relative) is str and relative and "\\" not in relative
            and not PurePosixPath(relative).is_absolute()
            and all(part not in ("", ".", "..") for part in relative.split("/")),
            "invalid relative artifact")
    path = (root / relative).resolve(strict=True)
    require(path.is_relative_to(root.resolve()), "artifact escaped evidence root")
    return path


def source(root, runner=False):
    root = Path(root).resolve(strict=True)
    paths = set()
    for directory in ("crates", "tools", "benchmarks/adapters"):
        paths.update(path for path in (root / directory).rglob("*") if path.is_file()
                     and "target" not in path.parts and "__pycache__" not in path.parts)
    paths.update((root / "corpus/manifests").glob("*.json"))
    names = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"]
    if runner:
        names += ["benchmarks/release-matrix.json", "benchmarks/requirements.txt",
                  "benchmarks/node-package.json", "benchmarks/node-package-lock.json"]
    paths.update(root / name for name in names if (root / name).exists())
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + read(path))
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    return {"root": str(root), "commit": commit, "source_sha256": digest.hexdigest()}


def ppm(raw, operation):
    match = re.match(rb"P6\n([1-9][0-9]*) ([1-9][0-9]*)\n255\n", raw)
    require(match is not None, "expected frozen canonical P6 oracle")
    width, height = map(int, match.groups())
    pixels = raw[match.end():]
    require(len(pixels) == width * height * 3, "oracle payload mismatch")
    if operation == "tile":
        w, h = min(width, 256), min(height, 256)
        pixels = b"".join(pixels[y * width * 3:(y * width + w) * 3] for y in range(h))
        width, height = w, h
    return width, height, pixels


def quality(raw, expected):
    width, height, target = expected
    result = {"status": "missing-output", "dimensions_equal": False,
              "opaque_alpha_equal": False, "rgb_equal": False,
              "different_channels": None, "max_channel_error": None}
    if raw is None:
        return result
    match = re.match(rb"RGBA8 ([1-9][0-9]*) ([1-9][0-9]*) premultiplied\n", raw)
    if match is None:
        return {**result, "status": "invalid-format"}
    w, h = map(int, match.groups())
    pixels = raw[match.end():]
    if (w, h) != (width, height) or len(pixels) != w * h * 4:
        return {**result, "status": "wrong-dimensions-or-length"}
    alpha = pixels[3::4] == b"\xff" * (w * h)
    if alpha and all(pixels[c::4] == target[c::3] for c in range(3)):
        return {"status": "correct", "dimensions_equal": True, "opaque_alpha_equal": True,
                "rgb_equal": True, "different_channels": 0, "max_channel_error": 0}
    differences, maximum = 0, 0
    for channel in range(3):
        for actual, wanted in zip(pixels[channel::4], target[channel::3]):
            delta = abs(actual - wanted)
            differences += delta != 0
            maximum = max(maximum, delta)
    return {"status": "correct" if alpha and differences == 0 else "incorrect",
            "dimensions_equal": True, "opaque_alpha_equal": alpha,
            "rgb_equal": differences == 0, "different_channels": differences,
            "max_channel_error": maximum}


def distribution(values):
    ordered = sorted(values)
    if not ordered:
        return {"count": 0}
    def quantile(p):
        position = (len(ordered) - 1) * p
        lo, hi = math.floor(position), math.ceil(position)
        return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)
    return {"count": len(ordered), "minimum": ordered[0], "q1": quantile(0.25),
            "median": statistics.median(ordered), "q3": quantile(0.75),
            "p95": quantile(0.95), "maximum": ordered[-1]}


def statistics_module(pin):
    bound(pin)
    spec = importlib.util.spec_from_file_location("pinned_start_current_statistics", pin["path"])
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def paired_metric(rows, getter, module, bootstrap):
    values = {side: [getter(row[side]) for row in rows] for side in SIDES}
    if len(rows) != PAIRS or not all(positive(v) for group in values.values() for v in group):
        return {"status": "unavailable-incomplete-pairs", "required_pairs": PAIRS,
                "available_values": {side: sum(positive(v) for v in group) for side, group in values.items()}}
    base, current = values["baseline"], values["candidate"]
    result = {"status": "descriptive-all-pairs", "pairs": PAIRS,
              "baseline": distribution(base), "candidate": distribution(current),
              "median_ratio": statistics.median(current) / statistics.median(base),
              "p95_ratio": distribution(current)["p95"] / distribution(base)["p95"],
              "maximum_ratio": max(current) / max(base),
              "paired_candidate_minus_baseline": distribution([c - b for b, c in zip(base, current)])}
    if bootstrap:
        result["paired_bootstrap"] = module.ratio_bounds(current, base, RESAMPLES, CONFIDENCE, SEED)
    return result


def expected_schedule(phase):
    if phase == "preflight":
        return [{"sequence": i, "phase": "preflight", "pair": 0, "cell_index": i,
                 **dict(zip(("suite", "case", "mode", "operation"), cell)), "order": "BC"}
                for i, cell in enumerate(CELLS)]
    require(phase == "confirmation", "unknown phase")
    rng, result = random.Random(SEED), []
    for section, count in (("warmup", WARMUPS), ("measured", PAIRS)):
        orders = {}
        for index in range(len(CELLS)):
            values = ["BC"] * ((count + 1) // 2) + ["CB"] * (count // 2)
            rng.shuffle(values)
            orders[index] = values
        for pair in range(count):
            indices = list(range(len(CELLS)))
            rng.shuffle(indices)
            for index in indices:
                result.append({"sequence": len(result), "phase": section, "pair": pair,
                               "cell_index": index,
                               **dict(zip(("suite", "case", "mode", "operation"), CELLS[index])),
                               "order": orders[index][pair]})
    return result


def cells_json():
    return [dict(zip(("suite", "case", "mode", "operation"), cell)) for cell in CELLS]


def json_bound(pin, maximum=FILE_LIMIT):
    return json.loads(bound(pin, maximum))


def descriptor(record, root=None):
    require(type(record) is dict and set(record) == {"path", "sha256", "bytes"}
            and type(record["bytes"]) is int and 0 <= record["bytes"] <= FILE_LIMIT,
            "invalid retained descriptor")
    path = within(root, record["path"]) if root else Path(record["path"])
    require(path.is_absolute(), "external descriptor must be absolute")
    raw = bound({"path": str(path), "sha256": record["sha256"]})
    require(len(raw) == record["bytes"], "retained descriptor size mismatch")
    return raw


def frozen_inputs(pin):
    manifest = json_bound(pin)
    require(manifest["schema"] == 1 and manifest["experiment"] == "start-current"
            and manifest["cells"] == cells_json(), "frozen input matrix changed")
    inputs = manifest["inputs"]
    expected = {suite + ":" + case for suite, case, _, _ in CELLS}
    require(set(inputs) == expected and len(inputs) == 35, "frozen input set changed")
    for key, item in inputs.items():
        raw = descriptor(item["pdf"])
        require(raw.startswith(b"%PDF-"), "frozen input is not a PDF")
        width, height, _ = ppm(descriptor(item["oracle"]), "full")
        require((item["width"], item["height"]) == (width, height), "input dimensions differ")
    closure = manifest["source_files_sha256"]
    require(type(closure) is dict and closure, "missing independent fixture recipe pins")
    for path, digest in closure.items():
        bound({"path": path, "sha256": digest})
    # Analytic19 has earlier external frozen identities in addition to this
    # experiment's pre-run manifest. Check that none of its oracles was replaced.
    old_manifest = [path for path in closure if path.endswith("/analytic19-conversion/manifest.json")]
    old_references = [path for path in closure if path.endswith("/analytic19-conversion/references.json")]
    require(len(old_manifest) == len(old_references) == 1, "missing original analytic reference pins")
    documents = json_bound({"path": old_manifest[0], "sha256": MANIFEST_SHA})["documents"]
    references = json_bound({"path": old_references[0], "sha256": REFERENCES_SHA})["cases"]
    require([d["id"] for d in documents] == [r["id"] for r in references] == list(ANALYTIC_CASES),
            "original analytic matrix changed")
    for document, reference in zip(documents, references):
        require(document["pages"] == 1 and document["selected_pages"] == [0]
                and len(document["oracles"]) == 1, "original page contract changed")
        oracle = document["oracles"][0]
        require((oracle["page"], oracle["dpi"], oracle["tolerance"]) == (0, 72, 0)
                and reference["expected"] == "render" and reference["dpi"] == 72,
                "original independent settings changed")
        item = inputs["analytic19:" + document["id"]]
        require(item["pdf"]["sha256"] == document["sha256"] == reference["sha256"]
                and item["oracle"]["sha256"] == oracle["sha256"] == reference["oracle_sha256"],
                "analytic PDF/oracle replaced")
    return manifest


def completed(receipt, expected_exit=0):
    require(receipt["schema"] == 1 and receipt["status"] == ("completed" if expected_exit == 0 else "failed")
            and receipt["runner"]["system"] == "Darwin"
            and type(receipt["exit_code"]) is int and receipt["exit_code"] == expected_exit
            and type(receipt["wrapper_exit_code"]) is int and receipt["wrapper_exit_code"] == expected_exit
            and receipt["cancellation_signal"] is None and receipt["errors"] == []
            and receipt["artifact_errors"] == {} and receipt["source_unchanged"] is True
            and receipt["source_before"] == receipt["source_after"], "nonterminal/failed outer receipt")
    times = [datetime.fromisoformat(receipt[key]) for key in ("started_at", "ended_at", "finalized_at")]
    require(all(t.tzinfo for t in times) and times == sorted(times), "runner times unordered")
    require(receipt["coordination"]["scope"] == "cooperative"
            and Path(receipt["coordination"]["lock_file"]).resolve() == Path("/tmp/pagura-bench.lock").resolve()
            and Path(receipt["coordination"]["lock_directory"]).resolve() == Path("/tmp/pagura-bench.lockdir").resolve(),
            "wrong cooperative coordination")


def verify_bindings(pin, root):
    bindings = json_bound(pin)
    require(bindings["schema"] == 1 and bindings["experiment"] == "start-current"
            and set(bindings["identities"]) == set(SIDES), "wrong start/current binding")
    identities = bindings["identities"]
    require(identities["baseline"]["commit"] == BASELINE
            and identities["candidate"]["commit"] == CANDIDATE
            and identities["candidate"]["root"] == str(root), "wrong fixed source arms")
    for side, identity in identities.items():
        require(source(identity["root"]) == {k: identity[k] for k in ("root", "commit", "source_sha256")},
                side + " source changed")
        bound({"path": identity["binary"], "sha256": identity["binary_sha256"]})
    old_root = Path(identities["baseline"]["root"])
    shared = ("crates/pagura-render/examples/scene_index_profile.rs", "rust-toolchain.toml",
              ".cargo/config.toml", "Cargo.toml", "Cargo.lock")
    for relative in shared:
        require(read(root / relative) == read(old_root / relative), "build/example configuration differs")
    # The older checkout's only intentional overlay is the common example.
    status = subprocess.check_output(["git", "-C", str(old_root), "status", "--porcelain=v1",
                                      "--untracked-files=all"], text=True).strip()
    require(status == "?? crates/pagura-render/examples/scene_index_profile.rs", "unexpected baseline overlay")
    paths = ("crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml")
    result = subprocess.run(["git", "-C", str(old_root), "diff", "--quiet", START, BASELINE, "--", *paths], check=False)
    require(result.returncode == 0, "control product differs from session start")
    for directory in (old_root, root):
        result = subprocess.run(["git", "-C", str(directory), "diff", "--quiet", "HEAD", "--", *paths], check=False)
        require(result.returncode == 0, "tracked product changed after frozen commit")
    runner = source(root, runner=True)
    require(bindings["candidate_runner_source_sha256"] == runner["source_sha256"], "runner source drift")
    for relative, digest in bindings["bound_files_sha256"].items():
        bound({"path": str(within(root, relative)), "sha256": digest})
    build = json_bound(bindings["build_report"])
    require(build["status"] == "built-both-arms" and build["identities"] == identities
            and build["session_start"] == START and set(build["builds"]) == set(SIDES), "build identities differ")
    require(build["build_environment"] == {"CARGO_BUILD_JOBS": "1", "CARGO_BUILD_TARGET": None,
            "RUSTFLAGS": None, "CARGO_ENCODED_RUSTFLAGS": None, "RUSTC_WRAPPER": None,
            "RUSTC_WORKSPACE_WRAPPER": None, "CARGO_PROFILE_RELEASE_DEBUG": None,
            "CARGO_PROFILE_RELEASE_LTO": None}, "build environment changed")
    require(type(build["toolchain"]["rustc"]) is str and type(build["toolchain"]["cargo"]) is str
            and build["toolchain"]["rustc"] and build["toolchain"]["cargo"], "missing build toolchain identity")
    for path, digest in build["config_sha256"].items():
        bound({"path": path, "sha256": digest})
    build_driver = root / "benchmarks/results/sources/start-current/build.py"
    require(sha(read(build_driver)) == build["driver_sha256"], "build driver changed")
    for side, identity in identities.items():
        directory, item = Path(identity["root"]), build["builds"][side]
        requested = ["cargo", "build", "--locked", "--offline", "--release", "-p", "pagura-render",
                     "--example", "scene_index_profile", "--manifest-path", str(directory / "Cargo.toml"),
                     "--target-dir", str(directory / "target"), "--message-format=json", "-j", "1"]
        require(item["command"] == requested and type(item["process"]["exit_code"]) is int
                and item["process"]["exit_code"] == 0 and item["process"]["status"] == "failed"
                and item["source_before"] == item["source_after"]
                == {k: identity[k] for k in ("root", "commit", "source_sha256")}, "failed or drifting build")
        raw = bound({"path": str(Path(bindings["build_report"]["path"]).parent / (side + ".log")),
                     "sha256": item["log_sha256"]}, 1 << 20)
        artifacts = [json.loads(line) for line in raw.splitlines() if line.startswith(b"{")]
        artifact = item["cargo_artifact"]
        require(artifact in artifacts and artifact["reason"] == "compiler-artifact"
                and artifact["target"]["name"] == "scene_index_profile"
                and artifact["target"]["kind"] == ["example"] and artifact["features"] == []
                and artifact["profile"]["opt_level"] == "3" and artifact["profile"]["test"] is False
                and artifact["executable"] == str(directory / "target/release/examples/scene_index_profile"),
                "wrong Cargo native artifact")
    owners = []
    for evidence in bindings["completed_evidence"]:
        receipt, guard = json_bound(evidence["receipt"]), json_bound(evidence["guard"])
        completed(receipt)
        require(receipt["artifacts"] == evidence["artifacts"]
                and receipt["artifacts"].get(evidence["guard"]["path"]) == evidence["guard"]["sha256"],
                "completed evidence artifact binding differs")
        require(guard["status"] == "completed" and guard["observation_only"] is False
                and guard["exit_code"] == guard["child_exit_code"] == 0
                and [event["stage"] for event in guard["events"]] == ["preflight", "postflight"]
                and all(event["matches"] == [] for event in guard["events"]), "build/evidence guard did not complete cleanly")
        for path, digest in evidence["artifacts"].items():
            bound({"path": path, "sha256": digest})
        if receipt["artifacts"].get(bindings["build_report"]["path"]) == bindings["build_report"]["sha256"]:
            owners.append(receipt)
            require(receipt["run_id"] == build["run_id"]
                    and receipt["source_before"] == {"commit": runner["commit"], "sha256": runner["source_sha256"]}
                    and guard["command"] == ["python3", "-B", str(build_driver.relative_to(root))]
                    and receipt["argv"][6:] == guard["command"], "build owner launch/source differs")
    require(len(owners) == 1, "missing/ambiguous completed build owner")
    return bindings, runner


def strict_guard(guard, expected_exit):
    require(guard["schema"] == 1 and guard["observation_only"] is False
            and guard["status"] == ("completed" if expected_exit == 0 else "child-failed")
            and all(type(guard[key]) is int and guard[key] == expected_exit
                    for key in ("exit_code", "child_exit_code"))
            and guard.get("cancellation_requested_utc") is None
            and type(guard["events"]) is list and len(guard["events"]) >= 2
            and guard["events"][0]["stage"] == "preflight"
            and guard["events"][-1]["stage"] == "postflight"
            and all(event["matches"] == [] for event in guard["events"])
            and type(guard["elapsed_seconds"]) in (int, float)
            and 0 < guard["elapsed_seconds"] < guard["window_seconds"],
            "contention/deadline/cancellation guard did not complete cleanly")


def python_launch(requested, observed, closure, runtime):
    """macOS framework Python can replace argv[0]; both bytes must be pinned."""
    require(type(requested) is list and type(observed) is list and len(requested) >= 3
            and requested[1:] == observed[1:] and requested[1] == "-B"
            and all(type(arg) is str for arg in requested + observed), "producer launch arguments differ")
    paths = [Path(arg).resolve(strict=True) for arg in (requested[0], observed[0], runtime["executable"], runtime["launcher"])]
    require(all(Path(arg).is_absolute() for arg in (requested[0], observed[0]))
            and all(str(path) in closure for path in paths), "Python launcher/app not prebound")
    # Permit only the already recorded app/launcher pair, not arbitrary Python.
    require(paths[0] in paths[2:] and paths[1] in paths[2:], "unexpected Python handoff")
    for path in paths:
        bound({"path": str(path), "sha256": closure[str(path)]})


def verify_terminal(pins, plan, report, runner, expected_exit):
    receipt, guard = json_bound(pins["receipt"]), json_bound(pins["guard"])
    completed(receipt, expected_exit)
    strict_guard(guard, expected_exit)
    root = Path(pins["root"]).resolve(strict=True)
    require(receipt["kind"] == "benchmark" and receipt["cwd"] == receipt["source_root"] == str(root)
            and receipt["source_before"] == {"commit": runner["commit"], "sha256": runner["source_sha256"]}
            and receipt["run_id"] == report["runner"]["run_id"]
            and Path(report["runner"]["receipt"]).resolve() == Path(pins["receipt"]["path"]).resolve(),
            "final run identity/source differs")
    for key in ("report", "plan", "journal", "guard"):
        require(receipt["artifacts"].get(pins[key]["path"]) == pins[key]["sha256"],
                "receipt does not bind final " + key)
    guard_path = Path(pins["guard"]["path"]).resolve(strict=True)
    guard_script = Path(pins["guard_script"]["path"]).resolve(strict=True)
    bound(pins["guard_script"])
    argv = receipt["argv"]
    require(len(argv) >= 9 and argv[1] == "-B"
            and (root / argv[2]).resolve() == guard_script
            and argv[3] == "--evidence-dir" and (root / argv[4]).resolve() == guard_path.parent
            and argv[5] == "--"
            and argv[6:] == guard["command"], "outer receipt does not launch this guard and collector")
    python_launch(guard["command"], report["producer_argv"], plan["bound_files_sha256"], plan["runtime"])
    expected_command = ["-B", report["producer_argv"][2], "--root", str(root),
                        "--bindings", pins["bindings"]["path"], "--bindings-sha256", pins["bindings"]["sha256"],
                        "--phase", pins["phase"], "--output", str(Path(pins["report"]["path"]).parent)]
    if pins["phase"] == "confirmation":
        prerequisite = pins["prerequisites"]
        expected_command += ["--preflight-pins", prerequisite["path"], "--preflight-pins-sha256", prerequisite["sha256"]]
    require(report["producer_argv"][1:] == expected_command, "unexpected producer CLI/settings")
    script = Path(report["producer_argv"][2]).resolve(strict=True)
    require(script.name == "collect.py" and str(script) in plan["bound_files_sha256"]
            and str(script.with_name("protocol.py")) in plan["bound_files_sha256"], "collector closure missing")
    start = json.loads(descriptor(plan["runner_start"], Path(pins["report"]["path"]).parent))
    require(start["status"] == "running" and start["run_id"] == receipt["run_id"]
            and all(start[key] == receipt[key] for key in
                    ("argv", "cwd", "source_root", "source_before", "runner", "coordination"))
            and plan["runner_source_before"] == report["runner_source_before"] == start["source_before"],
            "running-to-final receipt chain differs")
    snapshots = {str(Path(pin["path"]).resolve()): pin for pin in pins["guard_snapshots"]}
    expected_snapshots = {str(within(guard_path.parent, event["snapshot"])) for event in guard["events"]}
    require(set(snapshots) == expected_snapshots, "guard snapshots missing or extra")
    for pin in snapshots.values():
        require(bound(pin), "empty guard snapshot")
    return receipt, guard


def verify_prerequisites(pin, plan, bindings, bindings_sha):
    require(type(pin) is dict and set(pin) == {"path", "sha256"}, "confirmation preflight pin absent")
    prerequisites = json_bound(pin)
    require(prerequisites == plan["prerequisites"], "embedded prerequisites differ from external pins")
    require(prerequisites["schema"] == 1 and prerequisites["experiment"] == "start-current"
            and prerequisites["bindings_sha256"] == bindings_sha, "prerequisite binding differs")
    prior_records = None
    for name in ("preflight", "accuracy"):
        evidence = prerequisites[name]
        previous = json_bound(evidence["report"], 256 << 20)
        audit = json_bound(evidence["audit"], 256 << 20)
        receipt, guard = json_bound(evidence["receipt"]), json_bound(evidence["guard"])
        completed(receipt)
        strict_guard(guard, 0)
        required = ("report", "guard", "plan", "journal") if name == "preflight" else ("report", "guard")
        for key in required:
            bound(evidence[key], 256 << 20)
            require(receipt["artifacts"].get(evidence[key]["path"]) == evidence[key]["sha256"],
                    "prerequisite final receipt binding differs")
        require(audit["report_sha256"] == evidence["report"]["sha256"], "prerequisite audit/report differs")
        if name == "preflight":
            require(audit["status"] == "verified-preflight" and audit["candidate_preflight_correct"] is True
                    and audit["bindings_sha256"] == bindings_sha
                    and previous["status"] == "completed-preflight" and previous["timing_launch_allowed"] is True
                    and previous["identities"] == bindings["identities"]
                    and previous["bindings_sha256"] == bindings_sha and len(previous["records"]) == len(CELLS),
                    "timing prerequisite did not pass candidate preflight")
            prior_records = previous["records"]
        else:
            require(audit["status"] == "verified-accuracy-probes" and audit["candidate_quality_verified"] is True,
                    "independent accuracy probes failed")
            for side in SIDES:
                require(all(previous["identities"][side][key] == bindings["identities"][side][key]
                            for key in ("root", "commit", "source_sha256")), "accuracy-probe source differs")
    return prior_records


def verify_journal(raw, report, schedule):
    """Check every flushed event against the final report, including warmups."""
    lines = iter(raw.splitlines())
    counter, previous_time = 0, 0
    def take(kind, expected):
        nonlocal counter, previous_time
        line = next(lines, None)
        require(line is not None and len(line) <= EVENT_LIMIT, "missing/oversized journal event")
        event = json.loads(line)
        require(event.pop("event") == counter and event.pop("type") == kind, "journal event order differs")
        stamp = event.pop("monotonic_ns")
        require(positive(stamp) and stamp >= previous_time, "journal clock reversed")
        require(event == expected, "journal event disagrees with report: " + kind)
        counter, previous_time = counter + 1, stamp
    take("run-start", {"runner": report["runner"], "source_before": report["runner_source_before"],
                       "producer_argv": report["producer_argv"], "started_unix_ns": report["started_unix_ns"]})
    for row, expected in zip(report["records"], schedule):
        index = expected["sequence"]
        planned = {key: value for key, value in expected.items() if key != "sequence"}
        take("pair-start", {"pair_sequence": index, "scheduled": planned,
                            "started_monotonic_ns": row["started_monotonic_ns"]})
        for label in expected["order"]:
            arm = row[label]
            take("arm-start", {"pair_sequence": index, "label": label, "command": arm["command"],
                               "started_monotonic_ns": arm["started_monotonic_ns"]})
            take("arm-result", {"pair_sequence": index, "label": label, "record": arm})
        take("pair-finished", {"pair_sequence": index, "classification": row["classification"],
                               "raw_equal": row["raw_equal"], "finished_monotonic_ns": row["finished_monotonic_ns"]})
    take("run-finished", {"status": report["status"], "finished_unix_ns": report["finished_unix_ns"],
                          "duration_ns": report["duration_ns"], "error": report.get("error")})
    require(next(lines, None) is None and counter == report["journal"]["events"] == 2 + 6 * len(schedule),
            "journal extra/missing attempts")


def verify_arm(arm, row, side, inputs, expected, directory, identities, cache):
    key, operation = row["suite"] + ":" + row["case"], row["operation"]
    command = [identities[side]["binary"], str(directory / inputs[key]["pdf"]["path"]),
               row["mode"], operation, str(directory / "pending.rgba"), "72"]
    require(arm["command"] == command and not any(k in arm for k in ("error", "interrupted", "artifact_error")),
            "wrong invocation or interrupted attempt")
    require(positive(arm["started_monotonic_ns"]) and positive(arm["finished_monotonic_ns"])
            and arm["started_monotonic_ns"] <= arm["finished_monotonic_ns"], "unordered attempt")
    process = arm["process"]
    log = descriptor(arm["log"], directory)
    require(len(log) < LOG_LIMIT, "truncated combined process log")
    diagnostic = log.decode("ascii", errors="strict")
    rss = re.findall(r"^\s*(\d+)\s+maximum resident set size\s*$", diagnostic, re.MULTILINE)
    require(len(rss) == 1 and re.findall(r"(\d+)\s+maximum resident set size", diagnostic) == rss
            and positive(process["peak_rss_bytes"]) and int(rss[0]) == process["peak_rss_bytes"] == arm["rss_bytes"]
            and process["rss_status"] == "available" and positive(process["elapsed_ns"])
            and type(process["exit_code"]) is int and process["status"] in ("failed", "timeout"),
            "invalid native process/RSS evidence")
    success = process["exit_code"] == 0 and process["status"] != "timeout"
    require(arm["process_success"] is success, "process-success classification differs")
    wanted = expected[(key, operation)]
    raw = descriptor(arm["output"], directory) if "output" in arm else None
    cache_key = (sha(raw) if raw is not None else None, inputs[key]["oracle"]["sha256"], operation)
    if cache_key not in cache:
        cache[cache_key] = quality(raw, wanted)
    checked = cache[cache_key]
    reported = arm["quality"]
    require(reported["correct"] is (checked["status"] == "correct"), "producer quality disagrees with independent bytes")
    if raw is None:
        require(reported == {"correct": False, "reason": "missing-output"}, "missing output classification differs")
    else:
        require(reported["expected_width"] == wanted[0] and reported["expected_height"] == wanted[1]
                and reported["format_valid"] is checked["dimensions_equal"], "output dimensions classification differs")
        if checked["dimensions_equal"]:
            require(reported["rgb_differing_channels"] == checked["different_channels"]
                    and reported["rgb_maximum_error"] == checked["max_channel_error"]
                    and (reported["nonopaque_pixels"] == 0) is checked["opaque_alpha_equal"],
                    "reported pixel counts differ")
    stages = arm.get("stages")
    if success:
        records = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
        require(records == [stages] and stages["schema"] == 1 and stages["accuracy"] == row["mode"]
                and stages["operation"] == operation and type(stages["dpi"]) is int and stages["dpi"] == 72
                and (stages["width"], stages["height"]) == wanted[:2]
                and all(positive(stages[k]) for k in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
                and stages["metric_ns"] == stages["total_ns" if operation == "cold" else "render_ns"]
                and stages["total_ns"] >= sum(stages[k] for k in ("open_ns", "compile_ns", "render_ns")),
                "stage identity/timing differs")
    else:
        require(stages is None, "failed process cannot provide qualified stage timing")
    return {"process_success": success, "quality": checked, "process": process, "stages": stages,
            "output_sha256": sha(raw) if raw is not None else None}


def pair_classification(base, current):
    if not base["process_success"] or not current["process_success"]:
        return "process-failure"
    b, c = (arm["quality"]["status"] == "correct" for arm in (base, current))
    if b and c:
        return "both-correct-equal" if base["output_sha256"] == current["output_sha256"] else "both-correct-bytes-differ"
    return ("baseline-incorrect-current-correct" if c else
            "baseline-correct-current-incorrect" if b else "neither-correct")


def verify_run(pins, bindings, runner):
    directory = Path(pins["report"]["path"]).parent.resolve(strict=True)
    require(Path(pins["plan"]["path"]).resolve() == directory / "plan.json"
            and Path(pins["journal"]["path"]).resolve() == directory / "events.jsonl"
            and Path(pins["report"]["path"]).name == "report.json", "unexpected evidence paths")
    report = json_bound(pins["report"], 256 << 20)
    plan = json_bound(pins["plan"])
    schedule = expected_schedule(pins["phase"])
    expected_attempts = 2 * len(schedule)
    require(report["schema"] == plan["schema"] == 1 and report["experiment"] == plan["experiment"] == "start-current"
            and report["phase"] == plan["phase"] == pins["phase"]
            and report["status"] == "completed-" + pins["phase"] and report["admission"] is False
            and report["bindings_sha256"] == plan["bindings_sha256"] == pins["bindings"]["sha256"]
            and report["identities"] == plan["identities"] == bindings["identities"]
            and report["plan_sha256"] == pins["plan"]["sha256"]
            and plan["seed"] == SEED and plan["warmup_pairs_per_cell"] == WARMUPS
            and plan["measured_pairs_per_cell"] == PAIRS and plan["cells"] == cells_json()
            and plan["expected_attempts"] == report["attempted_processes"] == expected_attempts
            and plan["schedule"] == [{k: v for k, v in row.items() if k != "sequence"} for row in schedule]
            and len(report["records"]) == len(schedule) and report["producer_argv"] == plan["producer_argv"]
            and report.get("error") is None and report.get("journal_error") is None,
            "incomplete/changed experiment plan or final report")
    require(plan["statistics"] == {"performed_by": "independent post-completion auditor",
            "resamples": RESAMPLES, "one_sided_confidence": CONFIDENCE, "seed": SEED,
            "max_rss_ratio": "descriptive; no confidence interval"}
            and plan["limits"] == {"artifact_bytes": FILE_LIMIT, "retained_bytes": 1 << 30,
                "log_bytes": LOG_LIMIT, "child_timeout_seconds": 120}, "statistics/resource plan differs")
    for path, digest in plan["bound_files_sha256"].items():
        bound({"path": path, "sha256": digest})
    root = Path(pins["root"]).resolve()
    for relative, digest in bindings["bound_files_sha256"].items():
        path = str((root / relative).resolve())
        if path in plan["bound_files_sha256"]:
            require(plan["bound_files_sha256"][path] == digest, "loaded helper identity differs")
    frozen = frozen_inputs(bindings["inputs"])
    require(plan["inputs_manifest_sha256"] == bindings["inputs"]["sha256"]
            and plan["fixture_generation_sources_sha256"] == frozen["source_files_sha256"]
            and set(plan["inputs"]) == set(frozen["inputs"])
            and plan["example_sha256"] == sha(read(root / "crates/pagura-render/examples/scene_index_profile.rs")),
            "frozen input/example identity differs")
    expected, retained = {}, {"plan.json", "runner-start.json", "bindings.json", "build.json",
                              "build-receipt.json", "build-guard.json", "inputs-manifest.json"}
    for key, item in plan["inputs"].items():
        original = frozen["inputs"][key]
        require((item["width"], item["height"]) == (original["width"], original["height"]), "retained input dimensions differ")
        for kind in ("pdf", "oracle"):
            raw = descriptor(item[kind], directory)
            require(item[kind]["sha256"] == original[kind]["sha256"] and item[kind]["bytes"] == original[kind]["bytes"],
                    "retained input/oracle bytes changed")
            retained.add(item[kind]["path"])
        for operation in {cell[3] for cell in CELLS if cell[0] + ":" + cell[1] == key}:
            expected[(key, operation)] = ppm(descriptor(item["oracle"], directory), operation)
    require(read(directory / "bindings.json") == bound(pins["bindings"])
            and read(directory / "inputs-manifest.json") == bound(bindings["inputs"])
            and read(directory / "build.json") == bound(bindings["build_report"]), "retained prerequisite copy differs")
    owner = [e for e in bindings["completed_evidence"]
             if e["artifacts"].get(bindings["build_report"]["path"]) == bindings["build_report"]["sha256"]][0]
    require(read(directory / "build-receipt.json") == bound(owner["receipt"])
            and read(directory / "build-guard.json") == bound(owner["guard"]), "retained build receipt copy differs")
    prior = None
    if pins["phase"] == "confirmation":
        prior = verify_prerequisites(pins["prerequisites"], plan, bindings, pins["bindings"]["sha256"])
        require(read(directory / "preflight-pins.json") == bound(pins["prerequisites"]), "retained preflight pins differ")
        retained.add("preflight-pins.json")
    else:
        require(plan["prerequisites"] is None, "preflight unexpectedly depends on earlier measurement")
    journal_raw = bound(pins["journal"], 256 << 20)
    require(report["journal"]["sha256"] == pins["journal"]["sha256"]
            and report["journal"]["path"] == "events.jsonl"
            and report["journal"]["bytes"] == len(journal_raw), "journal binding differs")
    verify_journal(journal_raw, report, schedule)
    checked_rows, cache = [], {}
    previous_finished = 0
    for row, planned in zip(report["records"], schedule):
        require(all(row[k] == v for k, v in planned.items()), "attempt omitted/reordered/renamed")
        require(positive(row["started_monotonic_ns"]) and positive(row["finished_monotonic_ns"])
                and previous_finished <= row["started_monotonic_ns"] <= row["finished_monotonic_ns"], "pair chronology differs")
        previous_finished = row["finished_monotonic_ns"]
        checked = {**planned}
        for label, side in (("B", "baseline"), ("C", "candidate")):
            checked[side] = verify_arm(row[label], row, side, plan["inputs"], expected, directory, bindings["identities"], cache)
            require(row[label]["log"]["path"] == f"logs/{planned['sequence']:05d}-{label}.stdout-stderr.txt", "log sequence binding differs")
            retained.add(row[label]["log"]["path"])
            if "output" in row[label]:
                require(row[label]["output"]["path"] == "blobs/" + row[label]["output"]["sha256"] + ".rgba", "raw blob naming differs")
                retained.add(row[label]["output"]["path"])
        first, second = (row[label] for label in row["order"])
        require(row["started_monotonic_ns"] <= first["started_monotonic_ns"] <= first["finished_monotonic_ns"]
                <= second["started_monotonic_ns"] <= second["finished_monotonic_ns"] <= row["finished_monotonic_ns"],
                "actual arm order differs")
        checked["classification"] = pair_classification(checked["baseline"], checked["candidate"])
        hashes = [checked[side]["output_sha256"] for side in SIDES]
        checked["raw_equal"] = hashes[0] == hashes[1] if all(h is not None for h in hashes) else None
        require(row["classification"] == checked["classification"] and row["raw_equal"] is checked["raw_equal"],
                "pair quality classification differs")
        checked_rows.append(checked)
    candidate_ok = all(row["candidate"]["process_success"] and row["candidate"]["quality"]["status"] == "correct"
                       for row in checked_rows)
    require(report["candidate_quality_correct"] is candidate_ok
            and report["timing_launch_allowed"] is (candidate_ok and pins["phase"] == "preflight"),
            "candidate completion classification differs")
    verify_terminal(pins, plan, report, runner, 0 if candidate_ok else 1)
    files = {str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file()}
    require(files == retained | {"report.json", "events.jsonl"}, "unbound/missing retained artifact")
    require(sum((directory / name).stat().st_size for name in retained) == report["stored_bytes"] <= 1 << 30,
            "retained-byte ledger differs")
    require(positive(report["duration_ns"]) and positive(report["started_unix_ns"])
            and report["finished_unix_ns"] >= report["started_unix_ns"], "invalid run duration")
    return checked_rows, candidate_ok, prior


def cell_summaries(rows, phase, module, prior):
    result = []
    for index, cell in enumerate(cells_json()):
        all_rows = [row for row in rows if row["cell_index"] == index]
        measured = [row for row in all_rows if row["phase"] == "measured"]
        counts = {side: {"attempts": len(all_rows), "process_failures": sum(not r[side]["process_success"] for r in all_rows),
                         "quality_statuses": {status: sum(r[side]["quality"]["status"] == status for r in all_rows)
                            for status in sorted({r[side]["quality"]["status"] for r in all_rows})}} for side in SIDES}
        qualifications = {side: all(r[side]["process_success"] and r[side]["quality"]["status"] == "correct" for r in all_rows)
                          for side in SIDES}
        if prior is not None:
            before = prior[index]
            require(before["cell_index"] == index and all(before[k] == v for k, v in cell.items()), "preflight cell order differs")
            for side, label in (("baseline", "B"), ("candidate", "C")):
                qualifications[side] &= before[label]["process_success"] is True and before[label]["quality"]["correct"] is True
        equal = all(qualifications.values())
        item = {**cell, "cell_index": index, "attempts": counts,
                "quality_qualified": qualifications, "equivalent_output_timing_qualified": equal,
                "timing_interpretation": "equivalent-output comparison" if equal else
                    "correction-cost only" if qualifications["candidate"] else "candidate failed quality/process checks",
                "raw_pair_equality_count": sum(row["raw_equal"] is True for row in all_rows),
                "selected_latency_field": "total_ns" if cell["operation"] == "cold" else "render_ns"}
        if phase == "confirmation":
            selected = item["selected_latency_field"]
            metric = lambda key: (lambda arm: (arm["stages"] or {}).get(key))
            item["metrics"] = {
                "selected_latency": paired_metric(measured, metric(selected), module, True),
                "peak_rss_bytes": paired_metric(measured, lambda arm: arm["process"].get("peak_rss_bytes"), module, True),
                "process_elapsed_ns": paired_metric(measured, lambda arm: arm["process"].get("elapsed_ns"), module, False),
                **{key: paired_metric(measured, metric(key), module, False)
                   for key in ("open_ns", "compile_ns", "render_ns", "total_ns")}}
        result.append(item)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pins", type=Path, required=True)
    parser.add_argument("--pins-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new JSON outside the measured evidence directory")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error("audit output already exists")
    result = {"schema": 1, "experiment": "start-current", "status": "invalid-evidence", "admission": False,
              "audit_source_sha256": sha(read(Path(__file__).resolve())), "pins_sha256": args.pins_sha256,
              "candidate_preflight_correct": False, "cells": [], "errors": []}
    exit_code = 1
    try:
        pins = json_bound({"path": str(args.pins.resolve()), "sha256": args.pins_sha256})
        require(pins["schema"] == 1 and pins["experiment"] == "start-current"
                and pins["phase"] in ("preflight", "confirmation"), "invalid external audit pins")
        result.update(report_sha256=pins["report"]["sha256"], bindings_sha256=pins["bindings"]["sha256"], phase=pins["phase"])
        root = Path(pins["root"]).resolve(strict=True)
        require(not output.is_relative_to(Path(pins["report"]["path"]).parent.resolve()), "audit cannot mutate measured evidence directory")
        bindings, runner = verify_bindings(pins["bindings"], root)
        rows, candidate_ok, prior = verify_run(pins, bindings, runner)
        statistics_path = str((root / "tools/benchkit/statistics.py").resolve())
        require(pins["statistics"]["path"] == statistics_path, "wrong statistics helper")
        bound(pins["statistics"])
        module = statistics_module(pins["statistics"]) if pins["phase"] == "confirmation" else None
        result["cells"] = cell_summaries(rows, pins["phase"], module, prior)
        result.update(status=("verified-" + pins["phase"]) if candidate_ok else "verified-evidence-candidate-failed",
                      candidate_preflight_correct=candidate_ok if pins["phase"] == "preflight" else False,
                      candidate_quality_verified=candidate_ok, attempted_processes=len(rows) * 2,
                      quality_qualified_cells=sum(c["equivalent_output_timing_qualified"] for c in result["cells"]),
                      statistics={"resamples": RESAMPLES, "one_sided_confidence": CONFIDENCE, "seed_per_cell": SEED,
                          "bootstrap_metrics": ["selected_latency", "peak_rss_bytes"], "max_rss_ci": None},
                      scope="Native default-repair open/full compile; timed matrix emits no diagnostics; exact frozen RGB/dimensions/opaque alpha.",
                      limitations=["No aggregate, release admission or world-engine comparison.",
                          "Wrong baseline pixels retain correction-cost timing, never equivalent-output speed claims.",
                          "Intervals are paired percentile bootstrap estimates; each endpoint has separate one-sided 0.95 confidence, not joint coverage.",
                          "No guarantee for arbitrary distributions, temporal dependence, tail precision or simultaneous 84-cell comparisons.",
                          "Maximum RSS is descriptive only; OS high-water bytes include process/runtime costs.",
                          "Guard detects known processes at intervals, not absolute host exclusivity.",
                          "Frozen opaque fixture recipes and external audit pins remain trusted coordinator inputs; no renderer parity oracle."])
        exit_code = 0 if candidate_ok else 1
    except (OSError, ValueError, KeyError, TypeError, IndexError, subprocess.SubprocessError) as error:
        result.update(status="invalid-evidence", candidate_preflight_correct=False, candidate_quality_verified=False, cells=[])
        result["errors"].append(f"{type(error).__name__}: {error}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x") as handle:
        json.dump(result, handle, indent=2, allow_nan=False)
        handle.write("\n")
    print(f"{result['status']}: {output}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
