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


def post(version, named=True):
    # GIDs 0..4 name .notdef, A, .null, B, space. Negative offsets are
    # exercised even when the displayed glyph's offset is positive.
    names = (0, 36, 1, 37, 3)
    header = u32(version) + bytes(28)
    if version == 0x30000:
        return header
    if version == 0x40000:
        # Apple post-4 has no count field; maxp determines its uint16 array.
        # FFFF denotes no associated code. Unicode cmap selection is the
        # subject here, not an inferred spelling of generated glyph names.
        codes = (0xffff, 0x0041 if named else 0xffff, 0xffff, 0xffff, 0xffff)
        return header + b''.join(u16(code) for code in codes)
    header += u16(len(names))
    if version == 0x25000:
        return header + bytes((name-glyph) % 256 for glyph, name in enumerate(names))
    return header + b''.join(u16(name) for name in names)


def font_objects(mode, version):
    records = [(3, 1, {65: 1})] if mode.startswith('direct') else [(3, 1, {66: 2}), (1, 0, {65: 2})]
    program = truetype(table_overrides={b'post': post(version, mode != 'direct-unnamed'), b'cmap': text_cmap_records(sorted(records))})
    encoding = b'/WinAnsiEncoding' if mode != 'differences' else b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /A] >>'
    return source_objects(program, encoding)


def source_objects(program, encoding):
    font = (b'<< /Type /Font /Subtype /TrueType /BaseFont /LegacyName /FirstChar 65 /LastChar 65 '
            b'/Widths [600] /Encoding ' + encoding + b' /FontDescriptor 6 0 R >>')
    descriptor = (b'<< /Type /FontDescriptor /FontName /LegacyName /Flags 32 /FontBBox [0 0 300 300] '
                  b'/ItalicAngle 0 /Ascent 300 /Descent 0 /CapHeight 200 /StemV 50 /FontFile2 7 0 R >>')
    return [font, descriptor, stream(program, b'/Length1 %d' % len(program))]


def post4_name_cases():
    # Apple specifies an "a" prefix followed by hexadecimal code characters,
    # but does not fix padding or letter case. These are observations of each
    # candidate, paired with an exact literal post-2 name; not normative names.
    for label, code, name in [('short-digit', 0x41, b'a41'), ('padded-digit', 0x41, b'a0041'),
                              ('short-upper', 0x4a, b'a4A'), ('short-lower', 0x4a, b'a4a'),
                              ('padded-upper', 0x4a, b'a004A'), ('padded-lower', 0x4a, b'a004a')]:
        for version, tag in [(0x40000, 'post4-name'), (0x20000, 'post2-name-control')]:
            header = u32(version) + bytes(28)
            if version == 0x40000:
                table = header + b''.join(u16(n) for n in (0xffff, code, 0xffff, 0xffff, 0xffff))
            else:
                table = header + u16(5) + b''.join(u16(n) for n in (0, 258, 0, 0, 0))
                table += bytes([len(name)]) + name
            cmap = text_cmap_records([(3, 1, {66: 2})])
            program = truetype(table_overrides={b'post': table, b'cmap': cmap})
            encoding = b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /' + name + b'] >>'
            yield case(f'{tag}-{label}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                       source_objects(program, encoding), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement=REQUIREMENT)


def cases():
    # GID 1 is the existing independent 100 x 200 unit rectangle. At 100 pt,
    # its visible area is 10 x 20 pixels, independently of the 600-unit advance.
    for mode in ('direct', 'name-fallback', 'differences'):
        for version, label in ((0x25000, 'post25'), (0x20000, 'post2-control')):
            yield case(f'{label}-{mode}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                       font_objects(mode, version), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement=REQUIREMENT)
    yield from post4_name_cases()
    yield case('ordinary-rectangle', b'0 g 4 4 10 20 re f',
               rectangles=[((4, 4, 10, 20), BLACK)], requirement=REQUIREMENT)
    for mode in ('direct', 'direct-unnamed'):
        for version, label in ((0x40000, 'post4'), (0x30000, 'post3-control')):
            yield case(f'{label}-{mode}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                       font_objects(mode, version), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement=REQUIREMENT)
