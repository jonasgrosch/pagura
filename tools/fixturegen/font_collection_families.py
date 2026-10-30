"""Table124 selected-family guards; actual CFF charset never replaces CID2 maps.

Valid glyf twins map CID17 to the authored empty GID0 or rectangle GID1.
Contradictory CFF/CID2 twins use the same two maps and must both refuse,
rather than silently drawing CFF charset CID17->GID1 in both cases.
"""
from dataclasses import replace

from .font import cff_font
from .font_collections import NAMES, container, face, objects
from .model import OPEN, Case
from .paint import BLACK, FONT, case
from .pdf import stream

AREA = 'font_collection_families'


def named_cff_face(index):
    flavor, tables = face('cff', index, True)
    unit = b'\x1e\xa0\x02\xff' if index else b'\x1e\xa0\x01\xff'
    tables[b'CFF '] = cff_font(conforming=True, top_matrix=unit+b'\x8b\x8b'+unit+b'\x8b\x8b')
    return flavor, tables


def mapped(fonts, cid_subtype, gid):
    fonts = list(fonts)
    fonts[1] = fonts[1].replace(b'/Subtype /CIDFontType0', b'/Subtype /'+cid_subtype)
    if b'/CIDToGIDMap /Identity' in fonts[1]:
        fonts[1] = fonts[1].replace(b'/CIDToGIDMap /Identity', b'/CIDToGIDMap 9 0 R')
    else:
        fonts[1] = fonts[1].replace(b'/DW 600', b'/CIDToGIDMap 9 0 R /DW 600')
    mapping = bytearray(36)
    mapping[34:36] = gid.to_bytes(2, 'big')
    return [*fonts, stream(bytes(mapping))]


def cases():
    programs = {
        'glyf': container([face('glyf', n, True) for n in (0, 1)], 1),
        'cid-cff': container([face('cff', n, True) for n in (0, 1)], 1),
        'name-cff': container([named_cff_face(n) for n in (0, 1)], 1),
    }
    def emit(name, fonts, text, valid, marked=True):
        content = b'0 0 1 rg 0 0 64 64 re f 0 g BT /F 50 Tf 1 0 0 1 4 4 Tm '+text+b' Tj ET'
        original = case(name, content, fonts, FONT,
                        [((0, 0, 64, 64), (0, 0, 255))]+([((4, 4, 10, 20), BLACK)] if marked else []),
                        requirement='text-embedded-fonts', size=64)
        if valid:
            return replace(original, repairs=('font.collection-face',), strict_valid=False)
        return Case(name, original.pdf, OPEN, original.requirements,
                    'Contradictory selected outline family/namespace; explicit glyph maps cannot be discarded',
                    strict_valid=False, extra={'render_error': 'Syntax'})
    for gid in (0, 1):
        yield emit('glyf-cid2-map'+str(gid), mapped(objects(programs['glyf'], 'glyf', True, NAMES[1]), b'CIDFontType2', gid), b'<0011>', True, marked=bool(gid))
        for kind in ('cid-cff', 'name-cff'):
            yield emit(kind+'-as-cid2-map'+str(gid), mapped(objects(programs[kind], 'cff', True, NAMES[1]), b'CIDFontType2', gid), b'<0011>', False)
        yield emit('glyf-as-cid0-map'+str(gid), mapped(objects(programs['glyf'], 'cff', True, NAMES[1]), b'CIDFontType0', gid), b'<0011>', False)
    for kind in ('glyf', 'cid-cff'):
        fonts = objects(programs[kind], 'cff', False, NAMES[1])
        # Unknown symbolic families have no eligible substitute; the guard's
        # original Syntax refusal must survive the existing fallback policy.
        fonts[-2] = fonts[-2].replace(b'/Flags 32', b'/Flags 4')
        yield emit(kind+'-as-simple-type1', fonts, b'(A)', False)
    yield emit('name-cff-simple-type1', objects(programs['name-cff'], 'cff', False, NAMES[1]), b'(A)', True)
    yield emit('name-cff-cid0-direct-gid', objects(programs['name-cff'], 'cff', True, NAMES[1]), b'<0001>', True)
    yield emit('cid-cff-cid0-charset', objects(programs['cid-cff'], 'cff', True, NAMES[1]), b'<0011>', True)
