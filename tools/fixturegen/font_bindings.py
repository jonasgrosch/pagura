"""Intact program bytes with contradictory CID embedding labels.

The original CFF charset/GID order and rectangle program are independent of
PDF labels. Controls differ only in those labels; no engine creates expected
pixels. Genuine Type 1 dictionary ordering does not define a CID order.
"""
from dataclasses import replace

from .font import type1_font
from .font_collections import NAMES, container, face, objects
from .model import OPEN, Case
from .paint import BLACK, FONT, case
from .pdf import stream

AREA = 'font_bindings'


def cases():
    cff_face = face('cff', 0, True)
    cff = cff_face[1][b'CFF ']
    otf = container([cff_face])
    glyf = container([face('glyf', 0, True)])
    for name, program, key, subtype, control_kind in [
        ('cff-label', cff, b'FontFile3', b'Type1C', 'cff'),
        ('cff-key', cff, b'FontFile', None, 'cff'),
        ('otf-label', otf, b'FontFile3', b'CIDFontType0C', 'cff'),
        ('glyf-label', glyf, b'FontFile3', b'CIDFontType0C', 'glyf'),
    ]:
        text = b'<0001>' if control_kind == 'glyf' else b'<0011>'
        content = b'BT /F 50 Tf 1 0 0 1 4 4 Tm '+text+b' Tj ET'
        canonical = objects(program, control_kind, True, NAMES[0])
        canonical[-1] = stream(program, b'/Length1 %d'%len(program) if control_kind == 'glyf'
                               else b'/Subtype /'+(b'OpenType' if program.startswith(b'OTTO') else b'CIDFontType0C'))
        yield case(name+'-control', content, canonical, FONT, [((4, 4, 5, 10), BLACK)],
                   requirement='text-embedded-fonts', size=64)
        damaged = objects(program, 'cff', True, NAMES[0])
        damaged[-2] = damaged[-2].replace(b'/FontFile3', b'/'+key)
        damaged[-1] = stream(program, b'/Subtype /'+subtype if subtype else b'')
        original = case(name, content, damaged, FONT, [((4, 4, 5, 10), BLACK)],
                        requirement='text-embedded-fonts', size=64)
        yield replace(original, repairs=('font.program-binding',), strict_valid=False)
    for name, program in [('type1-cid-order', type1_font()), ('damaged-cff', b'\x01\x00\x04\x01\x00')]:
        damaged = objects(program, 'cff', True, NAMES[0])
        damaged[-2] = damaged[-2].replace(b'/FontFile3', b'/FontFile')
        damaged[-1] = stream(program)
        original = case(name, b'BT /F 50 Tf <0001> Tj ET', damaged, FONT,
                        requirement='text-embedded-fonts', size=64)
        yield Case(name, original.pdf, OPEN, original.requirements,
                   'No recoverable complete program with a defined CID/GID order',
                   extra={'render_error': 'Syntax'}, strict_valid=False)
    damaged = objects(glyf, 'cff', True, NAMES[0])
    damaged[1] = damaged[1].replace(b'/DW 600', b'/CIDToGIDMap /Ambiguous /DW 600')
    damaged[-1] = stream(glyf, b'/Subtype /CIDFontType0C')
    original = case('glyf-conflicting-map', b'BT /F 50 Tf <0001> Tj ET', damaged, FONT,
                    requirement='text-embedded-fonts', size=64)
    yield Case('glyf-conflicting-map', original.pdf, OPEN, original.requirements,
               'A conflicting CID map cannot be discarded to reinterpret a program binding',
               extra={'render_error': 'Syntax'}, strict_valid=False)
