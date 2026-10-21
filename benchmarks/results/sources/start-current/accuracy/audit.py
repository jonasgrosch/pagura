#!/usr/bin/env python3
"""Independent untimed 108-output audit; imports no collector or renderer helper."""
import argparse
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess

SIDES, MODES = ("baseline", "candidate"), ("exact", "competitive")
BASELINE = "676a872eb9f6c279cbf1e3929ace3a0c99db9fc3"
CANDIDATE = "a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c"
PLAN = {"schema": 1, "experiment": "start-current-accuracy", "outputs": 108,
        "requests_per_mode": 27, "modes": list(MODES), "sides": list(SIDES),
        "dpi": 72, "page": 0, "timed": False}
PDF_PINS = (
    "5e82c29ff182df45ba2b01d936455eaa6cbd71f12e80ff5093a8a27c93e8c774",
    "f2aec50f0537d090b821e145c7b958c90d17946002884848fe9d9bd897ccd943",
    "09f0b3e394ccebea1599d830f65191b4ccd3eb7783f834585606fbaf14d87942",
    "2adb85f8b7830906f2b64949219d4f63054aea1a14b043dd2c96a7bba1d56dd1",
    "16f850229b588f744f83497265cb7835e288af3613d9e76a7abac530798b529f",
    "747f3ce2455feced4fe4edaf3e1791e6e4351c114354b67c9f34c98c1e16bba2",
    "b0dfc64ee5a7363fdfdc4cca38f0d95efadf80d28cfc0426b509e155cff8d0e9",
    "17e979ff308045a16ec5e322f8983103c843f27c021acab95c901adbf95c8815",
)


def require(value, message):
    if not value:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def read(path, limit=64 << 20):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, "oversized/nonregular file")
        with os.fdopen(fd, "rb", closefd=False) as handle:
            raw = handle.read(limit + 1)
    finally:
        os.close(fd)
    require(len(raw) <= limit, "file grew")
    return raw


def bound(pin):
    require(set(pin) == {"path", "sha256"} and Path(pin["path"]).is_absolute(), "invalid external pin")
    raw = read(pin["path"])
    require(sha(raw) == pin["sha256"], "external pin mismatch: " + pin["path"])
    return raw


def source(root, runner=False):
    root = Path(root).resolve()
    files = set()
    for directory in ("crates", "tools", "benchmarks/adapters"):
        files.update(path for path in (root / directory).rglob("*") if path.is_file()
                     and "target" not in path.parts and "__pycache__" not in path.parts)
    files.update((root / "corpus/manifests").glob("*.json"))
    names = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"]
    if runner:
        names += ["benchmarks/release-matrix.json", "benchmarks/requirements.txt", "benchmarks/node-package.json", "benchmarks/node-package-lock.json"]
    files.update(root / name for name in names)
    digest = hashlib.sha256()
    for path in sorted(files):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + read(path))
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    return {"root": str(root), "commit": commit, "source_sha256": digest.hexdigest()}


