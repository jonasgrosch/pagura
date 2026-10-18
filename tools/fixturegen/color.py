"""Raw ICC profiles for pagura-color, built by the same independent
ICC.1:2010 constructions as the content area's ICCBased cases, without a PDF.

Each case records the table values and tag entries its parser must admit,
counted from the construction, so budget boundaries are exact.
"""
from .content import icc_curve, icc_legacy, icc_pack, icc_profile, icc_tags
from .model import Vector

AREA = "color"


def cases():
    samples = tuple(int((i / 16) ** 2.2 * 65535 + .5) for i in range(17))
    # Three 17-sample TRCs; the matrix and white point are not table values.
    yield Vector("sampled-rgb",
                 {".icc": icc_profile(3, version=2, curve=icc_curve("sampled", samples))},
                 ("content-icc-matrix",),
                 "Version 2 RGB matrix/TRC profile with 17-sample curves (as content/icc-3-sampled)",
                 extra={"components": 3, "tags": 9, "table_values": 3 * 17})
    data, _ = icc_legacy(4, b"mft2", b"XYZ ")
    tags = {k: v for k, v in icc_tags(4, version=4).items() if k in (b"wtpt", b"desc", b"cprt")}
    tags[b"A2B0"] = data
    # Four two-sample input curves, 2^4 three-channel CLUT nodes and three
    # two-sample output curves.
    yield Vector("lut16-cmyk-xyz", {".icc": icc_pack(tags, 4, 4, pcs=b"XYZ ")},
                 ("content-icc-lut",),
                 "Version 4 CMYK lut16 profile (as content/icc-mft2-4-XYZ-v4)",
                 extra={"components": 4, "tags": 4, "table_values": 4 * 2 + 16 * 3 + 3 * 2})
