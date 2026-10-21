"""Three new, untimed route controls; fixed before their first render.

The caller supplies the existing tools/fixturegen import path. This module only
authors inputs when fixture() is called; it does not read any renderer output.
Its RGB references use separately specified literal top-down device boxes.
All requests use the default opaque white background, full rendering at 72 dpi.
"""
import struct

from fixturegen.font import i16, simple, truetype, u16, u32
from fixturegen.pdf import document, stream


CASES = ("fresh-h2-position", "fresh-h5-right-clip", "fresh-h9-chunk-crossing")

# Page side, glyph height in 1000-unit font space, PDF text origin x/y.
_PDF_SPECS = {
    "fresh-h2-position": (256, 200, 12, 17),
    "fresh-h5-right-clip": (256, 500, 252, 241),
    "fresh-h9-chunk-crossing": (512, 900, 254, 252),
}

# Independently fixed top-down ink bounds at 72 dpi: left, top, right, bottom.
# These are not obtained by parsing the PDF or transforming _PDF_SPECS.
_DEVICE_INK = {
    "fresh-h2-position": (12, 237, 16, 239),
    "fresh-h5-right-clip": (252, 10, 256, 15),
    "fresh-h9-chunk-crossing": (254, 251, 258, 260),
}


def _font(height):
    # The existing scene_index.font_objects recipe, narrowed to one encoded
    # glyph 33 -> ID 1 and parameterized only by the integer rectangle height.
    glyph = simple([(0, 0, True), (400, 0, True),
                    (400, height, True), (0, height, True)])
    glyph = glyph[:2] + struct.pack(">hhhh", 0, 0, 400, height) + glyph[10:]
    glyph += bytes(len(glyph) % 2)
    sub = (u16(4) + u16(32) + u16(0) + u16(4) + u16(4) + u16(1) + u16(0)
           + u16(33) + u16(65535) + u16(0) + u16(33) + u16(65535)
           + i16(-32) + i16(1) + u16(0) + u16(0))
    program = truetype(table_overrides={
        b"glyf": glyph,
        b"loca": u32(0) + u32(0) + u32(len(glyph)),
        b"maxp": u32(0x10000) + u16(2) + bytes(26),
        b"hhea": u32(0x10000) + bytes(30) + u16(2),
        b"hmtx": (u16(400) + i16(0)) * 2,
        b"cmap": u16(0) + u16(1) + u16(3) + u16(1) + u32(12) + sub,
        b"post": u32(0x30000) + bytes(28),
    })
    return [
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Rect /FirstChar 33 /LastChar 33 "
        b"/Widths [400] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>",
        b"<< /Type /FontDescriptor /FontName /Rect /Flags 32 /FontBBox [0 0 400 %d] "
        b"/ItalicAngle 0 /Ascent %d /Descent 0 /CapHeight %d /StemV 50 /FontFile2 7 0 R >>"
        % (height, height, height),
        stream(program, b"/Length1 %d" % len(program)),
    ]


def fixture(name):
    if name not in CASES:
        raise ValueError("unknown fresh glyph reach fixture")
    side, height, x, y = _PDF_SPECS[name]
    resources = (b"<< /Font << /F 5 0 R >> /ExtGState "
                 b"<< /Paint << /ca 1 /TK false >> >> >>")
    content = (b"/Paint gs 0 g\nBT /F 10 Tf\n1 0 0 1 %d %d Tm <21> Tj\nET\n"
               % (x, y))
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources "
         % (side, side)) + resources + b" /Contents 4 0 R >>",
        stream(content),
        *_font(height),
    ])
    left, top, right, bottom = _DEVICE_INK[name]
    pixels = bytearray(b"\xff" * (side * side * 3))
    for row in range(top, bottom):
        start = (row * side + left) * 3
        pixels[start:start + (right - left) * 3] = b"\0" * ((right - left) * 3)
    return pdf, b"P6\n%d %d\n255\n" % (side, side) + bytes(pixels)
