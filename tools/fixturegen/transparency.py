"""Clause 11 group colour fixtures with independent component-space oracles.

Separable modes use additive complements of all four CMYK channels (11.3.4).
Non-separable modes use complementary CMY and select K separately (11.3.5.3).
Normal composition uses rational arithmetic; the other modes use Python f64.
ICC destination fallback follows 8.6.5.5, Table 65. Calibrated destinations
remain explicit unsupported cases until reversible colour transforms exist.
"""
from fractions import Fraction as F

from .content import (
    blend_colour, ICC_D50, ICC_RGB_MATRIX, icc_fixed, icc_legacy, icc_pack, icc_profile, icc_tags,
)
from .model import Case, RENDER, UNSUPPORTED
from .pdf import document, opaque_rectangles, single_page, stream

AREA = "transparency"
MODES = (
    "Normal", "Compatible", "Multiply", "Screen", "Overlay", "Darken", "Lighten",
    "ColorDodge", "ColorBurn", "HardLight", "SoftLight", "Difference", "Exclusion",
    "Hue", "Saturation", "Color", "Luminosity",
)
SPACES = {"gray": b"/DeviceGray", "rgb": b"/DeviceRGB", "cmyk": b"/DeviceCMYK"}


def byte(value):
    return int(min(1, max(0, value)) * 255 + F(1, 2))


def additive(model, values):
    return tuple(1 - v if model == "cmyk" else v for v in values)


def rgb(model, color):
    if model == "gray":
        return (color[0],) * 3
    if model == "cmyk":
        return tuple(max(0, v + color[3] - 1) for v in color[:3])
    return tuple(color)


def convert(source, target, color):
    if source == target:
        return color
    color = rgb(source, color)
    if target == "gray":
        return (sum(a * b for a, b in zip(color, (F(3, 10), F(59, 100), F(11, 100)))),)
    if target == "cmyk":
        # Clause 10.3.4: common-ink BG/UCR is the renderer's device policy.
        inks = [1 - v for v in color]
        k = min(inks)
        return tuple(1 - (v - k) for v in inks) + (1 - k,)
    return color


def blend(model, mode, back, source):
    if mode in ("Normal", "Compatible"):
        return source
    if mode in ("Hue", "Saturation", "Color", "Luminosity"):
        b = back * 3 if model == "gray" else back[:3]
        s = source * 3 if model == "gray" else source[:3]
        result = tuple(blend_colour(mode, b, s))
        if model == "gray":
            return (sum(a * b for a, b in zip(result, (.3, .59, .11))),)
        if model == "cmyk":
            return result + (source[3] if mode == "Luminosity" else back[3],)
        return result
    return tuple(blend_colour(mode, back, source))


def over(back, source, alpha, model="rgb", mode="Normal"):
    blended = blend(model, mode, back, source)
    return tuple((1 - alpha) * b + alpha * s for b, s in zip(back, blended))


def paint(model, values, rectangle=b"0 0 8 8"):
    operator = {"gray": "g", "rgb": "rg", "cmyk": "k"}[model]
    numbers = " ".join(str(float(v)) for v in values)
    return numbers.encode() + b" " + operator.encode() + b" " + rectangle + b" re f "


def form(content, space=None, resources=b"<< >>", isolated=True, knockout=False):
    attributes = b"/S /Transparency /I " + (b"true" if isolated else b"false")
    if space is not None:
        attributes += b" /CS " + space
    if knockout:
        attributes += b" /K true"
    return stream(content, b"/Type /XObject /Subtype /Form /BBox [0 0 8 8] /Group << "
                  + attributes + b" >> /Resources " + resources)


def pdf(content, resources=b"<< >>", space=None, objects=(), intent=b""):
    page_group = b" /Group << /S /Transparency /CS " + space + b" >>" if space else b""
    return document([
        b"<< /Type /Catalog /Pages 2 0 R" + (b" " + intent if intent else b"") + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources " + resources
        + b" /Contents 4 0 R" + page_group + b" >>",
        stream(content), *objects,
    ])


def rendered(name, data, colors, requirement, description):
    geometry = [((i, 0, 1, 8), tuple(map(byte, color))) for i, color in enumerate(colors)]
    return Case(name, data, RENDER, (requirement,), description,
                oracle=opaque_rectangles(8, 8, geometry), extra={"tolerance": 1})


