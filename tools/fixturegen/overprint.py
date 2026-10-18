"""Independent separation and compatible-overprint oracles (ISO 8.6.7; 11.7.4).

Six additive components represent CMYK plus two named spot plates. Composition
uses rational arithmetic; only the final display conversion rounds to bytes.
The display policy adds spot ink equivalents to CMYK with saturation. Geometry
is pixel aligned except for the explicitly integrated coverage case.
"""
from fractions import Fraction as F

from .model import Case, RENDER, OPEN
from .pdf import document, stream

AREA = "overprint"
FLAGS = "overprint-flags"
PROCESS = "overprint-process"
SPOT = "overprint-spot"
GROUP = "overprint-groups"
PREVIEW = "overprint-preview"
PDFX = b"/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (Test print) >>]"
WHITE = ([F(0)] * 6, F(0))  # transparent, premultiplied
CYAN = {0: F(1), 1: F(0), 2: F(0), 3: F(0)}
MAGENTA = {0: F(0), 1: F(1), 2: F(0), 3: F(0)}
RECT = b"0 0 8 8 re f "
BACK = b"0 1 0 0 k " + RECT
STATE = (b"/ExtGState << /On << /OP true /OPM 1 >> "
         b"/Off << /OP false /op false >> /Zero << /op true /OPM 0 >> "
         b"/Fill << /OP false /op true /OPM 1 >> "
         b"/Stroke << /OP true /op false /OPM 1 >> "
         b"/Half << /OP true /OPM 1 /ca .4 /CA .4 >> "
         b"/Screen << /OP true /OPM 1 /BM /Screen >> "
         b"/Difference << /OP true /OPM 1 /BM /Difference >> >> ")
COLORS = (b"/ColorSpace << "
          b"/C [/Separation /InkC /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>] "
          b"/M [/Separation /InkM /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 0 0] /N 1 >>] "
          b"/PC [/Separation /Cyan /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>] "
          b"/DN [/DeviceN [/InkC /InkM] /DeviceCMYK 5 0 R] "
          b"/PN [/DeviceN [/Cyan /Black] /DeviceCMYK 6 0 R] "
          b"/All [/Separation /All /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 1 1 1] /N 1 >>] "
          b"/I [/Indexed /DeviceCMYK 0 <ff000000>] >> ")
RESOURCES = b"<< " + STATE + COLORS + b"/XObject << /Fm 7 0 R >> >>"


def compose(back, tints, *, op=False, nonzero=False, alpha=F(1), blend="Normal"):
    """11.3.6 compositing and the component choice in Table 146.

    Nonzero applies only to direct DeviceCMYK painting, never sampled images
    or shadings. Other blend modes operate on the implicit group's result.
    """
    cb, ab = back
    straight = [v / ab if ab else F(0) for v in cb]
    source = [1 - tints.get(i, F(0)) for i in range(6)]
    preserve = [op and (i not in tints or (nonzero and tints[i] == 0)) for i in range(6)]
    result = []
    for i, (b, s) in enumerate(zip(straight, source)):
        mode = "Normal" if i >= 4 and blend == "Difference" else blend
        if mode == "Normal":
            mixed = b if preserve[i] else s
        else:
            s = (1 - ab) * s + ab * b if preserve[i] else s
            mixed = b + s - b * s if mode == "Screen" else abs(b - s)
        result.append((1 - alpha) * cb[i] + alpha * ((1 - ab) * s + ab * mixed))
    return result, alpha + ab * (1 - alpha)


def group(back, commands, *, isolated=False, alpha=F(1)):
    initial = WHITE if isolated else back
    pixel = initial
    ag = F(0)
    for tints, options in commands:
        pixel = compose(pixel, tints, **options)
        a = options.get("alpha", F(1))
        ag = a + ag * (1 - a)
    source = [(v - (1 - ag) * b) / ag if ag else F(0)
              for v, b in zip(pixel[0], initial[0])]
    # A completed group paints every process and spot component (Table 146).
    return compose(back, {i: 1 - v for i, v in enumerate(source)}, alpha=ag * alpha)


