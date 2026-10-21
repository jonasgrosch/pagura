"""Two integer-edge controls with independent literal device-space pixels."""
from fixturegen.pdf import document, stream

CASES = ("opaque-empty", "opaque-distant")
SIDE = 512


def fixture(name):
    if name not in CASES:
        raise ValueError("unknown painted-interval fixture")
    commands = []
    if name == "opaque-distant":
        for cy in (0, 1):
            for cx in (0, 1):
                commands.extend((f"{256 * cx} {256 * cy + 255} 1 1 re f",
                                 f"{256 * cx + 255} {256 * cy} 1 1 re f"))
    content = ("0 g\n" + "\n".join(commands)).encode() if commands else b""
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 512 512] /Resources << >> /Contents 4 0 R >>",
        stream(content),
    ])
    # This oracle does not consume the emitted rectangle list or PDF bytes.
    # Literal top-down positions include the PDF y inversion: first and last
    # linear pixel of each of the four 256x256 device chunks.
    pixels = bytearray(b"\xff" * (SIDE * SIDE * 3))
    if name == "opaque-distant":
        for x, y in ((0, 0), (255, 255), (256, 0), (511, 255),
                     (0, 256), (255, 511), (256, 256), (511, 511)):
            offset = 3 * (y * SIDE + x)
            pixels[offset:offset + 3] = b"\0\0\0"
    return pdf, b"P6\n512 512\n255\n" + bytes(pixels)
