"""Explicit external-adapter smoke: exact analytic pixels at three resolutions.

Run on macOS after compiling quartz.swift outside the repository:
    python3 benchmarks/adapters/quartz_geometry_test.py /external/path/to/quartz

The generated PDF and expected pixels use only the standard library and never
invoke Pagura. This is an opt-in platform integration check, not a benchmark or
part of the portable, fake-engine unit suite.
"""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
from benchkit.images import normalize


def rectangle_pdf(rotation):
    """One centred black rectangle, a nonzero box origin and a valid classic xref."""
    content = b"0 g 17 19 48 24 re f\n"
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        ("<< /Type /Page /Parent 2 0 R /MediaBox [5 7 77 55] "
         "/CropBox [5 7 77 55] /Rotate " + str(rotation) +
         " /Resources << >> /Contents 4 0 R >>").encode(),
        b"<< /Length " + str(len(content)).encode() + b" >>\nstream\n" + content + b"endstream",
    ]
    data = bytearray(b"%PDF-1.7\n")
    offsets = []
    for number, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data += f"{number} 0 obj\n".encode() + obj + b"\nendobj\n"
    xref = len(data)
    data += b"xref\n0 5\n0000000000 65535 f \n"
    for offset in offsets:
        data += f"{offset:010} 00000 n \n".encode()
    data += f"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(data)


def check(binary):
    """Return six exact-pixel results, rejecting any adapter/process/output failure."""
    results = []
    with tempfile.TemporaryDirectory(prefix="pagura-quartz-geometry-") as temporary:
        root = Path(temporary)
        for rotation in (0, 90):
            source = root / f"rotate-{rotation}.pdf"
            source.write_bytes(rectangle_pdf(rotation))
            # These choices place every rectangle edge on an integer pixel.
            for dpi, long_side, short_side, border in ((36, 36, 24, 6), (72, 72, 48, 12), (150, 150, 100, 25)):
                width, height = (long_side, short_side) if rotation == 0 else (short_side, long_side)
                pixels = bytes(0 if border <= x < width - border and border <= y < height - border else 255
                               for y in range(height) for x in range(width) for _ in range(3))
                output = root / f"{rotation}-{dpi}"
                output.mkdir()
                process = subprocess.run([str(binary), str(source), str(output), str(dpi), "0", "0"],
                                         capture_output=True, text=True, timeout=30, check=True)
                actual = normalize((output / "page-0001.ppm").read_bytes())
                if actual != (width, height, pixels):
                    raise ValueError(f"analytic rectangle mismatch at {dpi} dpi, rotation {rotation}")
                record = json.loads(process.stdout)
                results.append({"dpi": dpi, "rotation": rotation, "exact_pixels": True, "timings": record})
    return results


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: quartz_geometry_test.py /external/path/to/quartz")
    print(json.dumps({"analytic_geometry": check(Path(sys.argv[1]).resolve())}, indent=2))