def rgb(pixel):
    color, alpha = pixel
    c = [v / alpha if alpha else F(1) for v in color]
    # InkC and InkM have cyan and magenta ink equivalents respectively.
    c[0] = max(F(0), c[0] + c[4] - 1)
    c[1] = max(F(0), c[1] + c[5] - 1)
    return tuple(int((max(F(0), c[i] + c[3] - 1) * alpha + 1 - alpha) * 255 + F(1, 2))
                 for i in range(3))


def pdf(content, *, form=b"", form_group=b"", page_group=b"", extra_resources=b"", extra_colors=b"", objects=(), intent=PDFX, trailer=b""):
    # Simulation oracles declare print intent; policy cases opt out explicitly.
    resources = (RESOURCES.replace(STATE, extra_resources) if extra_resources.startswith(b"/ExtGState")
                 else RESOURCES[:-2] + extra_resources + b">>")
    resources = resources.replace(COLORS, COLORS[:-3] + extra_colors + b">> ")
    return document([
        b"<< /Type /Catalog /Pages 2 0 R " + intent + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources " + resources
        + b" /Contents 4 0 R " + page_group + b">>",
        stream(content),
        stream(b"{ 0 0 }", b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1]"),
        stream(b"{ 0 exch 0 exch }", b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1 0 1]"),
        stream(form, b"/Type /XObject /Subtype /Form /BBox [0 0 8 8] /Resources "
               + RESOURCES + b" " + form_group),
        *objects,
    ], trailer_extra=trailer)


def case(name, content, expected, requirements=(PROCESS,), **options):
    color = rgb(expected)
    return Case(name, pdf(content, **options), RENDER, requirements, name,
                oracle=b"P6\n8 8\n255\n" + bytes(color) * 64,
                extra={"tolerance": 0})


