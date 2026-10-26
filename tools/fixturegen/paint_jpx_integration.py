"""R02 integration controls, independent of every decoder and renderer.

PDF 7.4.9, 8.9.7, 11.6; T.800 I.5.3; T.801 M.11.7.2; ICC.1:2010
6.3.4/10.10. ICC profiles and codestreams are authored by the existing
standard-derived builders. Expected colours, alpha and geometry come from
primary formulae, never another PDF engine or product output.
"""
import struct
from dataclasses import replace
from fractions import Fraction as F

from . import content as colour, jpx
from .model import Case, OPEN, UNSUPPORTED
from .paint import case, image_case, page
from .paint_jpx import encoded, srgb
from .pdf import stream

AREA = 'paint_jpx_integration'
ICC = 'paint-jpx-icc'
PALETTE = 'paint-jpx-indexed-alpha'
INTERACTION = 'paint-jpx-interactions'
INLINE = 'paint-inline-resources'


def colr(method, data, precedence=0, approximation=1):
    return jpx.box(b'colr', struct.pack('>BbB', method, precedence, approximation) + data)


def container(planes, specifications, *, bits=8, signed=False, extra=b'', origin=(0, 0), brand=b'jpx ', requirements=None):
    width = len(planes[0])
    header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, len(planes), bits - 1 + 128 * int(signed), 7, 0, 0))
    header += b''.join(specifications) + extra
    file_type = brand + bytes(4) + b'jp2 '
    reader = b''
    if requirements is not None:
        file_type = b'jpx ' + struct.pack('>I', 1) + b'jpx jpxb'
        reader = jpx.box(b'rreq', requirements)
    return (jpx.box(b'jP  ', b'\r\n\x87\n') + jpx.box(b'ftyp', file_type) + reader
            + jpx.box(b'jp2h', header)
            + jpx.box(b'jp2c', jpx.codestream(planes, width, 1, bits=bits, signed=signed, origin=origin)))


def definitions(rows):
    return jpx.box(b'cdef', struct.pack('>H', len(rows)) + b''.join(struct.pack('>HHH', *row) for row in rows))


def bad_image(name, data, dictionary=b'', width=3, requirement=ICC, error='syntax'):
    image = stream(data, b'/Subtype /Image /Width %d /Height 1 /Filter /JPXDecode ' % width + dictionary)
    return Case(name, page(b'6 0 0 2 2 2 cm /I Do', [image], b'<< /XObject << /I 5 0 R >> >>'),
                UNSUPPORTED if '.' in error else OPEN, (requirement,), 'Malformed or unavailable metadata remains explicit',
                feature=error if '.' in error else None, extra={} if '.' in error else {'render_error': error})


