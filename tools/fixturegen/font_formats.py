"""Legacy font format probes with independent glyph-name and rectangle controls.

OpenType post versions 2.0/2.5 and PDF 9.6.5.4. Version 2.5 uses the
historical 0x00025000 tag and signed offsets into standard Macintosh names.
These original fonts share outlines, metrics and cmap bytes within each pair.
"""
from .font import cff_font, truetype, u16, u32
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
    # Preserve the diagnostic BMP and isolated numeric-code pairs. The
    # supplementary subject maps the literal PDF byte to a different glyph,
    # so fallback to that byte cannot satisfy its independent rectangle oracle.
    for platform, encoding, label in ((3, 10, 'windows'), (0, 4, 'unicode')):
        for mode, code, name in (('bmp', 65, b'A'), ('ucs4', 0x1f600, b'u1F600')):
            for fmt in (8, 12):
                mapping = {code: 1}
                if mode != 'bmp':
                    mapping[65] = 2
                if fmt == 8:
                    cmap = mixed_cmap(mapping, platform, encoding)
                else:
                    cmap = text_cmap_records([(3, 10, mapping)])
                    cmap = cmap[:4] + u16(platform) + u16(encoding) + cmap[8:]
                program = truetype(table_overrides={b'cmap': cmap, b'post': u32(0x30000) + bytes(28)})
                font_encoding = b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /' + name + b'] >>'
                # This format-12 control was shared by both diagnostic
                # supplementary candidates. Keep its original ID and bytes.
                case_mode = 'utf16' if fmt == 12 and mode == 'ucs4' else mode
                suffix = '-isolated' if mode != 'bmp' else ''
                yield case(f'cmap{fmt}-{label}-{case_mode}{suffix}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                           source_objects(program, font_encoding), FONT, [((4, 4, 10, 20), BLACK)],
                           requirement=REQUIREMENT)


def cff_objects(program, cid):
    descriptor_at, stream_at = (7, 8) if cid else (6, 7)
    if cid:
        fonts = [b'<< /Type /Font /Subtype /Type0 /BaseFont /LegacyCFF /Encoding /Identity-H /DescendantFonts [6 0 R] >>',
                 b'<< /Type /Font /Subtype /CIDFontType0 /BaseFont /LegacyCFF /CIDSystemInfo '
                 b'<< /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 600 /FontDescriptor 7 0 R >>']
    else:
        fonts = [b'<< /Type /Font /Subtype /Type1 /BaseFont /LegacyCFF /FirstChar 65 /LastChar 65 '
                 b'/Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>']
    descriptor = (b'<< /Type /FontDescriptor /FontName /LegacyCFF /Flags %d /FontBBox [0 0 400 800] '
                  b'/ItalicAngle 0 /Ascent 800 /Descent 0 /CapHeight 400 /StemV 50 /FontFile3 %d 0 R >>'
                  % (4 if cid else 32, stream_at))
    assert len(fonts) + 5 == descriptor_at
    return [*fonts, descriptor, stream(program, b'/Subtype /OpenType')]


def cff_matrix_cases():
    # OpenType 1.8 listed Font DICT FontMatrix; 1.8.2 deprecated it and
    # current CFF2 has only Private in Font DICT. Keep this legacy case as a
    # diagnostic, using current ignore-unknown interpretation as one explicit
    # hypothesis. The second ordinary rectangle isolates matrix application.
    for cid in (False, True):
        for variant, label in (('fd4', 'none'), ('fd-matrix', 'matrix')):
            program = truetype(cff=cff_font(conforming=True, cff2=True, variant=variant), cff2=True)
            text = b'<0001>' if cid else b'(A)'
            route = 'cid' if cid else 'simple'
            yield case(f'cff2-fd-{label}-{route}', b'BT /F 50 Tf 1 0 0 1 4 4 Tm ' + text + b' Tj ET',
                       cff_objects(program, cid), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement='font-cff', size=64)
    # CFF1 retains its Font DICT matrix. CID17 selects GID1: the underlying
    # 100x200 subroutine, scaled by 2 then by 50/1000, is also 10x20 pixels.
    program = truetype(cff=cff_font(conforming=True, cid=True, variant='fd-array'))
    yield case('cff1-fd-matrix-cid-control', b'BT /F 50 Tf 1 0 0 1 4 4 Tm <0011> Tj ET',
               cff_objects(program, True), FONT, [((4, 4, 10, 20), BLACK)],
               requirement='font-cff', size=64)
    # Keep the original omission/composition observations above unchanged.
    # BCD .001/.002 matrices distinguish explicit Top DICT composition from
    # interpreting a Font DICT matrix as an absolute scale when Top omits it.
    top = b'\x1e\xa0\x01\xff\x8b\x8b\x1e\xa0\x01\xff\x8b\x8b'
    small = b'\x1e\xa0\x02\xff\x8b\x8b\x1e\xa0\x02\xff\x8b\x8b'
    variants = [
        ('cff1-fd-matrix-explicit-top-cid', False, 'fd-array', top, None),
        ('cff2-fd-matrix-explicit-top-cid', True, 'fd-matrix', top, None),
        ('cff2-fd-small-matrix-cid', True, 'fd-matrix', None, small),
    ]
    for name, cff2, variant, top_matrix, fd_matrix in variants:
        program = truetype(cff=cff_font(conforming=True, cid=not cff2, cff2=cff2, variant=variant,
                                       top_matrix=top_matrix, fd_matrix=fd_matrix), cff2=cff2)
        text = b'<0001>' if cff2 else b'<0011>'
        yield case(name, b'BT /F 50 Tf 1 0 0 1 4 4 Tm ' + text + b' Tj ET',
                   cff_objects(program, True), FONT, [((4, 4, 10, 20), BLACK)],
                   requirement='font-cff', size=64)
    for scale in (1, 2):
        width, height = 10*scale, 20*scale
        yield case(f'cff-matrix-rectangle-{scale}', b'0 g 4 4 %d %d re f' % (width, height),
                   rectangles=[((4, 4, width, height), BLACK)], requirement='font-cff', size=64)


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
    for mode in ('direct', 'direct-unnamed'):
        for version, label in ((0x40000, 'post4'), (0x30000, 'post3-control')):
            yield case(f'{label}-{mode}', b'BT /F 100 Tf 1 0 0 1 4 4 Tm (A) Tj ET',
                       font_objects(mode, version), FONT, [((4, 4, 10, 20), BLACK)],
                       requirement=REQUIREMENT)
    yield from cmap_cases()
    yield from cff_matrix_cases()
