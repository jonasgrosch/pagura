"""Fixed untimed probes; no renderer output participates in these references."""
from fractions import Fraction as F

MODES = ("exact", "competitive")
SIDES = ("baseline", "candidate")
PLAN = {"schema": 1, "experiment": "start-current-accuracy", "outputs": 108,
        "requests_per_mode": 27, "modes": list(MODES), "sides": list(SIDES),
        "dpi": 72, "page": 0, "timed": False}
ROOT_PINS = {
    "root-two-partial": ("precision-root-two-partial.pdf", "5e82c29ff182df45ba2b01d936455eaa6cbd71f12e80ff5093a8a27c93e8c774"),
    "root-slivers-1000": ("precision-root-slivers-1000.pdf", "f2aec50f0537d090b821e145c7b958c90d17946002884848fe9d9bd897ccd943"),
}
COVERAGE_PINS = {
    "single-tiny": "09f0b3e394ccebea1599d830f65191b4ccd3eb7783f834585606fbaf14d87942",
    "coincident-tiny": "2adb85f8b7830906f2b64949219d4f63054aea1a14b043dd2c96a7bba1d56dd1",
    "disjoint-tiny": "16f850229b588f744f83497265cb7835e288af3613d9e76a7abac530798b529f",
    "compound-tiny": "747f3ce2455feced4fe4edaf3e1791e6e4351c114354b67c9f34c98c1e16bba2",
    "full-pixel": "b0dfc64ee5a7363fdfdc4cca38f0d95efadf80d28cfc0426b509e155cff8d0e9",
    "disjoint-halves": "17e979ff308045a16ec5e322f8983103c843f27c021acab95c901adbf95c8815",
}


def rounded(value):
    return (2 * value.numerator + value.denominator) // (2 * value.denominator)


def pdf(width, height, content, resources=b"<< >>"):
    """Four literal objects and a classic xref; no fixture generator imports."""
    body = content.encode("ascii")
    objects = (b"<< /Type /Catalog /Pages 2 0 R >>",
               b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources ".encode()
               + resources + b" /Contents 4 0 R >>",
               f"<< /Length {len(body)} >>\nstream\n".encode() + body + b"\nendstream")
    data, offsets = bytearray(b"%PDF-1.4\n"), []
    for index, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{index} 0 obj\n".encode() + obj + b"\nendobj\n")
    xref = len(data)
    data.extend(b"xref\n0 5\n0000000000 65535 f \n")
    for offset in offsets:
        data.extend(f"{offset:010d} 00000 n \n".encode())
    data.extend(f"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return bytes(data)


def strict(pixels, width, height, model):
    return {"width": width, "height": height, "model": model,
            "bounds": [[channel, channel] for pixel in pixels for channel in pixel]}


def gray(value, model):
    allowed = [byte for byte in range(256) if abs(F(byte) - value) <= 1]
    return {"width": 1, "height": 1, "model": model,
            "rational": [str(value.numerator), str(value.denominator)],
            "bounds": [[allowed[0], allowed[-1]]] * 3 + [[255, 255]]}


def probes(root, load_pinned):
    """Return fixed request order; old PDF bytes are reused unchanged."""
    requests = []

    def add(case, data, reference, *, background=(255, 255, 255, 255), operation="full", tile=(0, 0, 0, 0)):
        requests.append({"case": case, "pdf": data, "reference": reference,
                         "background": list(background), "operation": operation, "tile": list(tile)})

    for case, (filename, pin) in ROOT_PINS.items():
        data = load_pinned(root / "corpus/generated/content" / filename, pin)
        value = F(44617, 204) if case == "root-two-partial" else 255 * F(254, 255) ** 1000
        reference = gray(value, "declared coverage-byte normal composition; absolute error <= one level")
        add(case, data, reference)
        add(case, data, reference, operation="tile", tile=(0, 0, 1, 1))
    for case, pin in COVERAGE_PINS.items():
        data = load_pinned(root / "benchmarks/results/local/n16-output-u16/subbyte-001/inputs" / (case + ".pdf"), pin)
        value = F(255) if case in ("single-tiny", "coincident-tiny", "disjoint-tiny") else F(16129, 255) if case == "disjoint-halves" else F(0)
        add("coverage-" + case, data, gray(value, "declared coverage-byte composition, not continuous scene coverage"))
    half = b"<< /ExtGState << /A << /ca 0.5 >> >> >>"
    add("half-alpha-red-twice", pdf(1, 1, "/A gs 1 0 0 rg 0 0 1 1 re f 0 0 1 1 re f\n", half),
        strict([(255, rounded(F(255, 4)), rounded(F(255, 4)), 255)], 1, 1, "full coverage Normal source-over; final half-up byte"))
    tiny = b"<< /ExtGState << /A << /ca 0.0009765625 >> >> >>"
    level = rounded(255 * F(1023, 1024) ** 8)
    add("tiny-alpha-black-eight", pdf(1, 1, "/A gs 0 g\n" + "0 0 1 1 re f\n" * 8, tiny),
        strict([(level, level, level, 255)], 1, 1, "full coverage Normal source-over; final half-up byte"))
    for alpha in (0, 128, 254, 255):
        background = (200, 100, 50, alpha)
        blank = tuple(rounded(F(c * alpha, 255)) for c in background[:3]) + (alpha,)
        for name, body, weight, resources in (
            ("blank", "", F(0), b"<< >>"),
            ("half-coverage", "0 g 0 0 0.5 1 re f\n", F(128, 255), b"<< >>"),
            ("half-opacity", "/A gs 0 g 0 0 1 1 re f\n", F(1, 2), half),
        ):
            first = tuple(rounded(c * (1 - weight)) for c in blank[:3]) + (rounded(255 * weight + alpha * (1 - weight)),)
            add(f"background-{alpha}-{name}", pdf(2, 1, body, resources),
                strict([first, blank], 2, 1, "premultiply background once to byte; declared coverage or opacity; final half-up byte"), background=background)
    background = (200, 100, 52, 255)
    painted = (rounded(F(255 * 3 + 200, 4)), 25, 13, 255)
    data = pdf(259, 3, "/A gs 1 0 0 rg 255 0 3 2 re f 255 0 3 2 re f\n", half)
    for operation, tile in (("full", (0, 0, 0, 0)), ("tile", (254, 0, 5, 3)), ("tile", (256, 1, 2, 2))):
        x, y, width, height = (0, 0, 259, 3) if operation == "full" else tile
        pixels = [painted if 255 <= px < 258 and 1 <= py < 3 else background
                  for py in range(y, y + height) for px in range(x, x + width)]
        add("colored-seam", data, strict(pixels, width, height, "two half-alpha red marks; opaque colored background"),
            background=background, operation=operation, tile=tile)
    if len(requests) != PLAN["requests_per_mode"]:
        raise ValueError("fixed request count changed")
    return requests


def evaluate(raw, reference):
    header, separator, pixels = raw.partition(b"\n")
    expected = f"RGBA8 {reference['width']} {reference['height']} premultiplied".encode()
    if not separator or header != expected or len(pixels) != len(reference["bounds"]):
        raise ValueError("wrong raw RGBA header, dimensions or payload")
    errors = [index for index, (byte, (low, high)) in enumerate(zip(pixels, reference["bounds"])) if not low <= byte <= high]
    return {"independent_pass": not errors, "different_channels": len(errors),
            "alpha_pass": all(low <= pixels[index] <= high for index, (low, high) in enumerate(reference["bounds"]) if index % 4 == 3),
            "first_differences": [{"channel": index, "actual": pixels[index], "allowed": reference["bounds"][index]} for index in errors[:16]]}
