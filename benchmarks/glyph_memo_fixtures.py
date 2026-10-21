"""Independent two-source orthogonal text controls for the root-hull memo.

This is a deliberately scoped synthetic page, not representative document text.
Imported by a benchmark entry point after paired_render establishes tools on
sys.path. No renderer is used to construct the font or expected pixels.
"""
import struct

from fixturegen.font import simple, truetype, u16, u32, i16
from fixturegen.pdf import document, stream, opaque_rectangles


CASES = ("outline2", "outline2-knockout", "outline2-late64")
WIDTH, HEIGHT = 512, 1024
LINES = 64
TEXT = b"LT" * 32


def _glyph(points):
    # simple() writes a generic bbox; use this independently specified outline's
    # actual bounds and pad the glyph to an even loca offset.
    encoded = simple([(x, y, True) for x, y in points])
    encoded = encoded[:2] + struct.pack(">hhhh", 0, 0, 500, 700) + encoded[10:]
    return encoded + bytes(len(encoded) % 2)


def _font_objects():
    # L: one concave, closed, all-on-curve polygon. T: another such polygon.
    # Both occupy [0,500] x [0,700] in a 1000-unit em; advance is 600.
    letter_l = _glyph(((0, 0), (500, 0), (500, 100), (100, 100),
                       (100, 700), (0, 700)))
    letter_t = _glyph(((200, 0), (300, 0), (300, 600), (500, 600),
                       (500, 700), (0, 700), (0, 600), (200, 600)))
    # Three format-4 segments map Unicode L->1, T->2, sentinel->0.
    sub = (u16(4) + u16(40) + u16(0) + u16(6) + u16(4) + u16(1) + u16(2)
           + u16(76) + u16(84) + u16(65535) + u16(0)
           + u16(76) + u16(84) + u16(65535)
           + i16(-75) + i16(-82) + i16(1) + u16(0) * 3)
    program = truetype(table_overrides={
        b"glyf": letter_l + letter_t,
        b"loca": b"".join(u32(offset) for offset in
                          (0, 0, len(letter_l), len(letter_l) + len(letter_t))),
        b"maxp": u32(0x10000) + u16(3) + bytes(26),
        b"hhea": u32(0x10000) + bytes(30) + u16(3),
        b"hmtx": (u16(600) + i16(0)) * 3,
        b"cmap": u16(0) + u16(1) + u16(3) + u16(1) + u32(12) + sub,
        b"post": u32(0x30000) + bytes(28),
    })
    return [
        b"<< /Type /Font /Subtype /TrueType /BaseFont /OrthogonalLT "
        b"/FirstChar 76 /LastChar 84 /Widths [" + b"600 " * 9
        + b"] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>",
        b"<< /Type /FontDescriptor /FontName /OrthogonalLT /Flags 32 "
        b"/FontBBox [0 0 500 700] /ItalicAngle 0 /Ascent 700 /Descent 0 "
        b"/CapHeight 700 /StemV 100 /FontFile2 7 0 R >>",
        stream(program, b"/Length1 %d" % len(program)),
    ]


def fixture(name):
    """Return (PDF bytes, independently derived RGB8 PPM) for one named case."""
    if name not in CASES:
        raise ValueError("unknown orthogonal text fixture: " + str(name))
    # Omitting TK selects PDF's true default. The two remaining cases explicitly
    # choose false; quarter alpha makes both modes enter the compositor.
    knockout = b"" if name == "outline2-knockout" else b"/TK false "
    resources = (b"<< /Font << /F 5 0 R >> /ExtGState << /Paint << /ca .25 "
                 + knockout + b">> >> >>")
    content = bytearray()
    if name == "outline2-late64":
        # Exactly 64 ordinary root path commands precede text. Their nonempty,
        # disjoint white-on-white 1px squares preserve the independent oracle.
        # The current interpreter retains each f as one Command::Path.
        content.extend(b"1 g\n")
        for index in range(64):
            content.extend(f"{index * 2} 0 1 1 re f\n".encode())
    content.extend(b"/Paint gs 0 g\n")
    rectangles = []
    gray = (191, 191, 191)
    for line in range(LINES):
        x, y = 16, 16 + line * 14
        # BT/ET reset the interpreter's current glyph run. This yields 64 root
        # runs for TK false, with two sources and 64 placements in each run.
        content.extend(f"BT /F 10 Tf 1 0 0 1 {x} {y} Tm ".encode())
        content.extend(b"<" + TEXT.hex().encode() + b"> Tj ET\n")
        for index, letter in enumerate(TEXT):
            left = x + index * 6
            # An independently specified disjoint rectangle partition of each
            # shape, in final pixels. It does not read font bytes or contours.
            pieces = ((0, 0, 1, 7), (1, 0, 4, 1)) if letter == ord("L") else (
                (2, 0, 1, 6), (0, 6, 5, 1))
            for dx, dy, width, height in pieces:
                rectangles.append(((left + dx, y + dy, width, height), gray))
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources "
        % (WIDTH, HEIGHT) + resources + b" /Contents 4 0 R >>",
        stream(bytes(content)), *_font_objects(),
    ])
    return pdf, opaque_rectangles(WIDTH, HEIGHT, rectangles)


def image_only():
    """Opaque backdrop, quarter-alpha black image; every RGB channel is 191."""
    size = 512
    resources = b"<< /XObject << /Im 5 0 R >> /ExtGState << /Paint << /ca .25 >> >> >>"
    image = stream(bytes(32 * 32 * 3),
                   b"/Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8 ")
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 512 512] /Resources " + resources + b" /Contents 4 0 R >>",
        stream(b"/Paint gs q 512 0 0 512 0 0 cm /Im Do Q"), image,
    ])
    return pdf, opaque_rectangles(size, size, [((0, 0, size, size), (191, 191, 191))])