def uniform(name, data, color, requirement, description):
    return rendered(name, data, [color] * 8, requirement, description)


def device_modes():
    pairs = [
        ((F(1, 5), F(2, 5), F(3, 5), F(1, 10)), (F(4, 5), F(3, 10), F(1, 10), F(3, 5))),
        ((0, 0, 0, 0), (1, 1, 1, 1)),
        ((1, 1, 1, 1), (0, 0, 0, 0)),
        ((F(1, 10), F(1, 5), F(9, 10), F(4, 5)), (F(9, 10), F(4, 5), F(1, 10), F(1, 5))),
        ((F(1, 4),) * 4, (F(7, 10), F(3, 10), F(1, 2), F(1, 10))),
        ((F(9, 10), F(1, 10), F(1, 2), F(1, 5)), (F(1, 5), F(4, 5), F(3, 5), F(7, 10))),
        ((F(3, 4), F(1, 2), F(1, 5), F(2, 5)), (F(2, 5), F(3, 5), F(4, 5), F(1, 10))),
        ((F(1, 100), F(99, 100), F(1, 5), F(3, 10)), (F(99, 100), F(1, 100), F(4, 5), F(3, 4))),
    ]
    for model in ("gray", "cmyk"):
        n = 1 if model == "gray" else 4
        for mode in MODES:
            content = b""
            colors = []
            for i, (back, source) in enumerate(pairs):
                back, source = back[:n], source[:n]
                rect = f"{i} 0 1 8".encode()
                content += b"q " + paint(model, back, rect) + b" /A gs " + paint(model, source, rect) + b" Q "
                result = over(additive(model, back), additive(model, source), F(1, 2), model, mode)
                colors.append(rgb(model, result))
            resources = b"<< /ExtGState << /A << /ca .5 /BM /" + mode.encode() + b" >> >> >>"
            yield rendered(model + "-blend-" + mode, pdf(content, resources, SPACES[model]), colors,
                           "transparency-subtractive-blends", "11.3.3–11.3.5: native components, half opacity, boundary values and independent blend formulas")


def boundaries():
    values = {"gray": (F(1, 4),), "rgb": (F(1, 5), F(3, 5), F(4, 5)),
              "cmyk": (F(4, 5), F(1, 5), F(3, 5), F(1, 2))}
    state = b"<< /ExtGState << /A << /ca .5 >> >> >>"
    for parent in SPACES:
        for child in SPACES:
            b = additive(parent, values[parent])
            s = additive(child, values[child])
            # The child's opaque result is converted once; invocation alpha is .5.
            expected = rgb(parent, over(b, convert(child, parent, s), F(1, 2)))
            resources = b"<< /XObject << /G 5 0 R >> /ExtGState << /A << /ca .5 >> >> >>"
            data = pdf(paint(parent, values[parent]) + b" /A gs /G Do", resources, SPACES[parent],
                       [form(paint(child, values[child]), SPACES[child])])
            yield uniform(f"boundary-{child}-to-{parent}", data, expected,
                          "transparency-group-boundaries", "11.4.7/11.6.6: isolated group conversion precedes parent-space composition")
    # Gray nested in RGB nested in CMYK; each boundary has its own opacity.
    gray = (F(1, 4),)
    middle = over((F(1, 5), F(3, 5), F(4, 5)), convert("gray", "rgb", gray), F(1, 2))
    final = over(additive("cmyk", values["cmyk"]), convert("rgb", "cmyk", middle), F(1, 2))
    resources = b"<< /XObject << /G 5 0 R >> /ExtGState << /A << /ca .5 >> >> >>"
    middle_content = paint("rgb", values["rgb"]) + b" /A gs /G Do"
    data = pdf(paint("cmyk", values["cmyk"]) + b" /A gs /G Do", resources, SPACES["cmyk"],
               [form(middle_content, SPACES["rgb"], resources.replace(b"5 0 R", b"6 0 R")),
                form(paint("gray", gray), SPACES["gray"])])
    yield uniform("nested-gray-rgb-cmyk", data, rgb("cmyk", final),
                  "transparency-group-boundaries", "11.6.6: three different nested blending spaces, with opacity at both boundaries")
    # Non-isolated groups inherit their parent's space.
    b = (F(1, 5), F(1, 2), F(4, 5), F(2, 5))
    s = (F(9, 10), F(1, 5), F(3, 10), F(1, 10))
    data = pdf(paint("cmyk", b) + b" /G Do", b"<< /XObject << /G 5 0 R >> >>", SPACES["cmyk"],
               [form(b"/A gs " + paint("cmyk", s), resources=state, isolated=False)])
    yield uniform("nonisolated-inherits-cmyk", data,
                  rgb("cmyk", over(additive("cmyk", b), additive("cmyk", s), F(1, 2))),
                  "transparency-group-boundaries", "11.6.6/11.7.2: non-isolated groups use the inherited component space")
    for ais in (False, True):
        b, s = (1, 0, 0, 1), (0, 0, 0, 0)
        state = b"<< /ExtGState << /A << /ca .5 /AIS " + (b"true" if ais else b"false") + b" >> >> >>"
        data = pdf(b"/G Do", b"<< /XObject << /G 5 0 R >> >>", objects=[
            form(paint("cmyk", b) + b" /A gs " + paint("cmyk", s), SPACES["cmyk"], state, knockout=True)])
        color = rgb("cmyk", over(additive("cmyk", b), additive("cmyk", s), F(1, 2))) if ais else (1, 1, 1)
        yield uniform("cmyk-knockout-ais-" + str(int(ais)), data, color,
                      "transparency-native-components", "11.4.6: CMYK knockout replaces by shape; AIS moves constant alpha into shape")


