"""Enumerated CIELab 14 controls, independent of every image reader.

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
    b = 3 << (depths[2] - 3) if depths[2] >= 3 else F(3, 1 << (3 - depths[2]))
    return 100, 0, 170, 1 << (depths[1] - 1), 200, b, D50


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


def container(raw, header, color, feature, *, alternatives=(), required=(), display_mask=None):
    # Corrected M.14: separate required masks for Part 1 and CIELab. RGB is
    # a JP2 baseline colour and needs no additional standard feature flag.
    features = [5] + ([feature] if feature else []) + list(required)
    mask = (1 << len(features)) - 1
    requirements = struct.pack('>BBBH', 1, mask, mask if display_mask is None else display_mask, len(features))
    requirements += b''.join(struct.pack('>HB', value, 1 << i) for i, value in enumerate(features))
    requirements += b'\0\0'  # NVF = 0.
    color = jpx.box(b'colr', b'\x01\x00\x01' + color)
    color = b''.join(jpx.box(b'colr', b'\x01\x01\x01' + choice) for choice in alternatives) + color
    # ihdr is first, then colr, followed by the existing palette/map if present.
    first = int.from_bytes(header[:4], 'big')
    header = header[:first] + color + header[first:]
    return (jpx.box(b'jP  ', b'\r\n\x87\n')
            + jpx.box(b'ftyp', b'jpx ' + struct.pack('>I', 1) + b'jpx jpxb')
            + jpx.box(b'rreq', requirements) + jpx.box(b'jp2h', header) + jpx.box(b'jp2c', raw))


def lab_box(parameters):
    return struct.pack('>I', 14) + (struct.pack('>7I', *parameters) if parameters is not None else b'')


def controls(subject, *, ranges=(-128, 127, -128, 127), lightness=(0, 100), pdf_lab_control=True):
    physical = values(subject)
    # T.4 E.6.4's D50 XYZ is (96.422, 100, 82.521), not rounded ICC D50.
    colors = [colour.cie_rgb('Lab', pixel, ranges=ranges, lightness=lightness) for pixel in physical]
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
            parameters = subject.parameters or defaults(subject.depths)
            extra.update(component_bits=subject.depths, encoded_samples=subject.pixels,
                         lab_parameters=[str(value) if isinstance(value, F) else value for value in parameters],
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
    if not pdf_lab_control:
        return
    # A plain PDF Lab control distinguishes a reader's CIE conversion policy
    # from its JPX interpretation. No JPX sample/output supplies its operands.
    commands = b'/L cs ' + b' '.join(
        (' '.join(f'{float(value):.12f}' for value in pixel) + f' scn {2 + column} 2 1 1 re f').encode()
        for column, pixel in enumerate(physical))
    range_text = b'-128 127 -128 127' if ranges == (-128, 127, -128, 127) else b' '.join(f'{float(v):.12f}'.encode() for v in ranges)
    resources = b'<< /ColorSpace << /L [/Lab << /WhitePoint [.96422 1 .82521] /Range [' + range_text + b'] >>] >> >>'
    yield paired(case(subject.name + '-pdf-lab-control', commands, resources=resources,
                      rectangles=[((2 + column, 2, 1, 1), pixel) for column, pixel in enumerate(colors)],
                      tolerance=1, requirement=REQUIREMENT), 'pdf-lab-control')


def interactions():
    subject = default_subject('lab-default-8', (8, 8, 8))
    raw, header = encoded_parts(subject)
    colors = [colour.cie_rgb('Lab', pixel, ranges=(-128, 127, -128, 127)) for pixel in values(subject)]
    options = {'matrix': (4, 0, 0, 1, 2, 2), 'requirement': REQUIREMENT}
    for name, parameters in [('illuminant', (100, 0, 170, 128, 200, 96, 0x00443635)),
                             ('zero-range', (0, 0, 170, 128, 200, 96, D50))]:
        # Both alternatives use feature 57; the selected explicit D50 record
        # must actually implement it. Its fields equal the default subject's
        # encoding, so the independently derived RGB oracle is unchanged.
        data = container(raw, header, lab_box(defaults(subject.depths)), 57,
                         alternatives=(lab_box(parameters),))
        yield image_case('lab-fallback-' + name, data, b'/Filter /JPXDecode', [], 4, 1,
                         colors=colors, tolerance=1, **options)
        yield image_case('lab-fallback-' + name + '-rgb-control', bytes(v for pixel in colors for v in pixel),
                         b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 4, 1, colors=colors, **options)
    # The file lists explicit Lab (feature57) in FUAM, but its sRGB alternative
    # needs only the Part1 codestream for display. A declared RGB choice remains
    # selectable after unsupported Lab; this is not an inferred DeviceRGB guess.
    data = container(raw, header, struct.pack('>I', 16), 0,
                     alternatives=(lab_box((100, 0, 170, 128, 200, 96, 0x00443635)),),
                     required=(57,), display_mask=1)
    yield image_case('lab-fallback-rgb', data, b'/Filter /JPXDecode', [], 4, 1,
                     colors=subject.pixels, tolerance=1, **options)
    yield image_case('lab-fallback-rgb-control', bytes(v for pixel in subject.pixels for v in pixel),
                     b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 4, 1, colors=subject.pixels, **options)
    # CIELab's offsets must be applied after removing sample premultiplication.
    # Codes divisible by three make alpha=85/170 exact in the stored domain.
    pixels = [(120, 129, 96), (180, 126, 96), (60, 132, 96), (0, 0, 0)]
    alpha = [255, 170, 85, 0]
    physical = values(Subject('alpha', (8, 8, 8), pixels))
    rgb = [colour.cie_rgb('Lab', pixel, ranges=(-128, 127, -128, 127)) for pixel in physical]
    composited = [tuple(int(F(value * opacity + 255 * (255-opacity), 255) + F(1, 2))
                        for value in pixel) for pixel, opacity in zip(rgb, alpha)]
    for mode in (1, 2):
        stored = pixels if mode == 1 else [tuple(value * opacity // 255 for value in pixel)
                                          for pixel, opacity in zip(pixels, alpha)]
        raw = jpx.codestream([list(channel) for channel in zip(*stored)] + [alpha], 4, 1)
        header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, 4, 4, 7, 7, 0, 0))
        definitions = [(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, mode, 0)]
        header += jpx.box(b'cdef', struct.pack('>H', 4) + b''.join(struct.pack('>3H', *v) for v in definitions))
        data = container(raw, header, lab_box(None), 56, required=(8 + mode,))
        yield image_case(f'lab-alpha-{mode}', data, b'/Filter /JPXDecode /SMaskInData %d' % mode, [], 4, 1,
                         colors=composited, tolerance=1, **options)
        yield image_case(f'lab-alpha-{mode}-rgb-control', bytes(v for pixel in composited for v in pixel),
                         b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 4, 1, colors=composited, **options)


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
    # Default b offsets below three bits are fractional. The precise ranges
    # below come directly from M-18, not from any decoder's result.
    for bits, ranges in [(1, (-170, 0, -150, 50)), (2, (-F(340, 3), F(170, 3), -100, 100))]:
        high = (1 << bits) - 1
        subject = Subject(f'lab-default-{bits}bit', (bits, bits, bits),
                          [(0, 0, 0), (high, high, high), (high, 0, 0), (0, high, high)])
        yield from controls(subject, ranges=ranges)
    mixed = Subject('lab-palette-1-2-16', (1, 2, 16),
                    [(0, 0, 0), (1, 3, 65535), (1, 2, 24576), (0, 1, 32768)],
                    representation='palette')
    yield from controls(mixed, ranges=(-F(340, 3), F(170, 3), -F(4915200, 65535), F(8191800, 65535)))
    extended = Subject('lab-explicit-extended-lightness', (8, 8, 8),
                       [(217, 43, 181), (255, 128, 96), (0, 128, 96), (191, 128, 96)],
                       (200, 64, 240, 128, 240, 96, D50))
    # Ordinary PDF Lab clips L* to 0..100, so it cannot serve as an equivalent
    # source control for this declared interval. Codec/RGB controls still do.
    yield from controls(extended, ranges=(-F(30720, 255), F(30480, 255), -F(23040, 255), F(38160, 255)),
                        lightness=(-F(12800, 255), F(38200, 255)), pdf_lab_control=False)
    yield from interactions()
