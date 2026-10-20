#!/usr/bin/env python3
"""Small old/new root-glyph-hull experiment; never a release/competitor claim.

Run only through the coordinator's exclusive runner. Both binaries must contain
the identical scene_index_profile example. Preparation, hashes, oracle creation,
verification and emission warm the filesystem; these are not cold-I/O samples.
"""
import argparse
import hashlib
import json
import os
import platform
import random
import struct
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.measurement import run_process
from benchkit.statistics import ratio_bounds
from fixturegen.font import simple, truetype, u16, u32, i16
from fixturegen.pdf import document, stream, opaque_rectangles

CASES = ("sparse64", "dense64", "tiny", "opaque64", "one-source64", "overcapacity128", "nested64",
         "default-knockout64")
MODES = ("exact", "competitive")
# Fixed before the pilot: targets plus cold/first-tile/full-tiled controls,
# every fallback family in both modes, and tiny cold preparation cost.
CELLS = tuple((name, mode, operation)
              for name in CASES for mode in MODES
              for operation in (("cold", "full", "tile", "tiled") if name == "sparse64"
                                else ("cold", "full") if name == "tiny" else ("full",)))
MAX_BYTES = 64 * 1024 * 1024
MAX_RETAINED = 512 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def font_objects():
    # 64 independently encoded glyph IDs, each a 400x600 rectangle. Distinct
    # glyph identities exercise 64 sources per run without changing geometry.
    glyph = simple([(0, 0, True), (400, 0, True), (400, 600, True), (0, 600, True)])
    glyph = glyph[:2] + struct.pack(">hhhh", 0, 0, 400, 600) + glyph[10:]
    glyph += bytes(len(glyph) % 2)
    offsets = [0, 0] + [len(glyph) * index for index in range(1, 65)]
    sub = (u16(4) + u16(32) + u16(0) + u16(4) + u16(4) + u16(1) + u16(0)
           + u16(96) + u16(65535) + u16(0) + u16(33) + u16(65535)
           + i16(-32) + i16(1) + u16(0) + u16(0))
    program = truetype(table_overrides={
        b"glyf": glyph * 64, b"loca": b"".join(u32(offset) for offset in offsets),
        b"maxp": u32(0x10000) + u16(65) + bytes(26),
        b"hhea": u32(0x10000) + bytes(30) + u16(65),
        b"hmtx": (u16(400) + i16(0)) * 65,
        b"cmap": u16(0) + u16(1) + u16(3) + u16(1) + u32(12) + sub,
        b"post": u32(0x30000) + bytes(28),
    })
    return [b"<< /Type /Font /Subtype /TrueType /BaseFont /Rect /FirstChar 33 /LastChar 96 /Widths ["
            + b"400 " * 64 + b"] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>",
            b"<< /Type /FontDescriptor /FontName /Rect /Flags 32 /FontBBox [0 0 400 600] /ItalicAngle 0 /Ascent 600 /Descent 0 /CapHeight 600 /StemV 50 /FontFile2 7 0 R >>",
            stream(program, b"/Length1 %d" % len(program))]


def fixture(name):
    size = 64 if name == "tiny" else 2048
    count = 1 if name == "tiny" else 128 if name == "overcapacity128" else 64
    # Default /TK true wraps text objects in groups and is a protected fallback.
    # The candidate only reaches explicitly non-knockout direct root glyph runs.
    knockout = b"" if name == "default-knockout64" else b"/TK false "
    # Quarter alpha has the same independently derived final RGB8 value (191)
    # under both current modes; half alpha exposes a pre-existing quantization
    # difference (Exact 128, Competitive 127), recorded in preflight separately.
    alpha = b"1" if name == "opaque64" else b".25"
    resources = (b"<< /Font << /F 5 0 R >> /ExtGState << /Paint << /ca " + alpha
                 + b" " + knockout + b">> >> >>")
    content = bytearray(b"/Paint gs 0 g\n")
    rectangles = []
    codes = bytes([33] * 64) if name == "one-source64" else bytes(range(33, 97))
    for index in range(count):
        content.extend(b"BT /F 10 Tf\n")
        if name == "dense64":
            content.extend(b"1 g\n")
            for character in range(33, 97):
                x, y = ((character - 33) % 2) * (size - 4), ((character - 33) // 2 % 2) * (size - 6)
                content.extend(f"1 0 0 1 {x} {y} Tm <{character:02x}> Tj\n".encode())
            # White at quarter alpha over white is exactly white on overlap.
        else:
            x = (index % 8) * 256
            y = 16 + (index // 8) * (128 if name == "overcapacity128" else 256)
            content.extend(f"1 0 0 1 {x} {y} Tm <{codes.hex()}> Tj\n".encode())
            gray = 0 if name == "opaque64" else 191
            rectangles.append(((x, y, 256, 6), (gray,) * 3))
        content.extend(b"ET\n")
    fonts = font_objects()
    if name == "nested64":
        form = stream(bytes(content), b"/Type /XObject /Subtype /Form /BBox [0 0 2048 2048] /Resources "
                      + resources + b" /Group << /S /Transparency /I true >> ")
        contents, resources, extra = b"/Nested Do", b"<< /XObject << /Nested 8 0 R >> >>", [form]
    else:
        contents, extra = bytes(content), []
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources " % (size, size)
        + resources + b" /Contents 4 0 R >>", stream(contents), *fonts, *extra,
    ])
    return pdf, opaque_rectangles(size, size, rectangles)