def soft_masks():
    for method in ("Alpha", "Luminosity"):
        for transfer in (False, True):
            native = (F(4, 5), F(1, 5), F(2, 5), F(1, 2))
            backdrop = (F(1, 5), F(2, 5), F(3, 5), F(1, 10))
            color = over(additive("cmyk", backdrop), additive("cmyk", native), F(1, 2))
            # 11.5.3 converts group colour to gray before the transfer function.
            luminosity = max(0, sum(a * b for a, b in zip(color[:3], (F(3, 10), F(59, 100), F(11, 100)))) + color[3] - 1)
            outside = max(0, sum(a * b for a, b in zip(additive("cmyk", backdrop)[:3], (F(3, 10), F(59, 100), F(11, 100)))) + (1 - backdrop[3]) - 1)
            inside, outside = (F(1, 2), 0) if method == "Alpha" else (luminosity, outside)
            if transfer:
                inside, outside = inside ** 2, outside ** 2
            options = b" /TR << /FunctionType 2 /Domain [0 1] /N 2 >>" if transfer else b""
            mask = b"<< /S /" + method.encode() + b" /G 6 0 R /BC [.2 .4 .6 .1]" + options + b" >>"
            outer_resources = b"<< /ExtGState << /M << /SMask " + mask + b" >> >> >>"
            data = pdf(b"/G Do", b"<< /XObject << /G 5 0 R >> >>", objects=[
                form(b"/M gs " + paint("cmyk", (1, 0, 0, 0)), SPACES["cmyk"], outer_resources),
                form(b"/A gs " + paint("cmyk", native, b"0 0 4 8"), SPACES["cmyk"],
                     b"<< /ExtGState << /A << /ca .5 >> >> >>"),
            ])
            colors = [(1 - inside, 1, 1)] * 4 + [(1 - outside, 1, 1)] * 4
            yield rendered("cmyk-mask-" + method + ("-transfer" if transfer else ""), data, colors,
                           "transparency-cmyk-masks", "11.5.2–11.5.3/11.6.5.1: masks inside CMYK, CMYK luminosity backdrop and transfer")


def profile(count, version=4):
    if count != 4:
        return icc_profile(count, version=version)
    tags = icc_tags(4, version=version)
    tags[b"A2B0"] = icc_legacy(4)[0]
    return icc_pack(tags, 4, version=version, pcs=b"Lab ")