def profile_choices():
    unit = colour.icc_curve('identity', ())
    exact = colour.icc_profile(3, curve=unit)
    approximate = colour.icc_profile(3, curve=colour.icc_curve('sampled', (0, 26214, 65535)))
    future = colour.icc_profile(3, version=5)
    samples = [(64, 128, 192), (128, 64, 255), (0, 255, 128)]
    planes = [list(values) for values in zip(*samples)]
    pixels = [tuple(map(srgb, pixel)) for pixel in samples]
    choices = [
        ('precedence-signed', [colr(2, approximate, -2), colr(2, exact, 2)]),
        ('approximation-quality', [colr(2, approximate, 3, 4), colr(2, exact, 3, 1)]),
        ('future-then-supported', [colr(3, future, 4), colr(2, exact, 3)]),
        ('jp2-future-then-supported', [colr(3, future, 0, 0), colr(2, exact, 0, 0)]),
        ('vendor-then-supported', [colr(4, bytes(16), 4), colr(2, exact, 3)]),
        ('stable-equal-priority', [colr(2, exact, 3), colr(2, approximate, 3)]),
    ]
    for name, specs in choices:
        observed = image_case(name, container(planes, specs, brand=b'jp2 ' if name.startswith('jp2') else b'jpx '),
                              b'/Filter /JPXDecode', [], 3, 1, colors=pixels, tolerance=1, requirement=ICC)
        if name in ('precedence-signed', 'approximation-quality', 'stable-equal-priority'):
            observed = replace(observed, extra={**observed.extra, 'jpx_validity': 'malformed-duplicate-method',
                                                'evidence_scope': 'historical malformed-input observation; T.801 M.11.7.1 forbids repeated METH2'})
        yield observed
    # M.11.7.1 permits repeated Any ICC (METH3), but not Restricted ICC
    # (METH2), in both jp2h and cgrp. Keep the old bytes above as observations;
    # these distinct files establish the same ranking obligations legally.
    # Cor4 Table M.14: FUAM needs Part1 (5) and Any ICC (44). DCM needs only
    # Part1, since the single restricted profile supplies the same exact colour.
    requirements = struct.pack('>BBBH', 1, 3, 1, 2) + struct.pack('>HBHBH', 5, 1, 44, 2, 0)
    ranked = [
        ('precedence-signed', [colr(3, approximate, -2, 4), colr(3, exact, 2)]),
        ('approximation-quality', [colr(3, approximate, 3, 4), colr(3, exact, 3, 1)]),
        ('stable-equal-priority', [colr(3, exact, 3, 4), colr(3, approximate, 3, 4)]),
    ]
    for name, specs in ranked:
        data = container(planes, [*specs, colr(2, exact, -128)], requirements=requirements)
        yield image_case('any-icc-' + name, data, b'/Filter /JPXDecode', [], 3, 1,
                         colors=pixels, tolerance=1, requirement=ICC)
        yield image_case('any-icc-' + name + '-codec-control', encoded(planes, exact),
                         b'/Filter /JPXDecode', [], 3, 1, colors=pixels, tolerance=1, requirement=ICC)
        yield image_case('any-icc-' + name + '-rgb-control', bytes(v for pixel in pixels for v in pixel),
                         b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 3, 1, colors=pixels, requirement=ICC)
    yield image_case('all-profiles-unavailable', container(planes, [colr(3, future, 4), colr(3, future, 3)]),
                     b'/Filter /JPXDecode', [], 3, 1, colors=samples, requirement=ICC,
                     resources=b'/ColorSpace << /DefaultRGB /DeviceGray >>')
    yield image_case('embedded-enumeration-ignores-default', container(planes, [colr(1, struct.pack('>I', 16))]),
                     b'/Filter /JPXDecode', [], 3, 1, colors=samples, requirement=ICC,
                     resources=b'/ColorSpace << /DefaultRGB /DeviceGray >>')
    # A future forward-element signature is an unsupported transform, not a
    # malformed-length repair. It is selected only for relative/absolute intent.
    tags = colour.icc_tags(3, unit)
    tags[b'A2B1'] = b'futr' + bytes(4)
    intent_profile = colour.icc_pack(tags)
    specs = [colr(3, intent_profile, 3), colr(1, struct.pack('>I', 16), 2)]
    data = container(planes, specs)
    for name, intent, expected in [('relative', b'/RelativeColorimetric', samples),
                                   ('perceptual', b'/Perceptual', pixels)]:
        yield image_case('selected-transform-' + name, data, b'/Filter /JPXDecode /Intent ' + intent,
                         [], 3, 1, colors=expected, tolerance=1, requirement=ICC)
    # The same source is interpreted twice under different inherited intents.
    # This detects both candidate-profile and image-description cache aliases.
    image = stream(data, b'/Subtype /Image /Width 3 /Height 1 /Filter /JPXDecode')
    rectangles = [((2 + 2 * x, y, 2, 2), value) for y, values in [(2, samples), (6, pixels)]
                  for x, value in enumerate(values)]
    yield case('inherited-intent-cache', b'q /RelativeColorimetric ri 6 0 0 2 2 2 cm /I Do Q '
               b'q /Perceptual ri 6 0 0 2 2 6 cm /I Do Q', [image],
               b'<< /XObject << /I 5 0 R >> >>', rectangles, requirement=ICC, tolerance=1)
    yield bad_image('malformed-selected-profile', container(planes, [colr(3, b'broken', 3), colr(2, exact, 2)]))
    yield bad_image('invalid-jpx-approximation', container(planes, [colr(2, exact, 0, 0)]))
    yield bad_image('signed-icc-sample-boundary', encoded(planes, lab_profile(), method=3, signed=True),
                    error='image.JPX-colour')
    yield image_case('explicit-overrides-all-candidates', container(planes, [colr(3, b'broken', 3), colr(1, struct.pack('>I', 19), 2)]),
                     b'/Filter /JPXDecode /ColorSpace /DeviceRGB', [], 3, 1, colors=samples, requirement=ICC)


