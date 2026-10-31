"""Independent Type 4 branch endpoint and full-pixel contracts (7.10.5)."""

import hashlib

from .model import OPEN, RENDER, UNSUPPORTED, Case, Vector
from .pdf import document, opaque_rectangles, stream

AREA = "calculator_branches"
REQUIREMENT = ("content-functions",)
FEATURE = "function.calculator"
PIECEWISE = b"{dup 0 ge {dup 1 sub abs 0.005 lt {pop 0.934 0.1935 0.1406 -5 } if } if 0 ge {1 0 0 } if }"
TWO_DEPTHS = b"{dup 0.5 gt {pop 0.2 0.4 0.6 -1} if 0 ge {0.6 0.8 0.2} if}"
THREE_DEPTHS = b"{dup 0.25 lt {pop 0.1 1} {dup 0.75 lt {pop 0.2 0.3 2} {pop 0.4 0.5 0.6 3} ifelse} ifelse dup 1 eq {pop} {dup 2 eq {pop pop} {pop pop pop} ifelse} ifelse}"
VALUES = [b"-1", b"0", b"0.5", b"0.994", b"0.995", b"0.9950000000000001", b"1", b"2"]
RED = (255, 0, 0)
SPECIAL = (238, 49, 36)


def tint(program, *, domain=b"[0 1]", outputs=3, values=VALUES):
    commands = b"0 0 1 rg 0 0 32 16 re f /C cs "
    commands += b" ".join(value + b" scn %d 0 4 12 re f" % (4 * index) for index, value in enumerate(values))
    commands += b" 0 1 0 rg 20 12 4 4 re f"
    target = b"/DeviceGray" if outputs == 1 else b"/DeviceRGB"
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 16] /Resources << /ColorSpace << /C [/Separation /Audit " + target + b" 5 0 R] >> >> /Contents 4 0 R >>",
        stream(commands),
        stream(program, b"/FunctionType 4 /Domain " + domain + b" /Range [" + b"0 1 " * outputs + b"] "),
    ])


def oracle(colors):
    return opaque_rectangles(32, 16, [
        ((0, 0, 32, 16), (0, 0, 255)),
        *[((4 * index, 0, 4, 12), color) for index, color in enumerate(colors)],
        ((20, 12, 4, 4), (0, 255, 0)),
    ])


def axial():
    # Pixel centers are (x+1/2)/400. Only x=398,399 exceed .995;
    # neither sample is near the floating point boundary.
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 8] /Resources << /Shading << /S 5 0 R >> >> /Contents 4 0 R >>",
        stream(b"/S sh"),
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 400 0] /Domain [0 1] /Function 6 0 R /Extend [true true] >>",
        stream(PIECEWISE, b"/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] "),
    ])
    expected = opaque_rectangles(400, 8, [((0, 0, 398, 8), RED), ((398, 0, 2, 8), SPECIAL)])
    return Case("piecewise-axial", pdf, RENDER, REQUIREMENT,
                "Independent center-sampled piecewise axial colors, including both branches", oracle=expected)


def cases():
    yield Case("piecewise-endpoints", tint(PIECEWISE), RENDER, REQUIREMENT,
               "Independent eight-column endpoint colors and green continuation witness",
               oracle=oracle([RED] * 5 + [SPECIAL] * 3))
    yield Case("two-depth-endpoints", tint(TWO_DEPTHS), RENDER, REQUIREMENT,
               "Two unequal intermediate depths normalize to three numeric colors",
               oracle=oracle([(153, 204, 51)] * 3 + [(51, 102, 153)] * 5))
    for name, program, domain in [
        ("bounded-ge", b"{dup 0 ge {pop 0.2 0.4 0.6} if}", b"[0 1]"),
        ("bounded-le", b"{dup 0 le {pop 0.2 0.4 0.6} if}", b"[-1 0]"),
        ("bounded-gt", b"{dup 0 gt {pop 0.2 0.4 0.6} if}", b"[0.25 1]"),
        ("bounded-lt", b"{dup 0 lt {pop 0.2 0.4 0.6} if}", b"[-1 -0.25]"),
    ]:
        yield Case(name, tint(program, domain=domain), RENDER, REQUIREMENT,
                   "Validated clamped Domain proves this comparison for every input",
                   oracle=oracle([(51, 102, 153)] * 8))
    yield Case("clamped-domain-red", tint(PIECEWISE, domain=b"[0.25 0.75]"), RENDER, REQUIREMENT,
               "Every external tint clamps inside a wholly red Domain interval", oracle=oracle([RED] * 8))
    yield Case("existing-same-depth", tint(b"{0.5 gt {1 0 0} {0 0 1} ifelse}"), RENDER, REQUIREMENT,
               "Existing equal-depth proof and colors are preserved",
               oracle=oracle([(0, 0, 255)] * 3 + [RED] * 5))
    yield axial()
    for name, program, outputs in [
        ("three-depths", THREE_DEPTHS, 1),
        ("branch-type-mismatch", b"{0.5 gt {true} {1} ifelse pop 1 0 0}", 3),
        ("arithmetic-facts-unproved", b"{dup 0.5 gt {pop 0.2 0.4 0.6 -1} if 0 add 0 ge {1 0 0} if}", 3),
    ]:
        yield Vector(name, {".ps": program}, REQUIREMENT,
                     "Direct Function API proof-boundary contract, independently checked in unit tests",
                     extra={"domain": [[0, 1]], "range": [[0, 1]] * outputs,
                            "expected": UNSUPPORTED, "feature": FEATURE})
    for name, program, domain, outputs in [
        ("output-mismatch", b"{dup 0.5 gt {pop 1 0 -1} if 0 ge {1 0 0} if}", b"[0.75 1]", 3),
        ("negative-domain-empty-path", PIECEWISE, b"[-1 -0.25]", 3),
        ("unknown-copy", b"{cvi copy}", b"[0 1]", 3),
        ("unknown-index", b"{cvi index}", b"[0 1]", 3),
    ]:
        yield Case(name, tint(program, domain=domain, outputs=outputs), UNSUPPORTED, REQUIREMENT,
                   "Unproved paths preserve the original calculator feature refusal", feature=FEATURE)
    for name, program, error in [
        ("numeric-underflow", b"{pop pop 1 0 0}", "syntax"),
        ("wrong-result-type", b"{pop true 0 0}", "syntax"),
        ("wrong-condition-type", b"{pop 1 {1 0 0} if}", "syntax"),
        ("stack-overflow", b"{pop " + b"1 " * 101 + b"}", "limit"),
    ]:
        yield Case(name, tint(program), OPEN, REQUIREMENT,
                   "Existing malformed paths and architectural stack limit remain authoritative",
                   extra={"render_error": error})

    yield from additional_cases()