def cases():
    # Identical paint commands: cyan replaces magenta without preview, whereas
    # OPM 1 preserves the magenta plate during simulation, giving blue.
    content = BACK + b"/On gs 1 0 0 0 k " + RECT
    for name, options, simulate in [
        ("preview-pdfx-intent", {}, True),
        ("preview-cmyk-group", {"page_group": b"/Group << /S /Transparency /CS /DeviceCMYK /I true >>"}, True),
        ("preview-no-intent", {"intent": b""}, False),
        ("preview-pdfx-info", {"intent": b"", "trailer": b"/Info 8 0 R",
                               "objects": [b"<< /GTS_PDFXVersion (PDF/X-1a:2003) >>"]}, True),
        ("preview-other-intent", {"intent": b"/OutputIntents [<< /S /GTS_PDFA1 >>]"}, False),
        ("preview-indirect-intent", {"intent": b"/OutputIntents 8 0 R",
                                     "objects": [b"[9 0 R]", b"<< /S 10 0 R >>", b"/GTS_PDFX"]}, True),
        ("preview-page-intent", {"intent": b"", "page_group": PDFX}, True),
        ("preview-page-overrides-catalog", {"page_group": b"/OutputIntents [<< /S /GTS_PDFA1 >>]"}, False),
        ("preview-empty-page-intents", {"page_group": b"/OutputIntents []"}, False),
        ("preview-indirect-page-intents", {"page_group": b"/OutputIntents 8 0 R",
                                          "objects": [b"[<< /S /GTS_PDFA1 >>]"]}, False),
        ("preview-invalid-page-intents", {"page_group": b"/OutputIntents null"}, True),
        ("preview-unresolved-page-intents", {"page_group": b"/OutputIntents 8 0 R",
                                            "objects": [b"8 0 R"]}, True),
        ("preview-page-intent-with-pdfx-info", {"page_group": b"/OutputIntents [<< /S /GTS_PDFA1 >>]",
                                              "trailer": b"/Info 8 0 R",
                                              "objects": [b"<< /GTS_PDFXVersion (PDF/X-1a:2003) >>"]}, True),
        ("preview-malformed-intent", {"intent": b"/OutputIntents [null 9 0 R << /S /Other >>]",
                                      "objects": [b"null", b"9 0 R"],
                                      "trailer": b"/Info 9 0 R"}, False),
        ("preview-malformed-info", {"intent": b"", "trailer": b"/Info 8 0 R",
                                    "objects": [b"<< /GTS_PDFXVersion /NotAString >>"]}, False),
    ]:
        yield case(name, content, compose(compose(WHITE, MAGENTA), CYAN, op=simulate, nonzero=True),
                   (PREVIEW,), **options)
    # A luminosity mask uses its own CMYK group even when the page needs no
    # simulated plates. Blue has luminosity .11; ordinary cyan has .70.
    for name, intent, luminosity in [("preview-cmyk-mask", PDFX, F(11, 100)),
                                     ("preview-cmyk-mask-no-intent", b"", F(7, 10))]:
        gray = int((1 - luminosity) * 255 + F(1, 2))
        yield Case(name, pdf(b"/M gs 0 g " + RECT, intent=intent,
                   extra_resources=b"/ExtGState << /M << /SMask << /S /Luminosity /G 7 0 R >> >> >>",
                   form=BACK + b"/On gs 1 0 0 0 k " + RECT,
                   form_group=b"/Group << /S /Transparency /CS /DeviceCMYK /I true >>"),
                   RENDER, (PREVIEW,), "Preview policy also controls compatible overprinting inside luminosity masks",
                   oracle=b"P6\n8 8\n255\n" + bytes([gray] * 3) * 64, extra={"tolerance": 1})
    # RGB Multiply has no subtractive or spot interaction. OP cannot change its
    # process model: (.5,.5,.5) * (.5,1,1) = (.25,.5,.5), rounded once.
    for flag in (False, True):
        content = b".5 .5 .5 rg " + RECT + b"/B gs .5 1 1 rg " + RECT
        resources = b"/ExtGState << /B << /BM /Multiply " + (b"/OP true" if flag else b"") + b" >> >>"
        yield Case("noop-rgb-multiply-" + ("on" if flag else "off"), pdf(content, extra_resources=resources),
                   RENDER, (FLAGS,), "RGB Multiply is independent of no-op overprint flags",
                   oracle=b"P6\n8 8\n255\n" + bytes([64,128,128]) * 64)
    back = compose(WHITE, MAGENTA)
    for state, op, nonzero in [(b"On", True, True), (b"Zero", True, False), (b"Off", False, False),
                               (b"Fill", True, True), (b"Stroke", False, True)]:
        yield case("fill-" + state.decode().lower(), BACK + b"/" + state + b" gs 1 0 0 0 k " + RECT,
                   compose(back, CYAN, op=op, nonzero=nonzero), (FLAGS, PROCESS))
    for state, op in [(b"On", True), (b"Off", False), (b"Fill", False), (b"Stroke", True)]:
        yield case("stroke-" + state.decode().lower(), BACK + b"/" + state + b" gs 1 0 0 0 K 8 w 0 4 m 8 4 l S",
                   compose(back, CYAN, op=op, nonzero=True), (FLAGS, PROCESS))
    yield case("mode-default-zero", BACK + b"/Bare gs 1 0 0 0 k " + RECT,
               compose(back, CYAN, op=True), (FLAGS, PROCESS),
               extra_resources=b"/ExtGState << /Bare << /op true >> >>")
    yield case("state-restored", BACK + b"/On gs q /Off gs Q 1 0 0 0 k " + RECT,
               compose(back, CYAN, op=True, nonzero=True), (FLAGS, PROCESS))
    yield case("explicit-op-precedes-OP", BACK + b"/Reverse gs 1 0 0 0 k " + RECT,
               compose(back, CYAN), (FLAGS, PROCESS),
               extra_resources=b"/ExtGState << /Reverse << /op false /OP true /OPM 1 >> >>")
    yield case("opm-inherited", BACK + b"/On gs /OnlyOp gs 1 0 0 0 k " + RECT,
               compose(back, CYAN, op=True, nonzero=True), (FLAGS, PROCESS),
               extra_resources=b"/ExtGState << /On << /op true /OPM 1 >> /OnlyOp << /op true >> >>")
    yield case("zero-tint-is-not-quantized", BACK + b"/On gs 1 0.00000000000000000001 0 0 k " + RECT,
               compose(back, {0: F(1), 1: F(1, 10**20), 2: F(0), 3: F(0)}, op=True, nonzero=True))
    yield case("indexed-base-cmyk", BACK + b"/On gs /I cs 0 scn " + RECT,
               compose(back, CYAN, op=True, nonzero=True))
    yield case("partial-opacity", BACK + b"/Half gs 1 0 0 0 k " + RECT,
               compose(back, CYAN, op=True, nonzero=True, alpha=F(2, 5)))
    yield case("screen-implicit-group", b".6 .4 .2 0 k " + RECT + b"/Screen gs .3 0 0 0 k " + RECT,
               compose(compose(WHITE, dict(enumerate(map(F, [".6", ".4", ".2", "0"])))),
                       {0: F(3, 10), 1: F(0), 2: F(0), 3: F(0)}, op=True, nonzero=True, blend="Screen"))
    for state in [b"On", b"Off"]:
        for family, command, tints in [("separation", b"/C cs .6 scn ", {4: F(3, 5)}),
                                       ("devicen", b"/DN cs .6 .2 scn ", {4: F(3, 5), 5: F(1, 5)})]:
            yield case(family + "-" + state.decode().lower(), BACK + b"/" + state + b" gs " + command + RECT,
                       compose(back, tints, op=state == b"On"), (SPOT,))
    spot_back = compose(WHITE, {4: F(3, 5)})
    yield case("process-preserves-spot", b"/C cs .6 scn " + RECT + b"/On gs 0 1 0 0 k " + RECT,
               compose(spot_back, MAGENTA, op=True, nonzero=True), (SPOT,))
    yield case("process-erases-spot", b"/C cs .6 scn " + RECT + b"/On gs /Off gs 0 1 0 0 k " + RECT,
               compose(spot_back, MAGENTA), (SPOT,))
    yield case("spot-replaces-same-name", b"/C cs 1 scn " + RECT + b"/On gs .2 scn " + RECT,
               compose(compose(WHITE, {4: F(1)}), {4: F(1, 5)}, op=True), (SPOT,))
    yield case("devicen-reuses-separation-name", b"/C cs .6 scn " + RECT + b"/On gs /DN cs .2 .4 scn " + RECT,
               compose(spot_back, {4: F(1, 5), 5: F(2, 5)}, op=True), (SPOT,))
    yield case("spot-over-spot", b"/C cs .6 scn " + RECT + b"/On gs /M cs .4 scn " + RECT,
               compose(spot_back, {5: F(2, 5)}, op=True), (SPOT,))
    yield case("spot-difference-uses-normal", b"/C cs .6 scn " + RECT + b"/Difference gs .2 scn " + RECT,
               compose(spot_back, {4: F(1, 5)}, op=True, blend="Difference"), (SPOT,))
    yield case("named-process-separation", BACK + b"/On gs /PC cs .6 scn " + RECT,
               compose(back, {0: F(3, 5)}, op=True), (SPOT, PROCESS))
    yield case("named-process-devicen", BACK + b"/On gs /PN cs .6 .2 scn " + RECT,
               compose(back, {0: F(3, 5), 3: F(1, 5)}, op=True), (SPOT, PROCESS))
    # Zero named tints paint, regardless of OPM; only direct DeviceCMYK uses it.
    yield case("named-zero-tint-erases", b"/C cs .6 scn " + RECT + b"/On gs 0 scn " + RECT,
               compose(spot_back, {4: F(0)}, op=True), (SPOT,))
    yellow = {0: F(0), 1: F(0), 2: F(1), 3: F(0)}
    yellow_back = compose(WHITE, yellow)
    for state, op, alpha in [(b"On", True, F(1)), (b"Half", True, F(2, 5)), (b"Off", False, F(1))]:
        expected = group(yellow_back, [(MAGENTA, {"op": op, "nonzero": True}),
                                       (CYAN, {"op": op, "nonzero": True})], alpha=alpha) if op else compose(yellow_back, CYAN)
        yield case("combined-fill-stroke-" + state.decode().lower(),
                   b"0 0 1 0 k " + RECT + b"/" + state + b" gs 0 1 0 0 k 1 0 0 0 K 16 w 0 0 8 8 re B",
                   expected, (GROUP, PROCESS))
    edge = rgb(compose(back, CYAN, op=True, nonzero=True, alpha=F(1, 2)))
    yield Case("fractional-coverage", pdf(BACK + b"/On gs 1 0 0 0 k .25 0 .5 8 re f"), RENDER,
               (PROCESS,), "Exact half-cell area before composition; coverage quantization has a one-level bound",
               oracle=b"P6\n8 8\n255\n" + (bytes(edge) + bytes(rgb(back)) * 7) * 8, extra={"tolerance": 1})
    for isolated in (False, True):
        attributes = b"/Group << /S /Transparency /I " + (b"true" if isolated else b"false") + b" >>"
        for family, form, tints in [("process", b"/On gs 1 0 0 0 k " + RECT, CYAN),
                                    ("spot", b"/On gs /C cs .6 scn " + RECT, {4: F(3, 5)})]:
            yield case(f"group-{family}-" + ("isolated" if isolated else "inherited"), BACK + b"/On gs /Fm Do",
                       group(back, [(tints, {"op": True, "nonzero": family == "process"})], isolated=isolated),
                       (GROUP, SPOT if family == "spot" else PROCESS), form=form, form_group=attributes)
    yield case("group-invocation-does-not-overprint", BACK + b"/On gs /Fm Do",
               group(back, [(CYAN, {})], isolated=True), (GROUP,),
               form=b"/Off gs 1 0 0 0 k " + RECT,
               form_group=b"/Group << /S /Transparency /CS /DeviceCMYK /I true >>")
    yield case("group-opacity", BACK + b"/Half gs /Fm Do",
               group(back, [({4: F(3, 5)}, {"op": True})], alpha=F(2, 5)), (GROUP, SPOT),
               form=b"/On gs /C cs .6 scn " + RECT,
               form_group=b"/Group << /S /Transparency >>")
    # Sampled CMYK sources paint every process plate even under OPM 1.
    for kind, paint in [("image", b"q 8 0 0 8 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /DeviceCMYK ID " + bytes([255, 0, 0, 0]) + b" EI Q"),
                        ("shading", b"/Sh sh")]:
        yield case(kind + "-ignores-nonzero-mode", BACK + b"/On gs " + paint,
                   compose(back, CYAN, op=True), (PROCESS,),
                   extra_resources=b"/Shading << /Sh << /ShadingType 2 /ColorSpace /DeviceCMYK /Coords [0 0 8 0] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0 0] /C1 [1 0 0 0] /N 1 >> >> >>")
    yield case("stencil-uses-current-nonzero-mode", BACK + b"/On gs 1 0 0 0 k q 8 0 0 8 0 0 cm BI /W 1 /H 1 /BPC 1 /IM true ID " + bytes([0]) + b" EI Q",
               compose(back, CYAN, op=True, nonzero=True), (PROCESS,))
    for state in [b"Zero", b"On"]:
        yield case("spot-mode-" + state.decode().lower(), BACK + b"/" + state + b" gs /C cs .6 scn " + RECT,
                   compose(back, {4: F(3, 5)}, op=True), (SPOT, FLAGS))
    yield case("spot-inline-image", BACK + b"/On gs q 8 0 0 8 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /C ID " + bytes([153]) + b" EI Q",
               compose(back, {4: F(3, 5)}, op=True), (SPOT,))
    yield case("spot-shading", BACK + b"/On gs /Sh sh", compose(back, {4: F(3, 5)}, op=True), (SPOT,),
               extra_resources=b"/Shading << /Sh << /ShadingType 2 /ColorSpace /C /Coords [0 0 8 0] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [.6] /C1 [.6] /N 1 >> >> >>")
    font = (b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 8 8] /FontMatrix [1 0 0 1 0 0] "
            b"/FirstChar 65 /LastChar 65 /Widths [8] /Encoding << /Differences [65 /A] >> "
            b"/CharProcs << /A 9 0 R >> /Resources << >> >>")
    yield case("text-glyph-overprint", BACK + b"/On gs 1 0 0 0 k BT /F 1 Tf (A) Tj ET",
               compose(back, CYAN, op=True, nonzero=True), (PROCESS, FLAGS),
               extra_resources=b"/Font << /F 8 0 R >>", objects=[font, stream(b"8 0 0 0 8 8 d1 " + RECT)])
    # Mask groups always substitute spot alternates (11.7.3), even while
    # rendering an outer spot batch. Luminosity of .6 cyan is .3*.4+.59+.11.
    luminosity = F(41, 50)
    mask = stream(b"/C cs .6 scn " + RECT,
                  b"/Type /XObject /Subtype /Form /BBox [0 0 8 8] /Resources " + RESOURCES
                  + b" /Group << /S /Transparency /CS /DeviceRGB /I true >>")
    states = STATE[:-3] + b"/Mask << /SMask << /S /Luminosity /G 8 0 R >> >> >> "
    yield case("spot-luminosity-mask-uses-alternate", BACK + b"/On gs /Mask gs /C cs .6 scn " + RECT,
               compose(back, {4: F(3, 5)}, op=True, alpha=luminosity), (SPOT, GROUP),
               extra_resources=states, objects=[mask])
    yield case("spot-passes-rgb-group", BACK + b"/On gs /Fm Do",
               group(back, [({4: F(3, 5)}, {"op": True})], isolated=True), (SPOT, GROUP),
               form=b"/On gs /C cs .6 scn " + RECT,
               form_group=b"/Group << /S /Transparency /CS /DeviceRGB /I true >>")
    yield case("named-process-in-rgb-group-uses-alternate", BACK + b"/On gs /Fm Do",
               group(back, [({0: F(3, 5), 1: F(0), 2: F(0), 3: F(0)}, {})], isolated=True),
               (PROCESS, GROUP), form=b"/On gs /PC cs .6 scn " + RECT,
               form_group=b"/Group << /S /Transparency /CS /DeviceRGB /I true >>")
    yield case("all-paints-process-and-spot", b"/C cs .6 scn " + RECT + b"/On gs /M cs 0 scn " + RECT + b"/All cs .1 scn " + RECT,
               compose(spot_back, dict.fromkeys(range(6), F(1, 10)), op=True), (SPOT,))
    # Only input 1 occurs in this image. A template must not evaluate an
    # unrelated zero input, where this calculator would divide by zero.
    yield case("sampled-tint-evaluates-only-pixels",
               b"q 8 0 0 8 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /Probe ID " + bytes([255]) + b" EI Q",
               compose(WHITE, CYAN), (PROCESS,),
               extra_colors=b"/Probe [/Separation /ProbeInk /DeviceCMYK 8 0 R] ",
               objects=[stream(b"{ dup div 0 0 0 }", b"/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1 0 1]")])
    for count in [6, 32, 33]:
        colors = b" ".join(b"/S%d [/Separation /Ink%d /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>]" % (i, i) for i in range(count))
        content = BACK + b"/On gs " + b" ".join(b"/S%d cs .01 scn " % i + RECT for i in range(count))
        if count == 33:
            yield Case("spot-budget", pdf(content, extra_colors=colors), OPEN, (SPOT,),
                       "A page with more than 32 simulated spot plates is a bounded failure",
                       extra={"render_error": "Limit"})
            yield Case("spot-budget-no-intent", pdf(content, extra_colors=colors, intent=b""), RENDER, (PREVIEW,),
                       "Auto without print intent bypasses the simulated plate limit and paints ordinary content",
                       oracle=b"P6\n8 8\n255\n" + bytes([252, 255, 255]) * 64)
        else:
            expected = (int((1 - F(count, 100)) * 255 + F(1, 2)), 0, 255)
            yield Case("spot-batches-" + str(count), pdf(content, extra_colors=colors), RENDER,
                       (SPOT,), "Multiple batches share process shape and opacity without conversions",
                       oracle=b"P6\n8 8\n255\n" + bytes(expected) * 64)