def lab_profile():
    tags = colour.icc_tags(4)
    tags[b'A2B0'] = colour.icc_mab(3, pcs=b'Lab ', only_b=True)
    return colour.icc_pack(tags, 3, space=b'Lab ', pcs=b'Lab ')


def lab_cases():
    profile = lab_profile()
    def rgb(values):
        return colour.cie_rgb('Lab', (100 * values[0], 255 * values[1] - 128, 255 * values[2] - 128),
                              white=colour.ICC_D50)
    for bits in (8, 16):
        maximum = (1 << bits) - 1
        factor = maximum // 255
        samples = [(0, 128, 128), (128, 128, 128), (160, 150, 100), (255, 128, 128)]
        planes = [[factor * value for value in channel] for channel in zip(*samples)]
        yield image_case(f'lab-input-{bits}', encoded(planes, profile, method=3, bits=bits),
                         b'/Filter /JPXDecode', [], 4, 1,
                         colors=[rgb(tuple(F(value, 255) for value in pixel)) for pixel in samples],
                         tolerance=1, requirement=ICC)
    alphas = [0, 128, 255]
    for mode in (1, 2):
        planes = [[128] * 3] * 3 if mode == 1 else [[0, 64, 128]] * 3
        colours = []
        for pixel, alpha in zip(zip(*planes), alphas):
            normalized = tuple(F(value, alpha if mode == 2 and alpha else 255) for value in pixel)
            colours.append(rgb(normalized))
        yield image_case(f'lab-alpha-{mode}', encoded(planes + [alphas], profile, method=3, alpha=mode),
                         b'/Filter /JPXDecode /SMaskInData %d' % mode, [], 3, 1,
                         colors=colours, alpha=[F(a, 255) for a in alphas], tolerance=1, requirement=ICC)
    # Decode is applied after undoing sample premultiplication (T.800 I-5).
    data = encoded([[0, 64, 255], alphas], b'ignored', alpha=2)
    yield image_case('premultiplied-inverted-decode', data,
                     b'/Filter /JPXDecode /SMaskInData 2 /ColorSpace /DeviceGray /Decode [1 0]', [], 3, 1,
                     colors=[(255,) * 3, (128,) * 3, (0,) * 3],
                     alpha=[F(a, 255) for a in alphas], tolerance=1, requirement=ICC)


