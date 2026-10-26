"""T.800 high-precision sample pairs, independent of every reader.

A.5.1/Table A.11 admits 1--38 sample bits, subject to coefficient bounds.
E-2 gives Mb=G+epsilon-1. The 38-bit subject avoids the single unsigned
endpoint whose centered magnitude cannot fit the maximum 37-bit Mb.
I.5.3.3/Table I.10 maps unsigned gray to the sRGB nonlinearity.
"""

import hashlib
import struct
from dataclasses import replace

from . import jpx
from .paint import image_case

AREA = 'jpx_precision_probe'
REQUIREMENT = 'jpx-high-precision'
PRECISIONS = (17, 24, 31, 32, 38)
COLORS = [(value,) * 3 for value in (0, 64, 128, 255)]


def codestream(values, bits):
    """One reversible, untransformed four-sample block with explicit QCD.

    Reuse the independently authored packet/MQ encoder; only SIZ and QCD
    differ from the ordinary 16-bit control. No floating-point encoder or
    forward transform participates in these chosen coefficient vectors.
    """
    width = len(values)
    exponent, guard = min(bits, 31), max(2, bits - 31)
    maximum = exponent + guard - 1
    centered = [value - (1 << (bits - 1)) for value in values]
    assert all(abs(value).bit_length() <= maximum for value in centered)
    siz = struct.pack('>H8IH', 0, width, 1, 0, 0, width, 1, 0, 0, 1)
    siz += bytes([bits - 1, 1, 1])
    # LRCP, one layer, no MCT/DWT, 64x64 block, ordinary MQ, reversible.
    coding = bytes([0, 0, 0, 1, 0, 0, 4, 4, 0, 1])
    quantization = bytes([guard << 5, exponent << 3])
    header, payload = jpx.packet([[centered]], [maximum], [(0, 0)])
    body = header + payload
    return (b'\xff\x4f' + jpx.marker(0x51, siz) + jpx.marker(0x52, coding)
            + jpx.marker(0x5c, quantization)
            + jpx.marker(0x90, struct.pack('>HIBB', 0, 14 + len(body), 0, 1))
            + b'\xff\x93' + body + b'\xff\xd9')


def samples(bits):
    # Literal integer expectations preserve low-order bits even when the
    # display output is the same byte. For 38 bits, -2**37+1 fits Mb=37.
    return [1, 1 << (bits - 2), 1 << (bits - 1), (1 << bits) - 1]


def cases():
    control_values = [0, 16384, 32768, 65535]
    control_raw = codestream(control_values, 16)
    for bits in PRECISIONS:
        name = f'unsigned-{bits}'
        values = samples(bits)
        raw = codestream(values, bits)
        # I.5.3.3: d=x/(2**bits-1). Integer nearest-byte conversion is
        # independent of the product's float colour/sample implementation.
        maximum = (1 << bits) - 1
        expected = [(255 * value * 2 + maximum) // (maximum * 2) for value in values]
        assert expected == [0, 64, 128, 255]
        yield jpx.vector(name + '-native', raw, [values], 4, 1,
                         requirements=(REQUIREMENT,), bits=bits)
        for suffix, role, source, depth in [
            ('', 'subject', raw, bits),
            ('-codec-control', 'codec-control', control_raw, 16),
        ]:
            data = jpx.jp2(source, 4, 1, 1, bits=depth)
            value = image_case(name + suffix, data, b'/Filter /JPXDecode', [], 4, 1,
                               matrix=(4, 0, 0, 1, 2, 2), colors=COLORS,
                               tolerance=1, requirement=REQUIREMENT)
            yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}',
                                        'role': role, 'precision': depth,
                                        'codestream_sha256': hashlib.sha256(source).hexdigest()})
        value = image_case(name + '-rgb-control', bytes(v for color in COLORS for v in color),
                           b'/ColorSpace /DeviceRGB /BitsPerComponent 8', [], 4, 1,
                           matrix=(4, 0, 0, 1, 2, 2), colors=COLORS, requirement=REQUIREMENT)
        yield replace(value, extra={**value.extra, 'control_group': f'{AREA}/{name}', 'role': 'rgb-control'})
