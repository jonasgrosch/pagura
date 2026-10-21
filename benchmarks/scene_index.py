#!/usr/bin/env python3
"""Small old/new root-glyph-hull experiment; never a release/competitor claim.

Run only through the coordinator's exclusive runner. Both binaries must contain
the identical scene_index_profile example. Preparation, hashes, oracle creation,
verification and emission warm the filesystem; these are not cold-I/O samples.
"""
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_render import ROOT, expected_image, run_experiment, sha, source_identity
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



def main():
    run_experiment(__file__, description=__doc__, experiment="root-glyph-hull-cache",
                   cases=CASES, cells=CELLS, target_case="sparse64", fixture=fixture,
                   target_scope="explicit /TK false non-knockout text; 64 distinct glyph sources per direct root run",
                   limitations=("white dense control tests full-page hull fallback, not typical black text cost",
                                "no default-knockout text benefit is claimed"))


if __name__ == "__main__":
    main()