def indexed_palettes():
    indices = [0, 1, 2]
    pdf_palette = b'/ColorSpace [/Indexed /DeviceRGB 2 <000000800000ff0000>]'
    colors = [(0, 0, 0), (128, 0, 0), (255, 0, 0)]
    for mapped in (False, True):
        for first in (False, True):
            for mode in (0, 1, 2):
                alphas = [0, 32768, 65535] if mapped else [0, 128, 255]
                maximum = 65535 if mapped else 255
                planes = [indices] if mapped else [indices, alphas]
                # Container RGB colours deliberately disagree with the PDF
                # palette. Its alpha column retains independent 16-bit depth.
                palette = struct.pack('>HB', 3, 4) + bytes([7, 7, 7, 15])
                palette += b''.join(bytes([0, 255, 0]) + struct.pack('>H', alpha)
                                    for alpha in [0, 32768, 65535])
                maps = [(0, 1, i) for i in range(3)]
                opacity = (0, 1, 3) if mapped else (1, 0, 0)
                maps = [opacity] + maps if first else maps + [opacity]
                rows = [(i + int(first), 0, i + 1) for i in range(3)]
                rows += [(0 if first else 3, max(mode, 1), 0)]
                extra = jpx.box(b'pclr', palette) + jpx.box(b'cmap', b''.join(struct.pack('>HBB', *row) for row in maps))
                extra += definitions(rows)
                data = container(planes, [colr(1, struct.pack('>I', 16))], extra=extra, origin=(3, 5))
                yield image_case(f'indexed-palette-{int(mapped)}-{int(first)}-{mode}', data,
                                 b'/Filter /JPXDecode /SMaskInData %d ' % mode + pdf_palette, [], 3, 1,
                                 colors=[(255, 0, 0)] * 3 if mode == 2 else colors,
                                 alpha=[F(a, maximum) for a in alphas] if mode else None,
                                 tolerance=1, requirement=PALETTE)
    data = container([indices], [colr(1, struct.pack('>I', 17))], extra=definitions([(0, 1, 0)]))
    yield bad_image('indexed-only-opacity', data, pdf_palette, requirement=PALETTE)
    # Mapping two colour channels from different indices cannot identify one
    # PDF Indexed sample and must not be guessed from the first component.
    extra = jpx.box(b'pclr', struct.pack('>HB', 1, 1) + b'\7\0')
    extra += jpx.box(b'cmap', struct.pack('>HBBHBB', 0, 1, 0, 1, 1, 0))
    yield bad_image('indexed-ambiguous-index', container([indices, indices], [colr(1, struct.pack('>I', 16))], extra=extra),
                    pdf_palette, requirement=PALETTE, error='image.JPX-sample-layout')
    extra = jpx.box(b'pclr', struct.pack('>HB', 1, 1) + b'\7\0')
    extra += jpx.box(b'cmap', struct.pack('>HBB', 0, 1, 0))
    for kind, error in [(3, 'syntax'), (65535, 'image.JPX-sample-layout')]:
        data = container([indices], [colr(1, struct.pack('>I', 17))], extra=extra + definitions([(0, kind, 1)]))
        yield bad_image(f'indexed-channel-type-{kind}', data, pdf_palette, requirement=PALETTE, error=error)


def interactions():
    unit = colour.icc_curve('identity', ())
    profile = colour.icc_profile(3, curve=unit)
    alpha = [0, 128, 255]
    pixels = [(srgb(128),) * 3] * 3
    data = encoded([[128] * 3] * 3, profile)
    mask = stream(bytes(alpha), b'/Subtype /Image /Width 3 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray')
    yield image_case('icc-external-soft-mask', data, b'/Filter /JPXDecode /SMask 6 0 R', [], 3, 1,
                     masks=[mask], colors=pixels, alpha=[F(a, 255) for a in alpha], requirement=INTERACTION, tolerance=1)
    for isolated in (False, True):
        image = stream(encoded([[255] * 3, [0] * 3, [0] * 3, alpha], profile, alpha=1),
                       b'/Subtype /Image /Width 3 /Height 1 /Filter /JPXDecode /SMaskInData 1')
        group = stream(b'6 0 0 2 2 2 cm /I Do', b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] '
                       b'/Group << /S /Transparency /CS /DeviceRGB /I ' + (b'true' if isolated else b'false')
                       + b' >> /Resources << /XObject << /I 5 0 R >> >>')
        rectangles = [((2 + x * 2, 2, 2, 2), (255, int(255 - F(a, 2) + F(1, 2)), int(255 - F(a, 2) + F(1, 2))))
                      for x, a in enumerate(alpha)]
        yield case(f'icc-alpha-group-{int(isolated)}', b'/G gs /F Do', [image, group],
                   b'<< /XObject << /F 6 0 R >> /ExtGState << /G << /ca .5 >> >> >>', rectangles,
                   requirement=INTERACTION, tolerance=1)
    for mode in (1, 2):
        planes = [[255, 255], [0, 0], [0, 0]] if mode == 1 else [[0, 255], [0, 0], [0, 0]]
        data = encoded(planes + [[0, 255]], profile, alpha=mode)
        image = stream(data, b'/Subtype /Image /Width 2 /Height 1 /Filter /JPXDecode /SMaskInData %d /Interpolate true' % mode)
        # Bilinear interpolation uses associated colour, so an invisible
        # texel cannot bleed black into its visible red neighbour.
        values = [F(0), F(0), F(1, 8), F(3, 8), F(5, 8), F(7, 8), F(1), F(1)]
        rectangles = [((2 + x, 2, 1, 2), (255,) + (int(255 * (1 - a) + F(1, 2)),) * 2)
                      for x, a in enumerate(values)]
        yield case(f'icc-alpha-interpolate-{mode}', b'8 0 0 2 2 2 cm /I Do', [image],
                   b'<< /XObject << /I 5 0 R >> >>', rectangles, requirement=INTERACTION, tolerance=1)
    gray = encoded([[0, 128, 255]], colour.icc_profile(1, curve=unit))
    yield image_case('icc-raw-color-key', gray, b'/Filter /JPXDecode /Mask [128 128]', [], 3, 1,
                     colors=[(srgb(v),) * 3 for v in [0, 128, 255]], alpha=[F(1), F(0), F(1)],
                     requirement=INTERACTION, tolerance=1)


