"""Private enumerated CIELab 14 controls, independent of every image reader.

T.801 M.9.2.4/M.11.7.4.1, equation M-18 and Tables M.29-M.30;
Cor.4 Table M.14; T.4 E.6.4; PDF 7.4.9/8.6.5.4. The Part 1 encoder,
independent CIE arithmetic and PDF geometry reuse their existing owners.
Only D50 RGB expectations are established here. Other listed illuminants,
unknown illuminants and degenerate ranges are distinct unprobed boundaries.
"""

import hashlib
import struct
from dataclasses import dataclass, replace
from fractions import Fraction as F

from . import content as colour, jpx
from .model import Vector
from .paint import case, image_case

AREA = 'jpx_lab_probe'
REQUIREMENT = 'jpx-pdf-contract'
D50 = 0x00443530


@dataclass(frozen=True)
class Subject:
    name: str
    depths: tuple[int, int, int]
    pixels: list[tuple[int, int, int]]
    parameters: tuple[int, ...] | None = None
    representation: str = 'direct'


def defaults(depths):
    # M-18 uses the precision of each colour channel, including palette output.
    return 100, 0, 170, 1 << (depths[1] - 1), 200, 3 << (depths[2] - 3), D50


def values(subject):
    parameters = subject.parameters or defaults(subject.depths)
    ranges, offsets = parameters[:6:2], parameters[1:6:2]
    return [tuple(F((value - offset) * span, (1 << bits) - 1)
                  for value, span, offset, bits in zip(pixel, ranges, offsets, subject.depths))
            for pixel in subject.pixels]


def default_subject(name, depths, representation='direct'):
    maxima = [(1 << bits) - 1 for bits in depths]
    a, b = 1 << (depths[1] - 1), 3 << (depths[2] - 3)
    pixels = [(maxima[0] // 2, a, b),
              (3 * maxima[0] // 4, a + maxima[1] // 8, b - maxima[2] // 8),
              (maxima[0] // 4, a - maxima[1] // 8, b + maxima[2] // 8),
              (maxima[0], a, b)]
    return Subject(name, depths, pixels, representation=representation)


def subjects():
    eight = default_subject('lab-default-8', (8, 8, 8))
    yield eight
    yield replace(eight, name='lab-explicit-default-8', parameters=defaults(eight.depths))
    yield default_subject('lab-default-16', (16, 16, 16))
    for bits in (8, 16):
        scale = 1 << (bits - 8)
        parameters = (200, 64 * scale, 120, 96 * scale, 240, 160 * scale, D50)
        pixels = [(lightness * scale, a * scale, b * scale)
                  for lightness, a, b in [(64, 96, 160), (96, 128, 128), (128, 64, 192), (191, 96, 160)]]
        yield Subject(f'lab-explicit-custom-{bits}', (bits, bits, bits), pixels, parameters)
    yield default_subject('lab-default-palette-8-12-16', (8, 12, 16), 'palette')


def encoded_parts(subject, *, sample_kind='unsigned'):
    width = len(subject.pixels)
    if subject.representation == 'palette':
        raw = jpx.codestream([list(range(width))], width, 1)
        header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, 1, 7, 7, 0, 0))
        palette = struct.pack('>HB', width, 3) + bytes(bits - 1 for bits in subject.depths)
        palette += b''.join(value.to_bytes((bits + 7) // 8, 'big')
                            for pixel in subject.pixels for value, bits in zip(pixel, subject.depths))
        header += jpx.box(b'pclr', palette)
        header += jpx.box(b'cmap', b''.join(struct.pack('>HBB', 0, 1, column) for column in range(3)))
    else:
        bits = subject.depths[0]
        assert subject.depths == (bits, bits, bits)
        signed = sample_kind == 'signed'
        raw = jpx.codestream([list(values) for values in zip(*subject.pixels)], width, 1,
                             bits=bits, signed=signed)
        header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, 3, bits - 1 + 128 * int(signed), 7, 0, 0))
    return raw, header


def container(raw, header, color, feature):
    # Corrected M.14: separate required masks for Part 1 and CIELab. RGB is
    # a JP2 baseline colour and needs no additional standard feature flag.
    requirements = struct.pack('>BBBH', 1, 3 if feature else 1, 3 if feature else 1, 2 if feature else 1)
    requirements += struct.pack('>HB', 5, 1)
    if feature:
        requirements += struct.pack('>HB', feature, 2)
    requirements += b'\0\0'  # NVF = 0.
    color = jpx.box(b'colr', b'\x01\x00\x01' + color)
    # ihdr is first, then colr, followed by the existing palette/map if present.
    first = int.from_bytes(header[:4], 'big')
    header = header[:first] + color + header[first:]
    return (jpx.box(b'jP  ', b'\r\n\x87\n')
            + jpx.box(b'ftyp', b'jpx ' + struct.pack('>I', 1) + b'jpx jpxb')
            + jpx.box(b'rreq', requirements) + jpx.box(b'jp2h', header) + jpx.box(b'jp2c', raw))


