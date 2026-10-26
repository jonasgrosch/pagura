"""R02 embedded colour integration: independent profiles, codestreams and pixels.

ISO 32000-2 7.4.9 / Table 87 and T.800 I.5.3.3. The existing independent
ICC.1:2010 profile builder supplies a unit TRC and D50-adapted sRGB primaries.
Expected pixels use the scalar IEC sRGB encoding of known linear samples;
they never call Pagura's transforms or another PDF engine.
"""
import struct
from fractions import Fraction as F

from . import content as colour, jpx
from .model import Case, OPEN, UNSUPPORTED
from .paint import image_case, page
from .pdf import stream

AREA = "paint_jpx"
REQUIREMENT = "paint-jpx-icc"


def encoded(planes, profile, *, alpha=0, method=2, bits=8):
    width = len(planes[0])
    header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, len(planes), bits - 1, 7, 0, 0))
    header += jpx.box(b'colr', bytes([method, 0, 0]) + profile)
    if alpha:
        entries = [(i, 0, i + 1) for i in range(len(planes) - 1)] + [(len(planes) - 1, alpha, 0)]
        header += jpx.box(b'cdef', struct.pack('>H', len(entries)) + b''.join(struct.pack('>HHH', *row) for row in entries))
    return (jpx.box(b'jP  ', b'\r\n\x87\n') + jpx.box(b'ftyp', b'jp2 \0\0\0\0jp2 ')
            + jpx.box(b'jp2h', header) + jpx.box(b'jp2c', jpx.codestream(planes, width, 1, bits=bits)))


def srgb(sample):
    linear = sample / 255
    return int(255 * (12.92 * linear if linear <= .0031308 else 1.055 * linear ** (1 / 2.4) - .055) + .5)


def cases():
    yield from sycc_cases()
    unit = colour.icc_curve('identity', ())
    gray = colour.icc_profile(1, curve=unit)
    rgb = colour.icc_profile(3, curve=unit)
    samples = [0, 64, 128, 255]
    yield image_case('gray-linear', encoded([samples], gray), b'/Filter /JPXDecode', [], 4, 1,
                     colors=[(srgb(value),) * 3 for value in samples], tolerance=1, requirement=REQUIREMENT)
    pixels = [(0, 0, 0), (64, 128, 192), (255, 0, 0), (255, 255, 255)]
    planes = [list(values) for values in zip(*pixels)]
    yield image_case('rgb-linear', encoded(planes, rgb), b'/Filter /JPXDecode', [], 4, 1,
                     colors=[tuple(map(srgb, pixel)) for pixel in pixels], tolerance=1, requirement=REQUIREMENT)
    # An invalid embedded profile is irrelevant when PDF ColorSpace wins;
    # the explicit Decode applies to the original samples as well.
    yield image_case('pdf-override', encoded(planes, b'not an ICC profile'),
                     b'/Filter /JPXDecode /ColorSpace /DeviceRGB /Decode [1 0 1 0 1 0]', [], 4, 1,
                     colors=[tuple(255 - value for value in pixel) for pixel in pixels], requirement=REQUIREMENT)
    # With no PDF ColorSpace, Decode is ignored and DefaultRGB cannot replace
    # the embedded ICC profile, whose actual colourimetry is authoritative.
    yield image_case('ignores-pdf-decode-default', encoded(planes, rgb),
                     b'/Filter /JPXDecode /Decode [1 0 1 0 1 0]', [], 4, 1,
                     resources=b'/ColorSpace << /DefaultRGB /DeviceGray >>',
                     colors=[tuple(map(srgb, pixel)) for pixel in pixels], tolerance=1, requirement=REQUIREMENT)
    # The ordinary channel count selects the device fallback for a later ICC
    # version that the existing profile parser explicitly cannot interpret.
    yield image_case('future-profile-fallback', encoded(planes, colour.icc_profile(3, version=5)),
                     b'/Filter /JPXDecode', [], 4, 1, colors=pixels, requirement=REQUIREMENT)
    alphas = [0, 128, 255]
    yield image_case('straight-alpha', encoded([[128] * 3] * 3 + [alphas], rgb, alpha=1),
                     b'/Filter /JPXDecode /SMaskInData 1', [], 3, 1,
                     colors=[(srgb(128),) * 3] * 3, alpha=[F(a, 255) for a in alphas], tolerance=1, requirement=REQUIREMENT)
    # Stored colour is 0, 64, 128 while alpha is 0, 128, 255. The middle
    # unassociated sample is exactly 1/2, within one byte of 128/255.
    yield image_case('premultiplied-alpha', encoded([[0, 64, 128]] * 3 + [alphas], rgb, alpha=2),
                     b'/Filter /JPXDecode /SMaskInData 2', [], 3, 1,
                     colors=[(srgb(128),) * 3] * 3, alpha=[F(a, 255) for a in alphas], tolerance=1, requirement=REQUIREMENT)
    for name, profile in [('truncated-profile', b'broken'), ('profile-channel-mismatch', gray)]:
        image = stream(encoded(planes, profile), b'/Subtype /Image /Width 4 /Height 1 /Filter /JPXDecode')
        yield Case(name, page(b'8 0 0 2 2 2 cm /I Do', [image], b'<< /XObject << /I 5 0 R >> >>'),
                   OPEN, (REQUIREMENT,), 'Malformed embedded ICC remains a syntax error', extra={'render_error': 'syntax'})