def inline_resources():
    # Raw samples deliberately contain whitespace/EI/whitespace. Framing is
    # arithmetic and must consume the complete row before resuming operators.
    data = b' EI '\
           b'Q!'
    pixels = [(value,) * 3 for value in data]
    rectangles = [((2 + x, 2, 1, 1), value) for x, value in enumerate(pixels)]
    profiles = [(b'/C 5 0 R', [b'[/ICCBased 6 0 R]', stream(colour.icc_profile(1, curve=colour.icc_curve('identity', ())), b'/N 7 0 R'), b'1'],
                 [(srgb(value),) * 3 for value in data]),
                (b'/C [/DeviceN 5 0 R /DeviceGray 6 0 R]', [b'[/Spot]', b'<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>'], pixels)]
    for number, (resource, objects, colours) in enumerate(profiles):
        rectangles = [((2 + x, 2, 1, 1), value) for x, value in enumerate(colours)]
        content = b'q 6 0 0 1 2 2 cm BI /W 6 /H 1 /BPC 8 /CS /C ID ' + data + b' EI Q 1 0 0 rg 10 2 1 1 re f'
        yield case(f'inline-resource-{number}', content, objects, b'<< /ColorSpace << ' + resource + b' >> >>',
                   rectangles + [((10, 2, 1, 1), (255, 0, 0))], requirement=INLINE, tolerance=1)
    # Five components exercise a count for which there is no device-name
    # surrogate. An indirect array of names is resolved without a transform.
    names = b'[/A /B /C /D /E]'
    function = b'<< /FunctionType 4 /Domain [0 1 0 1 0 1 0 1 0 1] /Range [0 1] >>'
    calculator = stream(b'{ pop pop pop pop }', function[2:-2])
    content = b'q 2 0 0 1 2 2 cm BI /W 2 /H 1 /BPC 8 /CS /C ID ' + bytes([0, 32, 69, 73, 32, 255, 32, 69, 73, 32]) + b' EI Q'
    yield case('inline-five-components', content, [names, calculator],
               b'<< /ColorSpace << /C [/DeviceN 5 0 R /DeviceGray 6 0 R] >> >>',
               [((2, 2, 1, 1), (0, 0, 0)), ((3, 2, 1, 1), (255, 255, 255))], requirement=INLINE)