def managed(count, values):
    """Independent profile forward model, retaining f64 through blending.

    ICC.1:2010 matrix/TRC and legacy Lab LUT encodings, followed by Bradford
    adaptation and sRGB. Solve the primary matrices rather than borrowing the
    renderer's inverse constants. These are the profiles authored by profile().
    """
    def apply(m, v):
        return [sum(a * b for a, b in zip(row, v)) for row in m]

    def solve(m, v):
        rows = [list(row) + [x] for row, x in zip(m, v)]
        for i in range(3):
            pivot = max(range(i, 3), key=lambda j: abs(rows[j][i]))
            rows[i], rows[pivot] = rows[pivot], rows[i]
            scale = rows[i][i]
            rows[i] = [x / scale for x in rows[i]]
            for j in range(3):
                if i != j:
                    scale = rows[j][i]
                    rows[j] = [a - scale * b for a, b in zip(rows[j], rows[i])]
        return [row[3] for row in rows]

    gamma = int(2.2 * 256 + .5) / 256
    decoded = [min(1, max(0, float(v))) ** gamma for v in values]
    if count == 1:
        xyz = [w * decoded[0] for w in ICC_D50]
    elif count == 3:
        matrix = [icc_fixed(v) / 65536 for v in ICC_RGB_MATRIX]
        xyz = [sum(matrix[i + 3*j] * decoded[j] for j in range(3)) for i in range(3)]
    else:
        _, reference = icc_legacy(4)
        l, a, b = [min(1, max(0, v * 65535 / 65280)) for v in reference(values)]
        m = (100 * l + 16) / 116
        f = lambda t: t**3 if t >= 6/29 else 3 * (6/29)**2 * (t - 4/29)
        xyz = [ICC_D50[0] * f(m + (255*a - 128)/500),
               f(m), ICC_D50[2] * f(m - (255*b - 128)/200)]
    bradford = [[.8951, .2664, -.1614], [-.7502, 1.7135, .0367], [.0389, -.0685, 1.0296]]
    d65 = [.95047, 1, 1.08883]
    xyz = solve(bradford, [x*t/w for x, t, w in zip(apply(bradford, xyz),
                                                  apply(bradford, d65), apply(bradford, ICC_D50))])
    primaries = [( .64, .33), (.30, .60), (.15, .06)]
    basis = [[(x/y, 1, (1-x-y)/y)[i] for x, y in primaries] for i in range(3)]
    scale = solve(basis, d65)
    linear = solve([[v*k for v, k in zip(row, scale)] for row in basis], xyz)
    def encode(v):
        v = min(1, max(0, v))
        return 12.92*v if v <= .0031308 else 1.055*v**(1/2.4) - .055
    return tuple(map(encode, linear))