def source_identity(root):
    root = Path(root).resolve()
    paths = set()
    for name in ("crates", "tools", "benchmarks/adapters"):
        paths.update(path for path in (root / name).rglob("*") if path.is_file()
                     and "target" not in path.parts and "__pycache__" not in path.parts)
    paths.update(path for path in (root / "corpus/manifests").glob("*.json"))
    paths.update(root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"))
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes())
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    return {"root": str(root), "commit": commit, "source_sha256": digest.hexdigest()}


def expected_image(oracle, operation):
    width, height, pixels = normalize(oracle)
    if operation != "tile":
        return width, height, pixels
    w, h = min(width, 256), min(height, 256)
    return w, h, b"".join(pixels[row * width * 3:row * width * 3 + w * 3] for row in range(h))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for side in ("baseline", "candidate"):
        parser.add_argument("--" + side, required=True, type=Path)
        parser.add_argument("--" + side + "-source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="new private evidence directory")
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--phase", choices=("pilot", "confirmation"), default="pilot")
    parser.add_argument("--seed", type=int, default=42017)
    args = parser.parse_args()
    if not 2 <= args.pairs <= 1000 or not 0 <= args.warmups <= 100:
        parser.error("pairs must be 2..1000; warmups 0..100")
    if args.phase == "confirmation" and (args.pairs < 100 or args.warmups < 5):
        parser.error("confirmation needs at least100 fresh pairs and five warmups")
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    report_path = directory / "report.json"
    roots = {side: getattr(args, side + "_source").resolve() for side in ("baseline", "candidate")}
    binaries = {side: getattr(args, side).resolve() for side in roots}
    identities = {side: {**source_identity(root), "binary": str(binaries[side]),
                         "binary_sha256": hash_regular_file(binaries[side])} for side, root in roots.items()}
    example = Path("crates/pagura-render/examples/scene_index_profile.rs")
    if (roots["baseline"] / example).read_bytes() != (roots["candidate"] / example).read_bytes():
        raise ValueError("baseline and candidate harness sources differ")
    report = {"schema": 1, "experiment": "root-glyph-hull-cache", "phase": args.phase,
        "status": "running", "admission": "not evaluated; pilot can never accept",
        "identities": identities, "driver_sha256": sha(Path(__file__).read_bytes()),
        "example_sha256": sha((roots["candidate"] / example).read_bytes()),
        "pairs": args.pairs, "warmups": args.warmups, "seed": args.seed, "dpi": 72,
        "scope": "native one-thread retained full-validation API; hashed/warm filesystem; not host or WASM",
        "target_scope": "explicit /TK false non-knockout text; 64 distinct glyph sources per direct root run",
        "environment": {"platform": platform.platform(), "machine": platform.machine(),
                        "python": sys.version, "run_id": os.environ.get("PAGURA_RUN_ID"),
                        "run_receipt": os.environ.get("PAGURA_RUN_RECEIPT"),
                        "rust_toolchain": (roots["candidate"] / "rust-toolchain.toml").read_text()},
        "limitations": ["build commands and binary/source correspondence are trusted coordinator records",
                        "white dense control tests full-page hull fallback, not typical black text cost",
                        "no default-knockout text benefit is claimed",
                        "100 pairs is a minimum, not proof of adequate p95 precision"],
        "thresholds": {"target_median_upper": .95, "target_p95_upper": 1.0,
                       "protected_median_upper": 1.02, "protected_p95_upper": 1.02,
                       "peak_rss_ratio_max": 1.0},
        "statistics": {"method": "paired percentile bootstrap", "resamples": 5000,
                       "one_sided_confidence": .95, "seed": 42017,
                       "scope": "marginal estimates; temporal dependence and p95 precision need review"},
        "declared_cells": [{"case": name, "mode": mode, "operation": operation,
                            "role": "target" if name == "sparse64" and operation == "full" else "protected"}
                           for name, mode, operation in CELLS],
        "inputs": {}, "cells": []}
    stored = 0

    def retain(relative, data):
        nonlocal stored
        path = directory / relative
        if path.exists():
            if read_bounded_regular(path, len(data)) != data:
                raise ValueError("retained evidence changed")
            return
        if stored + len(data) > MAX_RETAINED:
            raise ValueError("experiment retention budget exhausted")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o400)
        stored += len(data)

    def checkpoint():
        report["stored_bytes"] = stored
        report_path.write_text(json.dumps(report, indent=2) + "\n")

    policy = {"timeout_seconds": 120, "maximum_output_bytes": MAX_BYTES, "measure_rss": True}
    rng = random.Random(args.seed)
    try:
        oracles = {}
        for name in CASES:
            pdf, oracle = fixture(name)
            retain(f"inputs/{name}.pdf", pdf)
            retain(f"oracles/{name}.ppm", oracle)
            report["inputs"][name] = {"pdf_sha256": sha(pdf), "oracle_sha256": sha(oracle)}
            oracles[name] = oracle
        cells = list(CELLS)
        rng.shuffle(cells)
        for name, mode, operation in cells:
            cell = {"case": name, "mode": mode, "operation": operation,
                    "role": "target" if name == "sparse64" and operation == "full" else "protected", "pairs": []}
            report["cells"].append(cell)
            expected = expected_image(oracles[name], operation)
            for pair in range(-args.warmups, args.pairs):
                order = ["baseline", "candidate"]
                rng.shuffle(order)
                sample = {"pair": pair, "order": order, "started_monotonic_ns": time.monotonic_ns()}
                cell["pairs"].append(sample)
                for side in order:
                    pdf = directory / "inputs" / (name + ".pdf")
                    if sha(read_bounded_regular(pdf, MAX_BYTES)) != report["inputs"][name]["pdf_sha256"]:
                        raise ValueError("input changed")
                    if hash_regular_file(binaries[side]) != identities[side]["binary_sha256"]:
                        raise ValueError("binary changed")
                    output = directory / "pending.rgba"
                    output.unlink(missing_ok=True)
                    command = [str(binaries[side]), str(pdf), mode, operation, str(output), "72"]
                    process, diagnostic = run_process(command, policy)
                    record = {"command": command, "process": process, "diagnostic": diagnostic}
                    sample[side] = record
                    if process["exit_code"] != 0 or process["status"] == "timeout":
                        raise ValueError("sample failed; exact diagnostic retained")
                    stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
                    if len(stages) != 1 or not isinstance(stages[0], dict) or any(type(stages[0].get(key)) is not int or stages[0][key] <= 0
                        for key in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")):
                        raise ValueError("missing or invalid stage timings")
                    record["stages"] = stages[0]
                    stages = stages[0]
                    if (stages.get("schema") != 1 or stages.get("operation") != operation
                            or stages.get("accuracy") != mode or stages.get("dpi") != 72
                            or stages.get("width") != expected[0] or stages.get("height") != expected[1]
                            or stages["metric_ns"] != stages["total_ns" if operation == "cold" else "render_ns"]
                            or stages["total_ns"] < stages["open_ns"] + stages["compile_ns"] + stages["render_ns"]):
                        raise ValueError("stage identity or timing relationship mismatch")
                    raw = read_bounded_regular(output, MAX_BYTES)
                    blob = "blobs/" + sha(raw) + ".rgba"
                    retain(blob, raw)
                    actual = normalize(raw)
                    header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
                    if not raw.startswith(header) or raw[len(header) + 3::4] != b"\xff" * (expected[0] * expected[1]):
                        raise ValueError("expected premultiplied RGBA8 over opaque white background")
                    record.update(blob=blob, output_sha256=sha(raw), pixels_sha256=sha(actual[2]),
                                  width=actual[0], height=actual[1], oracle_equal=actual == expected)
                    if actual != expected:
                        raise ValueError("independent integer-rectangle oracle mismatch")
                    output.unlink()
                if sample["baseline"]["output_sha256"] != sample["candidate"]["output_sha256"]:
                    raise ValueError("same-policy baseline/candidate byte mismatch")
                sample["finished_monotonic_ns"] = time.monotonic_ns()
                checkpoint()
            print(f"completed {len(report['cells'])}/{len(CELLS)}: {name}/{mode}/{operation}", flush=True)
        for side, root in roots.items():
            if (source_identity(root) != {key: identities[side][key] for key in ("root", "commit", "source_sha256")}
                    or hash_regular_file(binaries[side]) != identities[side]["binary_sha256"]):
                raise ValueError("source or binary changed during run")
        # Statistical work follows all timed samples; no interim admission.
        for cell in report["cells"]:
            pairs = [sample for sample in cell["pairs"] if sample["pair"] >= 0]
            cell["latency"] = ratio_bounds([sample["candidate"]["stages"]["metric_ns"] for sample in pairs],
                                          [sample["baseline"]["stages"]["metric_ns"] for sample in pairs])
            rss = {side: [sample[side]["process"].get("peak_rss_bytes") for sample in pairs] for side in roots}
            cell["peak_rss_ratio"] = (max(rss["candidate"]) / max(rss["baseline"])
                if all(type(value) is int and value > 0 for values in rss.values() for value in values) else None)
        report["status"] = "complete-exploratory"
    except BaseException as error:
        report.update(status="invalid-or-incomplete", error=str(error))
        raise
    finally:
        checkpoint()
    print(report_path)


if __name__ == "__main__":
    main()