def signed_trc_cases():
    """T.800 I.5.3.3/I.9 restricted-profile negative-input clipping controls.

    Builders store signed source integers with an unsigned storage bias, while
    the colour formula uses the original signed value and full bit precision.
    These cases deliberately distinguish clipping from adding a colour offset
    or normalizing by the positive signed maximum. Unsigned JPX controls use
    the same profile, while ordinary RGB isolates final display sampling.
    """
    from dataclasses import replace
    unit = colour.icc_curve('identity', ())
    for bits in (8, 16):
        bias, maximum = 1 << (bits - 1), (1 << bits) - 1
        values = [0, bias // 2, bias - 1, bias, bias + bias // 2, maximum]
        for components in (1, 3):
            profile = colour.icc_profile(components, curve=unit)
            planes = [values] if components == 1 else [values, values[::-1], [bias] * len(values)]
            pixels = []
            for codes in zip(*planes):
                mapped = tuple(srgb(F(255 * max(0, code - bias), maximum)) for code in codes)
                pixels.append(mapped * 3 if components == 1 else mapped)
            name = f'signed-trc-{"gray" if components == 1 else "rgb"}-{bits}'
            options = {'matrix': (6, 0, 0, 1, 2, 2), 'colors': pixels, 'tolerance': 1, 'requirement': ICC}
            yield image_case(name, encoded(planes, profile, bits=bits, signed=True),
                             b'/Filter /JPXDecode', [], 6, 1, **options)
            unsigned = image_case(name + '-unsigned-control',
                                  encoded([[max(0, code - bias) for code in plane] for plane in planes], profile, bits=bits),
                                  b'/Filter /JPXDecode', [], 6, 1, **options)
            yield replace(unsigned, description=f'Unsigned JPX ICC control for {AREA}/{name}',
                          extra={**unsigned.extra, 'control_of': f'{AREA}/{name}'})
            control = image_case(name + '-rgb-control', bytes(channel for pixel in pixels for channel in pixel),
                                 b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 6, 1, **options)
            yield replace(control, description=f'Uncompressed RGB control for {AREA}/{name}',
                          extra={**control.extra, 'control_of': f'{AREA}/{name}'})


def signed_trc_integration():
    """Original signed values survive palette mapping, opacity and selection."""
    profile = colour.icc_profile(3, curve=colour.icc_curve('identity', ()))
    samples = [(-64, 0, -1000), (32, 128, 8192), (64, 256, 16384)]
    alphas = [0, 128, 255]
    # T.800 I.5.3.4 stores signed palette values in two's complement,
    # independently of the unsigned codestream index and opacity channel.
    palette = struct.pack('>HB', 3, 4) + bytes([135, 9, 143, 7])
    palette += b''.join(struct.pack('>BHHB', r & 255, g, b & 65535, alpha)
                        for (r, g, b), alpha in zip(samples, alphas))
    maps = b''.join(struct.pack('>HBB', 0, 1, column) for column in range(4))
    for mode in (0, 1, 2):
        extra = jpx.box(b'pclr', palette) + jpx.box(b'cmap', maps)
        extra += definitions([(i, 0, i + 1) for i in range(3)] + [(3, max(1, mode), 0)])
        data = container([[0, 1, 2]], [colr(2, profile)], extra=extra)
        pixels = []
        for sample, alpha in zip(samples, alphas):
            values = [F(max(0, value), maximum) for value, maximum in zip(sample, [255, 1023, 65535])]
            if mode == 2 and alpha:
                values = [value * F(255, alpha) for value in values]
            pixels.append(tuple(srgb(value * 255) for value in values))
        yield image_case(f'signed-trc-mixed-alpha-{mode}', data,
                         b'/Filter /JPXDecode /SMaskInData %d' % mode, [], 3, 1,
                         colors=pixels, alpha=[F(a, 255) for a in alphas] if mode else None,
                         tolerance=1, requirement=ICC)
    planes = [[128, 192, 255]] * 3
    data = container(planes, [colr(3, lab_profile(), 3), colr(2, profile, 2)], signed=True)
    yield image_case('signed-trc-candidate-fallback', data, b'/Filter /JPXDecode', [], 3, 1,
                     colors=[(srgb(value),) * 3 for value in [0, 64, 127]], tolerance=1, requirement=ICC)
    future = colour.icc_profile(3, version=5)
    yield bad_image('signed-trc-no-device-fallback', encoded(planes, future, method=3, signed=True),
                    error='image.JPX-colour')


def cases():
    yield from profile_choices()
    yield from lab_cases()
    yield from indexed_palettes()
    yield from interactions()
    yield from inline_resources()
    yield from signed_trc_cases()
    yield from signed_trc_integration()
