"""First-image JPX header and channel controls, independent of all readers.

PDF 7.4.9/8.9.5; T.801 M.9.2.2/.7/.8 and M.11.6/.7/.7.6/.7.7;
Cor.4 M.11.1/Table M.14. The original diagnostic uses one codestream/layer;
one additional control includes an unrelated later image.
First-image fields never duplicate JP2-header entries. No file/network provider,
composition, animation, fragments or nonidentity registration is implied.
"""

import hashlib
import struct
from dataclasses import replace
from fractions import Fraction as F

from . import jpx
from .model import Vector
from .paint import image_case
from .paint_jpx_integration import definitions

AREA = 'jpx_layout_probe'
REQUIREMENT = 'jpx-pdf-contract'
RGB = [(0, 64, 255), (255, 128, 0), (64, 192, 32), (128, 32, 192)]


def dimensions(components):
    return jpx.box(b'ihdr', struct.pack('>IIH4B', 1, 4, components, 7, 7, 0, 0))


def color(approximation=1):
    return jpx.box(b'colr', bytes([1, 0, approximation]) + struct.pack('>I', 16))


def requirements(features, display_mask=None):
    # Part1 unrestricted codestream plus the explicitly declared file features.
    features = [5, *features]
    mask = (1 << len(features)) - 1
    return (struct.pack('>BBBH', 1, mask, mask if display_mask is None else display_mask, len(features))
            + b''.join(struct.pack('>HB', feature, 1 << i) for i, feature in enumerate(features))
            + b'\0\0')


def container(raw, header, extra=b'', features=(), *, display_mask=None):
    return (jpx.box(b'jP  ', b'\r\n\x87\n')
            + jpx.box(b'ftyp', b'jpx ' + struct.pack('>I', 1) + b'jpx jpxb')
            + jpx.box(b'rreq', requirements(features, display_mask))
            + jpx.box(b'jp2h', header) + extra + jpx.box(b'jp2c', raw))


def jp2(raw, header):
    return (jpx.box(b'jP  ', b'\r\n\x87\n')
            + jpx.box(b'ftyp', b'jp2 ' + bytes(4) + b'jp2 ')
            + jpx.box(b'jp2h', header) + jpx.box(b'jp2c', raw))


def paired(name, raw, header, extra, flat_extra, colors, *, components=3, features=(), paint_mode=0, subject_data=None, color_box=color):
    options = {'matrix': (4, 0, 0, 1, 2, 2), 'requirement': REQUIREMENT}
    dictionary = b'/Filter /JPXDecode /SMaskInData %d' % paint_mode
    variants = [
        ('', 'subject', container(raw, header, extra, features) if subject_data is None else subject_data),
        ('-codec-control', 'codec-control', jp2(raw, dimensions(components) + color_box(0) + flat_extra)),
        ('-brand-control', 'brand-control', container(raw, dimensions(components) + color_box(1) + flat_extra,
                                                    features=features)),
    ]
    for suffix, role, data in variants:
        value = image_case(name + suffix, data, dictionary, [], 4, 1,
                           colors=colors, tolerance=1, **options)
        yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}', 'role': role,
                                    'codestream_sha256': hashlib.sha256(raw).hexdigest()})
    value = image_case(name + '-rgb-control', bytes(v for pixel in colors for v in pixel),
                       b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 4, 1, colors=colors, **options)
    yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}', 'role': 'rgb-control'})


