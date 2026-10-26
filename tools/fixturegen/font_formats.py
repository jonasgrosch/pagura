"""Legacy font format probes with independent glyph-name and rectangle controls.

OpenType post versions 2.0/2.5 and PDF 9.6.5.4. Version 2.5 uses the
historical 0x00025000 tag and signed offsets into standard Macintosh names.
These original fonts share outlines, metrics and cmap bytes within each pair.
"""
from .font import truetype, u16, u32
from .paint import BLACK, FONT, case
from .pdf import stream
from .text import text_cmap_records

AREA = 'font_formats'
REQUIREMENT = 'text-truetype-selection'


def post(version):
    # GIDs 0..4 name .notdef, A, .null, B, space. Negative offsets are
    # exercised even when the displayed glyph's offset is positive.
    names = (0, 36, 1, 37, 3)
    header = u32(version) + bytes(28) + u16(len(names))
    if version == 0x25000:
        return header + bytes((name-glyph) % 256 for glyph, name in enumerate(names))
    return header + b''.join(u16(name) for name in names)


def font_objects(mode, version):
    records = [(3, 1, {65: 1})] if mode == 'direct' else [(3, 1, {66: 2}), (1, 0, {65: 2})]
    program = truetype(table_overrides={b'post': post(version), b'cmap': text_cmap_records(sorted(records))})
    encoding = b'/WinAnsiEncoding' if mode != 'differences' else b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /A] >>'
    font = (b'<< /Type /Font /Subtype /TrueType /BaseFont /LegacyName /FirstChar 65 /LastChar 65 '
            b'/Widths [600] /Encoding ' + encoding + b' /FontDescriptor 6 0 R >>')
    descriptor = (b'<< /Type /FontDescriptor /FontName /LegacyName /Flags 32 /FontBBox [0 0 300 300] '
                  b'/ItalicAngle 0 /Ascent 300 /Descent 0 /CapHeight 200 /StemV 50 /FontFile2 7 0 R >>')
    return [font, descriptor, stream(program, b'/Length1 %d' % len(program))]


def cases():
    # GID 1 is the existing independent 100 x 200 unit rectangle. At 100 pt,
    # its visible area is 10 x 20 pixels, independently of the 600-unit advance.
    for mode in ('direct', 'name-fallback', 'differences'):
        for version, label in ((0x25000, 'post25'), (0x20000, 'post2-control')):
            yield case(f'{label}-{mode}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                       font_objects(mode, version), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement=REQUIREMENT)
    yield case('ordinary-rectangle', b'0 g 4 4 10 20 re f',
               rectangles=[((4, 4, 10, 20), BLACK)], requirement=REQUIREMENT)
