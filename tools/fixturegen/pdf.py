"""Minimal, independent PDF construction and analytic raster oracles."""
from __future__ import annotations

HEADER = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"


def stream(content: bytes, extra: bytes = b"") -> bytes:
    """A stream object body with an exact direct /Length."""
    return b"<< /Length %d %s>>\nstream\n" % (len(content), extra) + content + b"\nendstream"


def document(objects: list[bytes], trailer_extra: bytes = b"", tail: bytes = b"") -> bytes:
    """Objects numbered from 1, a classic xref table and a trailer (ISO 32000-2 7.5)."""
    data = bytearray(HEADER)
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(data))
        data += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(data)
    data += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for offset in offsets:
        data += b"%010d 00000 n \n" % offset
    data += b"trailer\n<< /Size %d /Root 1 0 R %s>>\n" % (len(objects) + 1, trailer_extra)
    data += b"startxref\n%d\n%%%%EOF\n" % xref + tail
    return bytes(data)


def single_page(contents: list[bytes], media_box: bytes = b"[0 0 64 64]", page_extra: bytes = b"",
                resources: bytes = b"<< >>", inherited_box: bool = False, tail: bytes = b"") -> bytes:
    """Catalog (1), Pages (2), Page (3) and content streams (4...)."""
    box = b"/MediaBox " + media_box + b" "
    refs = [b"%d 0 R" % (4 + i) for i in range(len(contents))]
    contents_entry = refs[0] if len(refs) == 1 else b"[" + b" ".join(refs) + b"]"
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 " + (box if inherited_box else b"") + b">>",
        b"<< /Type /Page /Parent 2 0 R " + (b"" if inherited_box else box)
        + b"/Resources " + resources + b" /Contents " + contents_entry + b" " + page_extra + b">>",
        *[stream(content) for content in contents],
    ], tail=tail)


def opaque_rectangles(width: int, height: int, rectangles, background=(255, 255, 255)) -> bytes:
    """A binary PPM: background plus opaque, pixel-aligned rectangles.

    Geometry is given in PDF user space with a bottom-left origin at 72 dpi
    and painted in order; coordinates are clipped to the page.
    """
    pixels = bytearray(bytes(background) * width * height)
    for (x, y, w, h), color in rectangles:
        x0, x1 = sorted((x, x + w))
        y0, y1 = sorted((y, y + h))
        for py in range(max(0, y0), min(height, y1)):
            row = (height - 1 - py) * width
            for px in range(max(0, x0), min(width, x1)):
                pixels[(row + px) * 3:(row + px) * 3 + 3] = bytes(color)
    return b"P6\n%d %d\n255\n" % (width, height) + bytes(pixels)
