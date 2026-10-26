"""R02 embedded ICC integration: independent profiles, codestreams and pixels.

ISO 32000-2 7.4.9 / Table 87 and T.800 I.5.3.3. The existing independent
ICC.1:2010 profile builder supplies a unit TRC and D50-adapted sRGB primaries.
Expected pixels use the scalar IEC sRGB encoding of known linear samples;
they never call Pagura's transforms or another PDF engine.
"""
import struct
from fractions import Fraction as F

from . import content as colour, jpx
from .model import Case, OPEN
from .paint import image_case, page
from .pdf import stream

AREA = "paint_jpx"
REQUIREMENT = "paint-jpx-icc"


def encoded(planes, profile, *, alpha=0, method=2):
    width = len(planes[0])
    header = jpx.box(b'ihdr', struct.pack('>IIH4B', 1, width, len(planes), 7, 7, 0, 0))
    header += jpx.box(b'colr', bytes([method, 0, 0]) + profile)
    if alpha:
        entries = [(i, 0, i + 1) for i in range(len(planes) - 1)] + [(len(planes) - 1, alpha, 0)]
        header += jpx.box(b'cdef', struct.pack('>H', len(entries)) + b''.join(struct.pack('>HHH', *row) for row in entries))
    return (jpx.box(b'jP  ', b'\r\n\x87\n') + jpx.box(b'ftyp', b'jp2 \0\0\0\0jp2 ')
            + jpx.box(b'jp2h', header) + jpx.box(b'jp2c', jpx.codestream(planes, width, 1)))


def srgb(sample):
    linear = sample / 255
    return int(255 * (12.92 * linear if linear <= .0031308 else 1.055 * linear ** (1 / 2.4) - .055) + .5)


def cases():
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
