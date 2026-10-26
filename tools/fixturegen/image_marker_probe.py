"""Private, primary-derived DNL/PLM/zero-CRG diagnostic controls.

T.81 B.2.5/E.2.4; T.800 A.7.2/A.9.1; ISO 32000-2 7.4.8-9.
Only fixture authoring runs locally; no decoder or renderer supplies an oracle.
"""

import struct

from . import image, jpx
from .paint import image_case

AREA = "image_marker_probe"
WIDTH = 8
HEIGHT = 8


def pdf(name, data, filter_name, samples, requirement):
    return image_case(
        name,
        data,
        b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /" + filter_name,
        samples,
        WIDTH,
        HEIGHT,
        matrix=(WIDTH, 0, 0, HEIGHT, 2, 2),
        requirement=requirement,
    )


def rgb(name, samples, requirement):
    return image_case(
        name,
        bytes(value for sample in samples for value in (sample,) * 3),
        b"/ColorSpace /DeviceRGB /BitsPerComponent 8",
        samples,
        WIDTH,
        HEIGHT,
        matrix=(WIDTH, 0, 0, HEIGHT, 2, 2),
        requirement=requirement,
    )


def constant_frame(progressive=False):
    # The existing literal one-DC-zero/one-EOB tables code one complete MCU.
    # Increasing the advertised grid from 1x1 to 8x8 keeps precisely one MCU.
    raw, _ = image.constant_jpeg(progressive=progressive)
    data = bytearray(raw)
    sof = data.index(b"\xff\xc2" if progressive else b"\xff\xc0")
    data[sof + 5:sof + 9] = struct.pack(">HH", HEIGHT, WIDTH)
    return bytes(data), sof


def dnl_cases():
    samples = [128] * (WIDTH * HEIGHT)
    baseline, sof = constant_frame()
    for name, initial in [("deferred", 0), ("same-height", HEIGHT), ("redefined", 16)]:
        data = bytearray(baseline)
        data[sof + 5:sof + 7] = struct.pack(">H", initial)
        # DNL follows scan1 after its one complete MCU row, before EOI.
        data[-2:-2] = image.segment(0xdc, struct.pack(">H", HEIGHT))
        yield pdf("jpeg-dnl-" + name, bytes(data), b"DCTDecode", samples, "image-dct")
    progressive, sof = constant_frame(progressive=True)
    data = bytearray(progressive)
    data[sof + 5:sof + 7] = b"\0\0"
    second_scan = data.index(b"\xff\xda", data.index(b"\xff\xda") + 2)
    data[second_scan:second_scan] = image.segment(0xdc, struct.pack(">H", HEIGHT))
    yield pdf("jpeg-dnl-progressive", bytes(data), b"DCTDecode", samples, "image-dct")
    yield pdf("jpeg-dnl-baseline-control", baseline, b"DCTDecode", samples, "image-dct")
    yield pdf("jpeg-dnl-progressive-control", progressive, b"DCTDecode", samples, "image-dct")
    yield rgb("jpeg-dnl-rgb-control", samples, "image-dct")


def packet_length(value):
    result = [value & 127]
    value >>= 7
    while value:
        result.insert(0, 128 | (value & 127))
        value >>= 7
    return bytes(result)


def marker_cases():
    # Literal input samples are the oracle. Reversible transform, one packet.
    samples = [0, 64, 128, 255, 255, 128, 64, 0] * HEIGHT
    raw = jpx.codestream([samples], WIDTH, HEIGHT)
    sot = raw.index(b"\xff\x90")
    sod = raw.index(b"\xff\x93")
    # One packet occupies everything between SOD and EOC. PLM includes its
    # inline packet header. Nplm is the encoded byte count, not packet count.
    length = packet_length(len(raw) - sod - 4)
    plm = jpx.marker(0x57, b"\0" + bytes([len(length)]) + length)
    crg = jpx.marker(0x63, struct.pack(">HH", 0, 0))
    for name, markers in [("plm", plm), ("crg-zero", crg), ("plm-crg-zero", plm + crg)]:
        stream = raw[:sot] + markers + raw[sot:]
        yield pdf(
            "jpx-" + name,
            jpx.jp2(stream, WIDTH, HEIGHT, 1),
            b"JPXDecode",
            samples,
            "jpx-marker-syntax",
        )
    yield pdf("jpx-marker-codec-control", jpx.jp2(raw, WIDTH, HEIGHT, 1),
              b"JPXDecode", samples, "jpx-marker-syntax")
    yield rgb("jpx-marker-rgb-control", samples, "jpx-marker-syntax")
    # T.801 M.8/M.9.2 requires jpx compatibility and jpxb for baseline files.
    # Cor4 adds MinV=1. Its corrected TableM.14 feature5 covers this Rsiz=0
    # Part1 codestream; feature5 contributes to both one-bit requirements masks.
    signature = jpx.box(b"jP  ", b"\r\n\x87\n")
    requirements = jpx.box(b"rreq", struct.pack(">BBBHHBH", 1, 1, 1, 1, 5, 1, 0))
    header = jpx.box(b"ihdr", struct.pack(">IIH4B", HEIGHT, WIDTH, 1, 7, 7, 0, 0))
    header += jpx.box(b"colr", b"\x01\x00\x01" + struct.pack(">I", 17))
    for name, compatibility in [("jpxb-only", b"jpx jpxb"),
                                ("jpxb-jp2-control", b"jpx jpxbjp2 ")]:
        container = (signature + jpx.box(b"ftyp", b"jpx " + struct.pack(">I", 1) + compatibility)
                     + requirements + jpx.box(b"jp2h", header) + jpx.box(b"jp2c", raw))
        yield pdf(name, container, b"JPXDecode", samples, "jpx-boxes")


def cases():
    yield from dnl_cases()
    yield from marker_cases()