def expected_requests():
    """Independently stated arithmetic and geometry, with literal background controls."""
    requests = []

    def add(case, width, height, pixels, *, background=(255, 255, 255, 255), tile=None, pin=None, body=None, alpha=None):
        requests.append({"case": case, "width": width, "height": height, "bounds": pixels,
                         "background": list(background), "operation": "tile" if tile else "full",
                         "tile": list(tile) if tile else [0, 0, 0, 0], "pin": pin, "body": body, "alpha": alpha})

    def solid(pixel):
        return [(channel, channel) for channel in pixel]

    def rational(value):
        allowed = [integer for integer in range(256) if abs(Fraction(integer) - value) <= 1]
        return [(min(allowed), max(allowed))] * 3 + [(255, 255)]

    for case, value, pin in (("root-two-partial", Fraction(44617, 204), PDF_PINS[0]),
                              ("root-slivers-1000", 255 * Fraction(254, 255) ** 1000, PDF_PINS[1])):
        add(case, 1, 1, rational(value), pin=pin)
        add(case, 1, 1, rational(value), pin=pin, tile=(0, 0, 1, 1))
    for index, case in enumerate(("single-tiny", "coincident-tiny", "disjoint-tiny", "compound-tiny", "full-pixel", "disjoint-halves")):
        value = Fraction(255) if index < 3 else Fraction(16129, 255) if index == 5 else Fraction(0)
        add("coverage-" + case, 1, 1, rational(value), pin=PDF_PINS[index + 2])
    add("half-alpha-red-twice", 1, 1, solid((255, 64, 64, 255)),
        body="/A gs 1 0 0 rg 0 0 1 1 re f 0 0 1 1 re f\n", alpha="0.5")
    # 255 * (1023/1024)^8 is between 252.5 and 253.5, hence byte253.
    value = 255 * Fraction(1023, 1024) ** 8
    require(Fraction(505, 2) < value < Fraction(507, 2), "tiny-alpha derivation changed")
    add("tiny-alpha-black-eight", 1, 1, solid((253, 253, 253, 255)),
        body="/A gs 0 g\n" + "0 0 1 1 re f\n" * 8, alpha="0.0009765625")
    for alpha, blank, covered, translucent in (
        (0, (0, 0, 0, 0), (0, 0, 0, 128), (0, 0, 0, 128)),
        (128, (100, 50, 25, 128), (50, 25, 12, 192), (50, 25, 13, 192)),
        (254, (199, 100, 50, 254), (99, 50, 25, 255), (100, 50, 25, 255)),
        (255, (200, 100, 50, 255), (100, 50, 25, 255), (100, 50, 25, 255)),
    ):
        for name, first, body, opacity in (("blank", blank, "", None),
             ("half-coverage", covered, "0 g 0 0 0.5 1 re f\n", None),
             ("half-opacity", translucent, "/A gs 0 g 0 0 1 1 re f\n", "0.5")):
            add(f"background-{alpha}-{name}", 2, 1, solid(first) + solid(blank),
                background=(200, 100, 50, alpha), body=body, alpha=opacity)
    for tile in (None, (254, 0, 5, 3), (256, 1, 2, 2)):
        x, y, width, height = tile or (0, 0, 259, 3)
        pixels = [bound for yy in range(y, y + height) for xx in range(x, x + width)
                  for bound in solid((241, 25, 13, 255) if 255 <= xx < 258 and yy >= 1 else (200, 100, 52, 255))]
        add("colored-seam", width, height, pixels, background=(200, 100, 52, 255), tile=tile,
            body="/A gs 1 0 0 rg 255 0 3 2 re f 255 0 3 2 re f\n", alpha="0.5")
    require(len(requests) == 27, "wrong independent request count")
    return requests


def check_pdf(raw, request):
    if request["pin"]:
        require(sha(raw) == request["pin"], "original PDF differs")
        return
    width, height = (259, 3) if request["case"] == "colored-seam" else (request["width"], request["height"])
    resource = "<< >>" if request["alpha"] is None else f"<< /ExtGState << /A << /ca {request['alpha']} >> >> >>"
    body = request["body"].encode("ascii")
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources {resource} /Contents 4 0 R >>".encode(),
               f"<< /Length {len(body)} >>\nstream\n".encode() + body + b"\nendstream"]
    expected, offsets = b"%PDF-1.4\n", []
    for index, obj in enumerate(objects, 1):
        offsets.append(len(expected))
        expected += f"{index} 0 obj\n".encode() + obj + b"\nendobj\n"
    xref = len(expected)
    expected += b"xref\n0 5\n0000000000 65535 f \n" + b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets)
    expected += f"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    require(raw == expected, "literal independent PDF geometry changed")


