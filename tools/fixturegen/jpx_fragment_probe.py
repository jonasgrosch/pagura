"""Selected JPX metadata-reference pairs, independent of every reader.

T.801 M.9.2.5/.6/.7, M.11.3/.3.1/.4/.9; Cor.4 M.11.1/Table M.14.
All referenced bytes are in this file, in listed order. Reference fragments
precede the selected codestream. No external file or network access is used.
"""

import struct
from dataclasses import replace

from . import content as colour, jpx, jpx_layout_probe as layout
from .paint_jpx import srgb
from .paint_jpx_integration import colr

AREA = 'jpx_fragment_probe'
REQUIREMENT = 'jpx-pdf-contract'


def prefix(header, features=(), *, display_mask=None):
    return (jpx.box(b'jP  ', b'\r\n\x87\n')
            + jpx.box(b'ftyp', b'jpx ' + struct.pack('>I', 1) + b'jpx jpxb')
            + jpx.box(b'rreq', layout.requirements(features, display_mask))
            + jpx.box(b'jp2h', header))


def fragment_list(spans):
    # M.11.3.1/Table M.17: NF u16; OFF u64; LEN u32; DR u16.
    # DR=0 means this JPX file. The offsets address contents, not box headers.
    return jpx.box(b'flst', struct.pack('>H', len(spans))
                   + b''.join(struct.pack('>QIH', offset, length, 0) for offset, length in spans))


def media(chunks, start):
    contents = bytearray()
    spans = []
    for chunk in chunks:
        # Deliberate non-fragment bytes ensure concatenation uses LEN/OFF,
        # rather than the physical byte range between the first/last spans.
        contents += jpx.box(b'free', b'gap')
        spans.append((start + len(contents) + 8, len(chunk)))
        contents += jpx.box(b'mdat', chunk)
    return spans, bytes(contents)


def referenced(raw, kind, chunks):
    # The target box itself need not physically exist. For ihdr the referenced
    # bytes are its fourteen-byte contents; for cgrp they are child-box bytes.
    header, owner = ((layout.color(), b'jpch') if kind == b'ihdr'
                     else (layout.dimensions(3), b'jplh'))
    start = prefix(header)
    placeholder = jpx.box(owner, jpx.box(b'cref', kind + fragment_list([(0, len(c)) for c in chunks])))
    spans, data = media(chunks, len(start) + len(placeholder))
    reference = jpx.box(owner, jpx.box(b'cref', kind + fragment_list(spans)))
    return start + reference + data + jpx.box(b'jp2c', raw)


def later_image(raw):
    # PDF displays the first layer/codestream. Later-image fragmentation is
    # declared in FUAM, while DCM needs only the selected Part 1 codestream.
    start = prefix(layout.dimensions(3) + layout.color(), (2, 13), display_mask=1)
    for index in range(2):
        registration = jpx.box(b'creg', struct.pack('>HHH4B', 1, 1, index, 1, 1, 0, 0))
        start += jpx.box(b'jplh', registration)
    start += jpx.box(b'jp2c', raw)
    later = jpx.codestream([[255, 0, 255, 0]] * 3, 4, 1)
    table = jpx.box(b'ftbl', fragment_list([(0, len(later))]))
    spans, data = media([later], len(start) + len(table))
    return start + jpx.box(b'ftbl', fragment_list(spans)) + data


def cases():
    raw = jpx.codestream([list(channel) for channel in zip(*layout.RGB)], 4, 1)
    header = layout.dimensions(3) + layout.color()
    dimensions = layout.dimensions(3)[8:]
    color = layout.color()
    subjects = [
        ('reference-image-header', referenced(raw, b'ihdr', [dimensions])),
        ('reference-split-color-group', referenced(raw, b'cgrp', [color[:3], color[3:]])),
        ('later-image-fragments', later_image(raw)),
    ]
    for name, data in subjects:
        for value in layout.paired(name, raw, header, b'', b'', layout.RGB, subject_data=data):
            validity = {'jpx_validity': 'baseline-first-image'} if value.extra['role'] == 'subject' else {}
            yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}',
                                        **validity,
                                        'evidence_scope': 'Independent internal-fragment diagnostic; runtime evidence is separate'})

    # A plain sRGB reference can also pass if a reader guesses DeviceRGB.
    # The existing restricted matrix/TRC profile instead interprets the stored
    # samples as linear RGB; the scalar sRGB transfer supplies the oracle.
    profile = colour.icc_profile(3, curve=colour.icc_curve('identity', ()))
    specification = colr(2, profile)
    data = referenced(raw, b'cgrp', [specification[:3], specification[3:]])
    pixels = [tuple(srgb(value) for value in pixel) for pixel in layout.RGB]
    assert pixels == [(0, 137, 255), (255, 188, 0), (137, 225, 99), (188, 99, 225)]
    name = 'reference-split-linear-icc'
    for value in layout.paired(name, raw, header, b'', b'', pixels, subject_data=data,
                                color_box=lambda approximation: colr(2, profile, approximation=approximation)):
        yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}',
                                    'evidence_scope': 'Colour-sensitive reference control; implicit RGB gives different pixels'})