def palette(pixels, depths):
    data = struct.pack('>HB', len(pixels), len(depths)) + bytes(bits - 1 for bits in depths)
    data += b''.join(value.to_bytes((bits + 7) // 8, 'big')
                     for pixel in pixels for value, bits in zip(pixel, depths))
    mapping = b''.join(struct.pack('>HBB', 0, 1, column) for column in range(len(depths)))
    return jpx.box(b'pclr', data) + jpx.box(b'cmap', mapping)


def opaque_cases():
    raw = jpx.codestream([list(channel) for channel in zip(*RGB)], 4, 1)
    header = dimensions(3) + color()
    for name, extra in [('jpch-inherit', jpx.box(b'jpch', b'')),
                        ('jplh-inherit', jpx.box(b'jplh', b'')),
                        ('creg-identity', jpx.box(b'jplh', jpx.box(b'creg', struct.pack('>HHH4B', 1, 1, 0, 1, 1, 0, 0))))]:
        yield from paired(name, raw, header, extra, b'', RGB)
    # M.11.7.1 explicitly permits a colour group when jp2h has no colr.
    yield from paired('cgrp-color', raw, dimensions(3), jpx.box(b'jplh', jpx.box(b'cgrp', color())), b'', RGB)
    raw = jpx.codestream([[0, 1, 2, 3]], 4, 1)
    mapping = palette(RGB, (8, 8, 8))
    yield from paired('jpch-palette', raw, dimensions(1) + color(), jpx.box(b'jpch', mapping), mapping, RGB,
                      components=1)


def alpha_cases():
    pixels = [(0, 0, 0), (63, 126, 189), (126, 63, 252), (255, 255, 255)]
    # Independent source-over arithmetic. These values make premultiplication
    # exactly representable for both 8- and 16-bit alpha, without rounding it.
    fractions = [F(0), F(1, 3), F(2, 3), F(1)]
    composited = [tuple(int(value * opacity + 255 * (1 - opacity) + F(1, 2)) for value in pixel)
                  for pixel, opacity in zip(pixels, fractions)]
    for mode in (1, 2):
        stored = pixels if mode == 1 else [tuple(int(value * opacity) for value in pixel)
                                          for pixel, opacity in zip(pixels, fractions)]
        cdef = definitions([(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, mode, 0)])
        raw = jpx.codestream([list(channel) for channel in zip(*stored)]
                             + [[int(opacity * 255) for opacity in fractions]], 4, 1)
        header = dimensions(4) + color()
        opct = jpx.box(b'opct', bytes([mode - 1]))
        name = 'opct-straight' if mode == 1 else 'opct-premultiplied'
        yield from paired(name, raw, header, jpx.box(b'jplh', opct), cdef, composited,
                          components=4, features=(8 + mode,), paint_mode=mode)
        if mode == 1:
            yield from paired('jplh-cdef', raw, header, jpx.box(b'jplh', cdef), cdef, composited,
                              components=4, features=(9,), paint_mode=1)
            yield from paired('opct-ignored', raw, header, jpx.box(b'jplh', opct), cdef, pixels,
                              components=4, features=(9,), paint_mode=0)
        raw = jpx.codestream([[0, 1, 2, 3]], 4, 1)
        entries = [(*pixel, int(opacity * 65535)) for pixel, opacity in zip(stored, fractions)]
        mapping = palette(entries, (8, 8, 8, 16))
        yield from paired(name + '-palette', raw, dimensions(1) + color() + mapping,
                          jpx.box(b'jplh', opct), mapping + cdef, composited,
                          components=1, features=(8 + mode,), paint_mode=mode)
        if mode == 1:
            # No repeated first-image entry: dimensions in jp2h, palette/map
            # in jpch, colour and opacity in jplh. All use the same codestream.
            layer = jpx.box(b'cgrp', color()) + opct
            yield from paired('combined-first-headers', raw, dimensions(1),
                              jpx.box(b'jpch', mapping) + jpx.box(b'jplh', layer), mapping + cdef, composited,
                              components=1, features=(9,), paint_mode=1)


def boundaries():
    raw = jpx.codestream([[0, 0, 0, 0]] * 3 + [[0, 85, 170, 255]], 4, 1)
    header = dimensions(4) + color()
    opct = jpx.box(b'opct', b'\0')
    cdef = definitions([(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 1, 0)])
    for name, data, reason in [
        ('opct-and-cdef', container(raw, header, jpx.box(b'jplh', opct + cdef), (9,)),
         'T.801 M.11.7/M.11.7.6: opct and cdef are mutually exclusive within a layer'),
        ('opct-location', container(raw, header + opct, features=(9,)),
         'T.801 M.11.7.6: opct is confined to a Compositing Layer Header'),
        ('opct-reserved', container(raw, header, jpx.box(b'jplh', jpx.box(b'opct', b'\3'))),
         'T.801 Table M.33: opacity type3 is reserved'),
        ('opct-excess', container(raw, header, jpx.box(b'jplh', jpx.box(b'opct', b'\0\0')), (9,)),
         'T.801 Table M.33: type0 has no following fields'),
        ('first-image-duplicate', container(raw, header, jpx.box(b'jpch', dimensions(4))
                                            + jpx.box(b'jplh', opct), (9,)),
         'T.801 M.9.2.7: baseline first-image entries cannot duplicate jp2h entries'),
    ]:
        yield jpx.rejection(name, data, (REQUIREMENT,), reason)
    # The table reverses the Otyp==2 field-presence conditions, contrary to
    # the prose/figure and Table M.33; no chroma-key/PDF oracle is guessed.
    reason = ('T.801 M.11.7.6/Table M.34 field-presence contradiction; chroma-key and PDF SMaskInData '
              'interaction remain outside this diagnostic. No invalid/unsupported verdict is assigned.')
    data = container(jpx.codestream([[0, 64, 128, 255]] * 3, 4, 1), dimensions(3) + color(),
                     jpx.box(b'jplh', jpx.box(b'opct', bytes([2, 3, 64, 64, 64]))), (11,))
    yield Vector('chroma-key-unadjudicated', {'.jpx': data, '.note': (reason + '\n').encode()},
                 (REQUIREMENT,), reason, extra={'observation': 'unprobed-boundary'})


def cases():
    yield from opaque_cases()
    yield from alpha_cases()
    yield from boundaries()
    # Only layer/codestream zero is displayed (PDF 7.4.9; T.801 M.9.2.2).
    # Later-image overrides are not duplicate ownership of the first image.
    raw = jpx.codestream([list(channel) for channel in zip(*RGB)], 4, 1)
    header = dimensions(3) + color()
    yield from paired('jpch-dimensions', raw, color(), jpx.box(b'jpch', dimensions(3)), b'', RGB)
    registration = jpx.box(b'creg', struct.pack('>HHH4B', 1, 1, 0, 1, 1, 0, 0))
    first = jpx.box(b'jpch', b'') + jpx.box(b'jplh', registration)
    data = container(raw, header, first, (2,), display_mask=1)
    data += jpx.box(b'jpch', dimensions(1))
    second = jpx.box(b'cgrp', jpx.box(b'colr', bytes([1, 0, 1]) + struct.pack('>I', 17)))
    second += jpx.box(b'creg', struct.pack('>HHH4B', 1, 1, 1, 1, 1, 0, 0))
    data += jpx.box(b'jplh', second)
    data += jpx.box(b'jp2c', jpx.codestream([[255, 0, 255, 0]], 4, 1))
    yield from paired('later-image-headers', raw, header, first, b'', RGB, subject_data=data)