def sycc_cases():
    # IEC 61966-2-1 Amendment 1 F.15/F.16. Fractions retain the published
    # decimal matrix exactly; this oracle does not reuse product arithmetic.
    def rgb(pixel):
        y, cb, cr = pixel[0], pixel[1] - 128, pixel[2] - 128
        values = (y + F(14020, 10000) * cr,
                  y - F(3441, 10000) * cb - F(7141, 10000) * cr,
                  y + F(17720, 10000) * cb)
        return tuple(int(min(255, max(0, value)) + F(1, 2)) for value in values)

    requirement = 'paint-jpx-sycc'
    enumeration = struct.pack('>I', 18)
    for name, pixels in [
        ('sycc-neutral', [(value, 128, 128) for value in (0, 64, 128, 255)]),
        ('sycc-chroma', [(76, 85, 255), (150, 44, 21), (29, 255, 107)]),
        ('sycc-gamut', [(0, 0, 0), (255, 255, 255), (128, 0, 255)]),
    ]:
        data = encoded([list(values) for values in zip(*pixels)], enumeration, method=1)
        yield image_case(name, data, b'/Filter /JPXDecode', [], len(pixels), 1,
                         colors=list(map(rgb, pixels)), requirement=requirement)
    pixels = [(76, 85, 255), (150, 44, 21), (29, 255, 107)]
    data = encoded([list(values) for values in zip(*pixels)], enumeration, method=1)
    yield image_case('sycc-pdf-override', data, b'/Filter /JPXDecode /ColorSpace /DeviceRGB', [], 3, 1,
                     colors=pixels, requirement=requirement)
    yield image_case('sycc-ignores-decode-default', data, b'/Filter /JPXDecode /Decode [1 0 1 0 1 0]', [], 3, 1,
                     resources=b'/ColorSpace << /DefaultRGB /DeviceGray >>',
                     colors=list(map(rgb, pixels)), requirement=requirement)
    alphas = [0, 128, 255]
    data = encoded([[76] * 3, [85] * 3, [255] * 3, alphas], enumeration, method=1, alpha=1)
    yield image_case('sycc-straight-alpha', data, b'/Filter /JPXDecode /SMaskInData 1', [], 3, 1,
                     colors=[rgb((76, 85, 255))] * 3, alpha=[F(a, 255) for a in alphas],
                     tolerance=1, requirement=requirement)
    # Unassociation precedes colour conversion: the middle sample recovers
    # exactly [127.5, 127.5, 127.5], not [128, 128, 128].
    data = encoded([[0, 64, 128]] * 3 + [alphas], enumeration, method=1, alpha=2)
    yield image_case('sycc-premultiplied-alpha', data, b'/Filter /JPXDecode /SMaskInData 2', [], 3, 1,
                     colors=[rgb((0, 0, 0)), rgb((F(255, 2),) * 3), rgb((128,) * 3)],
                     alpha=[F(a, 255) for a in alphas], tolerance=1, requirement=requirement)
    # Do not silently apply eight-bit code scaling to other precisions.
    data = encoded([[32768]] * 3, enumeration, method=1, bits=16)
    image = stream(data, b'/Subtype /Image /Width 1 /Height 1 /Filter /JPXDecode')
    yield Case('sycc-16bit-boundary', page(b'2 0 0 2 2 2 cm /I Do', [image], b'<< /XObject << /I 5 0 R >> >>'),
               UNSUPPORTED, (requirement,), 'Higher precision sYCC remains explicit', feature='xobject.jpx-color')
