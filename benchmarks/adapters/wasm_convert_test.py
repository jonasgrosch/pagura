"""Opt-in native/V8/Wasmtime parity against independent two-page rectangle pixels.

Usage: python3 benchmarks/adapters/wasm_convert_test.py NATIVE WASM WASMTIME
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.images import normalize
from benchkit.timings import parse_document_stages


def fixture():
    streams = [b"0 g 16 16 32 32 re f\n", b"1 0 0 rg 0 0 32 64 re f\n"]
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>"]
    for content in (5, 6):
        objects.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents {content} 0 R >>".encode())
    objects.extend(b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"endstream" for stream in streams)
    data = bytearray(b"%PDF-1.7\n")
    offsets = []
    for number, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data += f"{number} 0 obj\n".encode() + obj + b"\nendobj\n"
    xref = len(data)
    data += b"xref\n0 7\n0000000000 65535 f \n"
    for offset in offsets:
        data += f"{offset:010} 00000 n \n".encode()
    data += f"trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(data)


def expected(page, scale):
    size = 64 * scale
    pixels = bytearray()
    for y in range(size):
        for x in range(size):
            color = (0, 0, 0) if 16 * scale <= x < 48 * scale and 16 * scale <= y < 48 * scale else (255, 255, 255)
            if page == 1:
                color = (255, 0, 0) if x < 32 * scale else (255, 255, 255)
            pixels.extend(color)
    return size, size, bytes(pixels)


def main():
    native, wasm, wasmtime = map(lambda value: str(Path(value).resolve()), sys.argv[1:])
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        source = directory / "analytic.pdf"
        source.write_bytes(fixture())
        for runtime in ("native", "v8", "wasmtime"):
            for dpi, scale in ((72, 1), (144, 2)):
                output = directory / f"{runtime}-{dpi}"
                output.mkdir()
                if runtime == "native":
                    command = [native, str(source), str(output)]
                elif runtime == "v8":
                    command = [os.environ.get("PAGURA_NODE", "node"), "--single-threaded", "--no-liftoff",
                               str(ROOT / "benchmarks/adapters/wasi.mjs"), wasm, str(source), str(output)]
                else:
                    command = [wasmtime, "run", "-C", "cache=n", "-C", "parallel-compilation=n",
                               "--dir", f"{directory}::/input", "--dir", f"{output}::/output",
                               wasm, "/input/analytic.pdf", "/output"]
                result = subprocess.run(command + [str(dpi), "0", "1"], capture_output=True, text=True, timeout=60, check=True)
                stages = parse_document_stages(result.stdout, [0, 1], 60_000_000_000)
                assert stages and stages["render_ns"] > 0
                for page in (0, 1):
                    target = output / f"page-{page + 1:04}.ppm"
                    assert normalize(target.read_bytes()) == expected(page, scale)
                    target.unlink()
                refused = subprocess.run(command + [str(dpi), "0", "2"], capture_output=True, text=True, timeout=60)
                assert refused.returncode != 0 and "page range outside" in refused.stderr
                assert not list(output.iterdir())
                print(json.dumps({"runtime": runtime, "dpi": dpi, "independent_pages": 2,
                                  "pixels": "exact", "out_of_range": "refused"}))


if __name__ == "__main__":
    main()
