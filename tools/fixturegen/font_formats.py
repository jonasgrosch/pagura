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
    return source_objects(program, encoding)


def source_objects(program, encoding):
    font = (b'<< /Type /Font /Subtype /TrueType /BaseFont /LegacyName /FirstChar 65 /LastChar 65 '
            b'/Widths [600] /Encoding ' + encoding + b' /FontDescriptor 6 0 R >>')
    descriptor = (b'<< /Type /FontDescriptor /FontName /LegacyName /Flags 32 /FontBBox [0 0 300 300] '
                  b'/ItalicAngle 0 /Ascent 300 /Descent 0 /CapHeight 200 /StemV 50 /FontFile2 7 0 R >>')
    return [font, descriptor, stream(program, b'/Length1 %d' % len(program))]


def mixed_cmap(mapping, platform, encoding):
    """Original format-8 table: literal code groups and high-word bitmap."""
    is32 = bytearray(8192)
    for code in mapping:
        if code > 0xffff:
            high = code >> 16
            is32[high // 8] |= 1 << (7 - high % 8)
    groups = b''.join(u32(code) + u32(code) + u32(glyph) for code, glyph in sorted(mapping.items()))
    sub = u16(8) + u16(0) + u32(8208 + len(groups)) + u32(0) + is32 + u32(len(mapping)) + groups
    return u16(0) + u16(1) + u16(platform) + u16(encoding) + u32(12) + sub


def cmap_cases():
    # The format-8 prose describes UTF-16 and also gives a conflicting UCS-4
    # high-word example. Keep both supplementary candidates separate; neither
    # outcome alone adjudicates the encoding rule. The BMP subject is unambiguous.
    for platform, encoding, label in ((3, 10, 'windows'), (0, 4, 'unicode')):
        for mode, code, name in (('bmp', 65, b'A'), ('utf16', 0xd83dde00, b'u1F600'),
                                 ('ucs4', 0x1f600, b'u1F600')):
            for isolated in ([False] if mode == 'bmp' else [False, True]):
                for fmt in (8, 12):
                    # Both supplementary encodings share the same format-12
                    # control; keep only one extra copy for each platform.
                    if isolated and fmt == 12 and mode == 'ucs4':
                        continue
                    mapping = {code if fmt == 8 else (65 if mode == 'bmp' else 0x1f600): 1}
                    if mode != 'bmp':
                        # GID2 is a different curved glyph. Literal-byte fallback
                        # must no longer pass the supplementary selection oracle.
                        mapping[65] = 2 if isolated else 1
                    if fmt == 8:
                        cmap = mixed_cmap(mapping, platform, encoding)
                    else:
                        cmap = text_cmap_records([(3, 10, mapping)])
                        cmap = cmap[:4] + u16(platform) + u16(encoding) + cmap[8:]
                    program = truetype(table_overrides={b'cmap': cmap, b'post': u32(0x30000) + bytes(28)})
                    font_encoding = b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /' + name + b'] >>'
                    suffix = '-isolated' if isolated else ''
                    yield case(f'cmap{fmt}-{label}-{mode}{suffix}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                               source_objects(program, font_encoding), FONT, [((4, 4, 10, 20), BLACK)],
                               requirement=REQUIREMENT)


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
    yield from cmap_cases()