def lab_box(parameters):
    return struct.pack('>I', 14) + (struct.pack('>7I', *parameters) if parameters is not None else b'')


def controls(subject):
    physical = values(subject)
    # T.4 E.6.4's D50 XYZ is (96.422, 100, 82.521), not rounded ICC D50.
    colors = [colour.cie_rgb('Lab', pixel, ranges=(-128, 127, -128, 127)) for pixel in physical]
    raw, header = encoded_parts(subject)
    lab = container(raw, header, lab_box(subject.parameters), 56 if subject.parameters is None else 57)
    rgb = container(raw, header, struct.pack('>I', 16), 0)
    width = len(colors)
    options = {'matrix': (width, 0, 0, 1, 2, 2), 'requirement': REQUIREMENT}
    def paired(control, role):
        extra = {**control.extra, 'control_group': f'{AREA}/{subject.name}', 'role': role}
        if role in ('subject', 'codec-control'):
            extra['codestream_sha256'] = hashlib.sha256(raw).hexdigest()
        if role == 'subject':
            extra.update(component_bits=subject.depths, encoded_samples=subject.pixels,
                         lab_parameters=subject.parameters or defaults(subject.depths),
                         lab_values=[[str(value) for value in pixel] for pixel in physical])
        return replace(control, extra=extra)
    yield paired(image_case(subject.name, lab, b'/Filter /JPXDecode', [], width, 1,
                            colors=colors, tolerance=1, **options), 'subject')
    codec_colors = [tuple(int(F(value * 255, (1 << bits) - 1) + F(1, 2))
                          for value, bits in zip(pixel, subject.depths)) for pixel in subject.pixels]
    yield paired(image_case(subject.name + '-codec-control', rgb, b'/Filter /JPXDecode', [], width, 1,
                            colors=codec_colors, tolerance=1, **options), 'codec-control')
    yield paired(image_case(subject.name + '-rgb-control', bytes(value for pixel in colors for value in pixel),
                            b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], width, 1, colors=colors, **options), 'rgb-control')
    # A plain PDF Lab control distinguishes a reader's CIE conversion policy
    # from its JPX interpretation. No JPX sample/output supplies its operands.
    commands = b'/L cs ' + b' '.join(
        (' '.join(f'{float(value):.12f}' for value in pixel) + f' scn {2 + column} 2 1 1 re f').encode()
        for column, pixel in enumerate(physical))
    resources = b'<< /ColorSpace << /L [/Lab << /WhitePoint [.96422 1 .82521] /Range [-128 127 -128 127] >>] >> >>'
    yield paired(case(subject.name + '-pdf-lab-control', commands, resources=resources,
                      rectangles=[((2 + column, 2, 1, 1), pixel) for column, pixel in enumerate(colors)],
                      tolerance=1, requirement=REQUIREMENT), 'pdf-lab-control')


def boundaries():
    subject = default_subject('lab-boundary', (8, 8, 8))
    raw, header = encoded_parts(subject)
    parameters = defaults(subject.depths)
    complete = struct.pack('>7I', *parameters)
    # Empty EP is the valid default above; every nonempty EP must be 28 bytes.
    for size in (1, 4, 24, 27, 29, 32):
        ep = (complete + bytes(4))[:size]
        yield jpx.rejection(f'lab-malformed-ep-{size}',
                            container(raw, header, struct.pack('>I', 14) + ep, 57),
                            (REQUIREMENT,), 'T.801 M.11.7.4.1/Table M.30: incomplete or excess EP fields')
    # These are source boundaries with no pixel/error verdict. Defined other
    # illuminants are not invalid merely because this probe establishes D50.
    for name, fields, reason in [
        ('lab-d65-unprobed', (*parameters[:6], 0x00443635), 'Table M.29 defines D65; no RGB oracle established here'),
        ('lab-temperature-unprobed', (*parameters[:6], 0x43541d4c), 'Table M.29 defines CT plus 7500 K; no RGB oracle established here'),
        ('lab-unknown-illuminant', (*parameters[:6], 0x5a5a5a5a), 'Unassigned illuminant has no guessed D50 replacement'),
        ('lab-zero-range-unadjudicated', (0, *parameters[1:]), 'Table M.30 lists zero but M-18 division is degenerate; no invented verdict'),
    ]:
        yield Vector(name, {'.jpx': container(raw, header, lab_box(fields), 57), '.note': (reason + '\n').encode()},
                     (REQUIREMENT,), reason, extra={'observation': 'unprobed-boundary'})
    raw, header = encoded_parts(subject, sample_kind='signed')
    reason = 'M-18 defines unsigned channel values; signed CIELab has no pixel oracle in this probe'
    yield Vector('lab-signed-boundary', {'.jpx': container(raw, header, lab_box(None), 56), '.note': (reason + '\n').encode()},
                 (REQUIREMENT,), reason, extra={'observation': 'unprobed-boundary'})


def cases():
    for subject in subjects():
        yield from controls(subject)
    yield from boundaries()
