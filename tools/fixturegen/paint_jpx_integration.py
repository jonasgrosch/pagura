"""R02 integration controls, independent of every decoder and renderer.

PDF 7.4.9, 8.9.7, 11.6; T.800 I.5.3; T.801 M.11.7.2; ICC.1:2010
6.3.4/10.10. ICC profiles and codestreams are authored by the existing
standard-derived builders. Expected colours, alpha and geometry come from
primary formulae, never another PDF engine or product output.
"""
import struct
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


def container(planes, specifications, *, bits=8, extra=b'', origin=(0, 0), brand=b'jpx '):
    width = len(planes[0])
    header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, len(planes), bits - 1, 7, 0, 0))
    header += b''.join(specifications) + extra
    return (jpx.box(b'jP  ', b'\r\n\x87\n') + jpx.box(b'ftyp', brand + bytes(4) + b'jp2 ')
            + jpx.box(b'jp2h', header)
            + jpx.box(b'jp2c', jpx.codestream(planes, width, 1, bits=bits, origin=origin)))


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
        yield image_case(name, container(planes, specs, brand=b'jp2 ' if name.startswith('jp2') else b'jpx '),
                         b'/Filter /JPXDecode', [], 3, 1, colors=pixels, tolerance=1, requirement=ICC)
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
    yield image_case('explicit-overrides-all-candidates', container(planes, [colr(3, b'broken', 3), colr(1, struct.pack('>I', 19), 2)]),
                     b'/Filter /JPXDecode /ColorSpace /DeviceRGB', [], 3, 1, colors=samples, requirement=ICC)


def lab_cases():
    tags = colour.icc_tags(4)
    tags[b'A2B0'] = colour.icc_mab(3, pcs=b'Lab ', only_b=True)
    profile = colour.icc_pack(tags, 3, space=b'Lab ', pcs=b'Lab ')
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


def cases():
    yield from profile_choices()
    yield from lab_cases()
    yield from indexed_palettes()
    yield from interactions()
    yield from inline_resources()