def icc_groups():
    for model, count, b, s in [
        ("gray", 1, (F(1, 5),), (F(4, 5),)),
        ("rgb", 3, (F(1, 5), F(2, 5), F(3, 5)), (F(4, 5), F(3, 10), F(1, 10))),
        ("cmyk", 4, (1, 0, 0, 1), (0, 0, 0, 0)),
    ]:
        for alternate in (False, True):
            cs = b"[/ICCBased 6 0 R]"
            resources = b"<< /ColorSpace << /C " + cs + b" >> /ExtGState << /A << /ca .5 >> >> >>"
            def mark(values):
                return b"/C cs " + " ".join(str(float(v)) for v in values).encode() + b" scn 0 0 8 8 re f "
            extra = b" /Alternate " + SPACES[model] if alternate else b""
            data = pdf(b"/G Do", b"<< /XObject << /G 5 0 R >> >>", objects=[
                form(mark(b) + b" /A gs " + mark(s), cs, resources),
                stream(profile(count), b"/N " + str(count).encode() + extra),
            ])
            result = rgb(model, over(convert("rgb", model, managed(count, b)),
                                     convert("rgb", model, managed(count, s)), F(1, 2)))
            yield uniform("icc-" + model + ("-alternate" if alternate else "-default"), data, result,
                          "transparency-icc-fallback", "8.6.5.5 Table 65: unsupported ICC destination falls back before blending; ICC sources keep their forward transforms")
    # Source ICC conversion still applies outside its own fallback group.
    cs = b"[/ICCBased 6 0 R]"
    resources = b"<< /ColorSpace << /C " + cs + b" >> /XObject << /G 5 0 R >> >>"
    content = b"/C cs .5 .5 .5 scn 0 0 4 8 re f /G Do"
    data = pdf(content, resources, objects=[
        form(b"/C cs .5 .5 .5 scn 4 0 4 8 re f", cs, resources), stream(profile(3), b"/N 3")])
    source_rgb = managed(3, (.5,) * 3)
    yield rendered("icc-source-and-group", data,
                   [source_rgb] * 8,
                   "transparency-icc-fallback", "8.6.5.5: group destination fallback does not disable ordinary ICC source conversion")
    # Default remapping and resource aliases must reach the same group model.
    for name, space, resources in [
        ("alias", b"/Alias", b"<< /ColorSpace << /Alias [/ICCBased 5 0 R] >> >>"),
        ("default", b"/DeviceCMYK", b"<< /ColorSpace << /DefaultCMYK [/ICCBased 5 0 R] >> >>"),
    ]:
        data = pdf(paint("cmyk", (F(1, 2), 0, 0, F(1, 4))), resources, space,
                   [stream(profile(4), b"/N 4 /Alternate /DeviceCMYK")])
        yield uniform("icc-cmyk-resource-" + name, data,
                      managed(4, (F(1, 2), 0, 0, F(1, 4))) if name == "default"
                      else (F(1, 4), F(3, 4), F(3, 4)),
                      "transparency-icc-fallback", "8.6.5.5–8.6.5.6: resource alias/default selection, without recursively remapping Alternate")
    # A later blend distinguishes the original black separation from an RGB
    # round trip, even when both give exactly the same displayed group colour.
    back = (F(1, 2), F(1, 4), F(1, 8), F(1, 4))
    source = (F(3, 10), F(1, 5), F(3, 5), F(1, 10))
    expected = rgb("cmyk", over(additive("cmyk", back), additive("cmyk", source),
                                F(1, 2), "cmyk", "Multiply"))
    resources = b"<< /XObject << /G 5 0 R >> /ExtGState << /A << /ca .5 /BM /Multiply >> >> >>"
    for name, parent, more in [
        ("device", SPACES["cmyk"], []),
        ("different-profile", b"[/ICCBased 7 0 R]", [stream(profile(4, version=2), b"/N 4")]),
    ]:
        data = pdf(b"/G Do /A gs " + paint("cmyk", source), resources, parent, [
            form(paint("cmyk", back), b"[/ICCBased 6 0 R]"),
            stream(profile(4), b"/N 4"), *more,
        ])
        yield uniform("icc-cmyk-" + name + "-boundary", data, expected,
                      "transparency-icc-fallback", "11.6.6: equivalent effective CMYK spaces preserve all four components before a subsequent blend")


def unsupported_spaces():
    definitions = [
        ("CalGray", b"[/CalGray << /WhitePoint [.95047 1 1.08883] /Gamma 2 >>]"),
        ("CalRGB", b"[/CalRGB << /WhitePoint [.95047 1 1.08883] /Gamma [2 2 2] >>]"),
        ("Lab", b"[/Lab << /WhitePoint [.95047 1 1.08883] >>]"),
    ]
    for name, space in definitions:
        yield Case("group-" + name, pdf(b"0 0 8 8 re f", space=space), UNSUPPORTED,
                   ("transparency-calibrated-boundary",),
                   "11.3.4: Lab is prohibited; calibrated destinations require reversible component conversion",
                   feature="transparency.group-colorspace")
    # Lab ICC input is not an admissible blending space, regardless of its N.
    tags = icc_tags(4)
    tags[b"A2B0"] = icc_legacy(3, constant=(.5, .5, .5))[0]
    data = pdf(b"0 0 8 8 re f", space=b"[/ICCBased 5 0 R]", objects=[
        stream(icc_pack(tags, 3, space=b"Lab ", pcs=b"Lab "), b"/N 3 /Range [0 100 -128 127 -128 127]")])
    yield Case("group-icc-Lab", data, UNSUPPORTED, ("transparency-calibrated-boundary",),
               "11.3.4 excludes ICC lightness/chromaticity input even with three components",
               feature="transparency.group-colorspace")