def audit(pins):
    root = Path(pins["root"]).resolve()
    report_raw = bound(pins["report"])
    report, bindings, receipt, guard = (json.loads(raw) for raw in (report_raw, bound(pins["bindings"]), bound(pins["receipt"]), bound(pins["guard"])))
    directory = Path(pins["report"]["path"]).parent
    require(report["schema"] == 1 and report["experiment"] == PLAN["experiment"]
            and report["plan"] == bindings["plan"] == PLAN and report["status"] == "completed"
            and report["candidate_quality_verified"] is True and report["all_processes_ok"] is True
            and report["admission"] is False and "fatal_error" not in report, "collection did not complete correctly")
    require(bindings["schema"] == 1 and bindings["experiment"] == PLAN["experiment"]
            and report["bindings_sha256"] == pins["bindings"]["sha256"]
            and report["identities"] == bindings["identities"]
            and report["harness_sha256"] == bindings["harness_sha256"], "binding identity differs")
    identities = bindings["identities"]
    require(set(identities) == set(SIDES) and identities["baseline"]["commit"] == BASELINE
            and identities["candidate"]["commit"] == CANDIDATE and identities["candidate"]["root"] == str(root), "wrong fixed source arms")
    for identity in identities.values():
        require(source(identity["root"]) == {key: identity[key] for key in ("root", "commit", "source_sha256")}, "product source changed")
        require(sha(read(identity["binary"])) == identity["binary_sha256"], "accuracy binary changed")
    for path, digest in bindings["harness_sha256"].items():
        require(sha(read(path)) == digest, "harness source changed")

    def artifact(relative):
        require(type(relative) is str and relative and not Path(relative).is_absolute()
                and all(part not in ("", ".", "..") for part in relative.split("/")), "invalid artifact path")
        path = (directory / relative).resolve(strict=True)
        require(path.is_relative_to(directory.resolve()), "artifact escaped directory")
        raw = read(path, 4 << 20)
        require(report["artifacts"][relative] == {"sha256": sha(raw), "bytes": len(raw)}, "artifact changed")
        return raw

    total = sum(len(artifact(name)) for name in report["artifacts"])
    require(total == report["stored_bytes"] and total <= 64 << 20, "retained byte inventory differs")
    require(artifact(report["bindings"]) == bound(pins["bindings"]), "retained binding differs")
    require(len(report["completed_evidence"]) == len(bindings["completed_evidence"]) > 0, "missing build evidence")
    for item, original in zip(report["completed_evidence"], bindings["completed_evidence"]):
        require({key: item[key] for key in ("path", "sha256")} == original
                and artifact(item["artifact"]) == bound(original), "completed evidence changed")
    # Independently pinned coordinator reviewed these build records before execution.
    require(pins["reviewed_build_evidence"] == bindings["completed_evidence"], "build review pins differ")
    command = pins["command"]
    require(len(command) == 11 and command[1] == "-B" and Path(command[0]).is_absolute()
            and Path(command[2]).is_absolute(), "wrong direct collector invocation")
    require(command[3:] == ["--root", str(root), "--bindings", pins["bindings"]["path"],
                           "--bindings-sha256", pins["bindings"]["sha256"], "--output", str(directory)], "collector options differ")
    require(command[2] in bindings["harness_sha256"], "collector not pinned")
    require(report["producer_argv"] == pins["producer_argv"] and report["producer_argv"][1:] == command[1:], "observed Python invocation differs")
    runtime = pins["runtime_files_sha256"]
    require({str(Path(command[0]).resolve()), str(Path(report["producer_argv"][0]).resolve())} <= set(runtime), "unbound launcher/runtime handoff")
    for path, digest in runtime.items():
        require(bindings["harness_sha256"].get(path) == digest and sha(read(path)) == digest,
                "runtime differs from pre-run binding")
    require(receipt["schema"] == 1 and receipt["status"] == "completed" and receipt["exit_code"] == receipt["wrapper_exit_code"] == 0
            and receipt["cancellation_signal"] is None and receipt["errors"] == [] and receipt["artifact_errors"] == {}
            and receipt["source_unchanged"] is True and receipt["source_before"] == receipt["source_after"], "nonterminal/failed outer receipt")
    runner = source(root, runner=True)
    require(receipt["cwd"] == receipt["source_root"] == str(root)
            and receipt["source_before"] == {"commit": runner["commit"], "sha256": runner["source_sha256"]}
            and receipt["run_id"] == report["run_id"] and report["receipt"] == pins["receipt"]["path"], "runner source or pointer differs")
    require(guard["schema"] == 1 and guard["status"] == "completed" and guard["observation_only"] is False
            and guard["exit_code"] == guard["child_exit_code"] == 0 and guard["command"] == command
            and [event["stage"] for event in guard["events"]] == ["preflight", "postflight"]
            and all(event["matches"] == [] for event in guard["events"]), "strict observer failed")
    observer = root / "benchmarks/results/sources/analytic19-conversion/load_observer.py"
    argv = receipt["argv"]
    require(len(argv) == len(command) + 6 and argv[1] == "-B" and (root / argv[2]).resolve() == observer
            and argv[3] == "--evidence-dir" and (root / argv[4]).resolve() == Path(pins["guard"]["path"]).parent
            and argv[5:] == ["--", *command], "runner observer collector chain differs")
    require(set(pins["coordination_sources"]) == {str(root / "tools/runner.py"), str(observer)}, "coordination source pins missing")
    for path, digest in pins["coordination_sources"].items():
        require(sha(read(path)) == digest, "coordination source changed")
    for name in ("report", "guard"):
        require(receipt["artifacts"].get(pins[name]["path"]) == pins[name]["sha256"], "terminal receipt omits artifact")
    expected = expected_requests()
    require(len(report["inputs"]) == 27 and len(report["records"]) == 108, "partial request matrix")
    failed, saved = [], {}
    for index, request in enumerate(expected):
        input_row = report["inputs"][index]
        require(input_row["request"] == index and all(input_row[key] == request[key] for key in ("case", "operation", "background", "tile")), "request settings differ")
        check_pdf(artifact(input_row["pdf"]), request)
        reference = json.loads(artifact(input_row["reference"]))
        require((reference["width"], reference["height"]) == (request["width"], request["height"])
                and reference["bounds"] == [list(pair) for pair in request["bounds"]], "retained reference differs from independent derivation")
        for mode_index, mode in enumerate(MODES):
            for side_index, side in enumerate(SIDES):
                row = report["records"][index * 4 + mode_index * 2 + side_index]
                require((row["request"], row["case"], row["mode"], row["side"], row["reference"])
                        == (index, request["case"], mode, side, input_row["reference"]), "record ordering differs")
                require(row["command"] == [identities[side]["binary"], str(directory / input_row["pdf"]), mode, request["operation"],
                        str(directory / row["output"]), *map(str, request["background"]), *map(str, request["tile"])], "renderer arguments differ")
                process = row["process"]
                require(type(process["exit_code"]) is int and process["exit_code"] == 0 and process["status"] != "timeout"
                        and row["process_ok"] is True and not any(key in row for key in ("error", "interrupted", "output_error")), "renderer process/evidence failure")
                log = artifact(row["log"])
                require(len(log) < 64 << 10, "possibly truncated log")
                metadata = [json.loads(line) for line in log.decode("ascii").splitlines() if line.lstrip().startswith("{")]
                wanted = {"schema": 1, "accuracy": mode, "operation": request["operation"], "page": 0, "dpi": 72,
                          "background": request["background"], "tile": request["tile"], "width": request["width"], "height": request["height"],
                          "diagnostics": 0, "suppressed_diagnostics": 0}
                require(metadata == [wanted] and row["metadata"] == wanted, "raw invocation metadata differs")
                raw = artifact(row["output"])
                header, separator, pixels = raw.partition(b"\n")
                require(separator and header == f"RGBA8 {request['width']} {request['height']} premultiplied".encode()
                        and len(pixels) == request["width"] * request["height"] * 4, "invalid raw dimensions/payload")
                differences = [i for i, (byte, (low, high)) in enumerate(zip(pixels, request["bounds"])) if not low <= byte <= high]
                quality = {"independent_pass": not differences, "different_channels": len(differences),
                           "alpha_pass": all(request["bounds"][i][0] <= pixels[i] <= request["bounds"][i][1] for i in range(3, len(pixels), 4)),
                           "first_differences": [{"channel": i, "actual": pixels[i], "allowed": list(request["bounds"][i])} for i in differences[:16]]}
                require(row["quality"] == quality and row["independent_pass"] is (not differences), "reported quality differs")
                require(side != "candidate" or not differences, "candidate violates independent reference")
                if differences:
                    failed.append({"request": index, "case": request["case"], "mode": mode})
                saved[index, mode, side] = pixels
    require(report["baseline_failed_probes"] == failed, "baseline failure ledger changed")
    for mode in MODES:
        for side in SIDES:
            require(saved[0, mode, side] == saved[1, mode, side] and saved[2, mode, side] == saved[3, mode, side], "root full/tile identity differs")
            full = saved[24, mode, side]
            for index, (x, y, width, height) in ((25, (254, 0, 5, 3)), (26, (256, 1, 2, 2))):
                crop = b"".join(full[(yy * 259 + x) * 4:(yy * 259 + x + width) * 4] for yy in range(y, y + height))
                require(crop == saved[index, mode, side], "colored full/tile identity differs")
    return {"schema": 1, "status": "verified-accuracy-probes", "candidate_quality_verified": True,
            "report_sha256": sha(report_raw), "identities": identities, "outputs_verified": 108,
            "baseline_failed_probes": failed, "admission": False,
            "limitations": ["Native retained-list only; declared scalar coverage is not continuous-scene perfection",
                            "Build evidence reviewed and externally pinned by coordinator; not recompiled by auditor",
                            "Requested launcher and observed Python app bytes match the pre-run binding; this is not full OS/runtime closure"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pins", type=Path, required=True)
    parser.add_argument("--pins-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raw = read(args.pins)
    require(sha(raw) == args.pins_sha256, "audit pins changed")
    result = audit(json.loads(raw))
    with args.output.open("x") as handle:
        json.dump(result, handle, indent=2, sort_keys=True)
        handle.write("\n")


if __name__ == "__main__":
    main()