def additional_cases():
    colors = [RED] * 5 + [SPECIAL] * 3
    expected = opaque_rectangles(32, 32, [
        ((0, 0, 32, 32), (0, 0, 255)),
        *[((4 * index, 0, 4, 28), color) for index, color in enumerate(colors)],
        ((20, 28, 4, 4), (0, 255, 0)),
    ])
    for name in ["piecewise-square", "piecewise-square-direct-twin"]:
        commands = b"0 0 1 rg 0 0 32 32 re f "
        resources = b"<< >>"
        if name == "piecewise-square":
            resources = b"<< /ColorSpace << /C [/Separation /Audit /DeviceRGB 5 0 R] >> >>"
            commands += b"/C cs " + b" ".join(value + b" scn %d 0 4 28 re f" % (4 * index) for index, value in enumerate(VALUES))
        else:
            commands += b" ".join((b"1 0 0" if index < 5 else b"0.934 0.1935 0.1406") + b" rg %d 0 4 28 re f" % (4 * index) for index in range(8))
        commands += b" 0 1 0 rg 20 28 4 4 re f"
        objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources " + resources + b" /Contents 4 0 R >>",
            stream(commands),
        ]
        if name == "piecewise-square":
            objects.append(stream(PIECEWISE, b"/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] "))
        yield Case(name, document(objects), RENDER, REQUIREMENT,
                   "Independent square branch feature/direct-RGB memory twin;256 square at576DPI",
                   oracle=expected)
    later_oracle = opaque_rectangles(32, 32, [
        ((0, 0, 32, 32), (0, 0, 255)), ((4, 4, 8, 8), SPECIAL), ((20, 20, 4, 4), (0, 255, 0)),
    ])
    first_oracle = opaque_rectangles(32, 32, [
        ((0, 0, 32, 32), (0, 0, 255)), ((4, 4, 8, 8), RED), ((20, 20, 4, 4), (0, 255, 0)),
    ])
    def content(value):
        return b"0 0 1 rg 0 0 32 32 re f /C cs " + value + b" scn 4 4 8 8 re f 0 1 0 rg 20 20 4 4 re f"
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /ColorSpace << /C [/Separation /Audit /DeviceRGB 5 0 R] >> >> /Contents 6 0 R >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /ColorSpace << /C [/Separation /Audit /DeviceRGB 5 0 R] >> >> /Contents 7 0 R >>",
        stream(PIECEWISE, b"/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] "),
        stream(content(b"0.5")), stream(content(b"1")),
    ])
    yield Case("shared-function-pages", pdf, RENDER, REQUIREMENT,
               "A shared function takes different branches on first and later pages", oracle=first_oracle, pages=2,
               extra={"qualification_pages": [{"page": 1, "dpi": 72,
                       "oracle": "corpus/generated/calculator_branches/shared-function-pages-p1.ppm",
                       "oracle_sha256": hashlib.sha256(later_oracle).hexdigest(), "width": 32, "height": 32}]})
    yield Vector("shared-function-pages-p1", {".ppm": later_oracle}, REQUIREMENT,
                 "Independent complete later-page pixel oracle", extra={"role": "independent-later-page-oracle", "page": 1})