def overprint_boundaries():
    from .overprint import PDFX, WHITE, compose, rgb as separation_rgb

    state = b"/ExtGState << /A << /op true /OP true /OPM 1 >> >>"
    back = (F(0), F(3, 5), F(1, 5), F(1, 10))
    source = (F(128, 255), F(0), F(32, 255), F(0))
    image = stream(bytes([128, 0, 32, 0]),
                   b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceCMYK")
    values = b" ".join(str(float(v)).encode() for v in source)
    shading = (b"/Shading << /S << /ShadingType 2 /ColorSpace /DeviceCMYK /Coords [0 0 8 0] "
               b"/Extend [true true] /Function << /FunctionType 2 /Domain [0 1] "
               b"/C0 [" + values + b"] /C1 [" + values + b"] /N 1 >> >> >>")
    for name, command, resources, objects in [
        ("path", paint("cmyk", source), b"", []),
        ("image", b"8 0 0 8 0 0 cm /I Do", b"/XObject << /I 6 0 R >>", [image]),
        ("shading", b"/S sh", shading, []),
    ]:
        data = pdf(paint("cmyk", back) + b"/A gs " + command, b"<< " + state + b" " + resources + b" >>",
                   b"[/ICCBased 5 0 R]", [stream(profile(4), b"/N 4"), *objects], intent=PDFX)
        pixel = compose(compose(WHITE, dict(enumerate(back))), dict(enumerate(source)),
                        op=True, nonzero=name == "path")
        expected = tuple(F(v, 255) for v in separation_rgb(pixel))
        yield uniform("icc-cmyk-overprint-" + name, data, expected,
                      "transparency-overprint-boundary",
                      "11.7.4: ICC fallback shares compatible overprinting; nonzero selection applies to paths, not sampled images or shadings")

    for name, source, space in [
        ("matching-profile", (F(1, 2), F(0), F(0), F(0)), b"/C cs"),
        ("tiny-nonzero", (F(1, 2), F(1, 10**20), F(0), F(0)), b"/C cs"),
        ("clipped-tints", (F(1, 2), F(-1), F(0), F(0)), b"/C cs"),
        ("default-remap", (F(1, 2), F(0), F(0), F(0)), b"/DeviceCMYK cs"),
    ]:
        numbers = b" ".join(format(float(v), ".24f").rstrip("0").rstrip(".").encode() for v in source)
        resources = b"<< " + state + b" /ColorSpace << /C [/ICCBased 5 0 R] /DefaultCMYK [/ICCBased 5 0 R] >> >>"
        data = pdf(paint("cmyk", back) + b"/A gs " + space + b" " + numbers + b" scn 0 0 8 8 re f",
                   resources, b"[/ICCBased 5 0 R]", [stream(profile(4), b"/N 4")], intent=PDFX)
        # DefaultCMYK also colour-manages the backdrop; a supported ICC
        # source is not a native device overprint source (11.7.4.3).
        expected = managed(4, source)
        yield uniform("icc-cmyk-overprint-" + name, data,
                      expected, "transparency-overprint-boundary",
                      "8.6.5.5/11.7.4.3: ICC sources stay colour-managed; native zero selection is limited to device CMYK")

    # Named process components and spot plates remain independent inside ICC
    # fallback groups; only final output combines their ink equivalents.
    for name, space, components, tints in [
        ("named-process", b"/PC", b".5", {0: F(1, 2)}),
        ("spot", b"/S", b".5", {4: F(1, 2)}),
    ]:
        colors = (b"/ColorSpace << /PC [/Separation /Cyan /DeviceCMYK "
                  b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>] "
                  b"/S [/Separation /InkC /DeviceCMYK "
                  b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>] >>")
        resources = b"<< " + state + b" " + colors + b" >>"
        data = pdf(paint("cmyk", back) + b"/A gs " + space + b" cs " + components + b" scn 0 0 8 8 re f",
                   resources, b"[/ICCBased 5 0 R]", [stream(profile(4), b"/N 4")], intent=PDFX)
        expected = separation_rgb(compose(compose(WHITE, dict(enumerate(back))), tints, op=True))
        yield uniform("icc-cmyk-overprint-" + name, data, tuple(F(v, 255) for v in expected),
                      "transparency-overprint-boundary", "11.7.3–11.7.4: named process and spot components use the shared group compositor")


def cases():
    yield from device_modes()
    yield from boundaries()
    yield from soft_masks()
    yield from icc_groups()
    yield from unsupported_spaces()
    yield from overprint_boundaries()
