"""Vector display-list fixtures, derived independently from geometry and PDF syntax.

These cases exercise pagura_render::compile_page and DisplayList::render.
Colours and opaque pixel geometry are exact; fractional rectangles use products
of pixel overlaps and rounded source-over composition, tolerance one level.
"""
from fractions import Fraction as F
from .model import Case, RENDER, UNSUPPORTED, OPEN
from .pdf import single_page, document, stream, opaque_rectangles

AREA = "content"
BASE = ("content-vector-pipeline",)
RED, BLUE, BLACK = (255, 0, 0), (0, 0, 255), (0, 0, 0)


def render(name, contents, geometry=(), width=8, height=8, requirements=(), **options):
    pdf = single_page(contents if isinstance(contents, list) else [contents],
                      media_box=b"[0 0 8 8]", **options)
    return Case(name, pdf, RENDER, BASE + requirements, name,
                oracle=opaque_rectangles(width, height, geometry),
                extra={"api": "compile_page"})


def unsupported(name, content, feature, resources=b"<< >>", page_extra=b""):
    return Case(name, single_page([content], media_box=b"[0 0 8 8]", resources=resources, page_extra=page_extra),
                UNSUPPORTED, ("content-explicit-operators",), name, feature=feature,
                extra={"api": "compile_page"})


def fractional_oracle(x0, y0, x1, y1):
    pixels = bytearray()
    for y in range(8):
        for x in range(8):
            area = max(F(0), min(F(x+1), x1)-max(F(x), x0)) * max(F(0), min(F(8-y), y1)-max(F(7-y), y0))
            alpha = int(area * 255 + F(1, 2))
            pixels += bytes([255-alpha] * 3)
    return b"P6\n8 8\n255\n" + pixels


def form_pdf(content=b"1 0 0 rg -2 -2 8 8 re f", extra=b"", form_resources=b"<< >>", page_content=b"/Fm Do", page_extra=b""):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /Fm 5 0 R >> >> /Contents 4 0 R " + page_extra + b">>",
        stream(page_content),
        stream(content, b"/Type /XObject /Subtype /Form /BBox [0 0 4 4] /Matrix [1 0 0 1 2 2] /Resources " + form_resources + b" " + extra),
    ])


def icc_case(name, count, alternate, command, key=b"ICC"):
    # An unsupported ICC profile is deliberately opaque to the renderer. The
    # required PDF stream shape and N/Alternate drive the permitted fallback.
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ColorSpace << /" + key + b" [/ICCBased 5 0 R] >> >> /Contents 4 0 R >>",
        stream(command), stream(b"unsupported-profile", b"/N %d " % count + alternate),
    ])
    return Case(name, pdf, RENDER, ("content-colour",), "ICC stream Alternate/N fallback; profile bytes are not interpreted", oracle=opaque_rectangles(8,8,[((0,0,8,8),RED)]), extra={"api":"compile_page"})


def optional_pdf(content, ocmd=b"<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AnyOn >>", configuration=b"/BaseState /OFF /ON [5 0 R]"):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << " + configuration + b" >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Properties << /On 5 0 R /Off 6 0 R /Members 7 0 R >> >> /Contents 4 0 R >>",
        stream(content), b"<< /Type /OCG /Name (on) >>", b"<< /Type /OCG /Name (off) >>", ocmd,
    ])


def cases():
    yield render("compound-path", b"0 0 m 8 0 l 8 8 l 0 8 l h 2 2 4 4 re f*", [((0,0,8,8),BLACK), ((2,2,4,4),(255,255,255))])
    yield render("nonzero", b"0 0 8 8 re 2 2 4 4 re f", [((0,0,8,8),BLACK)])
    yield render("ctm-during-path", b"0 0 m 1 0 0 1 4 0 cm 0 0 l 0 4 l -4 4 l h f", [((0,0,4,4),BLACK)])
    yield render("clip-after-paint", b"0 0 4 4 re W f 1 0 0 rg 0 0 8 8 re f", [((0,0,4,4), RED)])
    yield render("clip-q-Q", b"q 0 0 4 4 re W n 0 0 8 8 re f Q 1 0 0 rg 4 4 4 4 re f", [((0,0,4,4),BLACK), ((4,4,4,4),RED)])
    yield render("clip-is-not-restored-path", b"q 0 0 4 4 re Q f", [((0,0,4,4),BLACK)])
    yield render("stroke-square", b"2 w 2 J 2 4 m 6 4 l S", [((1,3,6,2),BLACK)])
    yield render("stroke-dash", b"2 w [2 2] 0 d 0 4 m 8 4 l S", [((0,3,2,2),BLACK), ((4,3,2,2),BLACK)])
    yield render("stroke-nonuniform", b"2 0 0 1 0 0 cm 2 w 2 J 1 4 m 3 4 l S", [((0,3,8,2),BLACK)])
    yield render("fill-stroke-close", b"1 0 0 rg 0 G 2 w 2 2 m 6 2 l 6 6 l 2 6 l b", [((1,1,6,6),BLACK), ((3,3,2,2),RED)])
    yield render("cmyk", b"0 1 1 0 k 0 0 4 8 re f 1 1 0 0 K 2 w 5 1 m 5 7 l S", [((0,0,4,8),RED), ((4,1,2,6),BLUE)], requirements=("content-colour",))
    yield render("indexed", b"/Palette cs 1 scn 0 0 8 8 re f", [((0,0,8,8),RED)], resources=b"<< /ColorSpace << /Palette [/Indexed /DeviceRGB 1 <000000ff0000>] >> >>", requirements=("content-colour",))
    yield icc_case("icc-alternate", 3, b"/Alternate /DeviceRGB", b"/ICC cs 1 0 0 sc 0 0 8 8 re f")
    yield icc_case("icc-default", 4, b"", b"/ICC cs 0 1 1 0 sc 0 0 8 8 re f")
    yield icc_case("default-space", 3, b"", b"1 0 0 rg 0 0 8 8 re f", key=b"DefaultRGB")
    yield render("compound-operands-split", [b"/Span << /Text (arbitrary) /Array [true false", b"null /Name <ff>] >> BDC 0 0 4 4 re f EMC"], [((0,0,4,4),BLACK)], requirements=("content-lexical-sequence",))
    yield render("compatibility", b"BX (opaque operands) /Name 2 future-op EX 0 0 4 4 re f", [((0,0,4,4),BLACK)], requirements=("content-explicit-operators",))
    yield render("extgstate", b"/GS gs 1 4 m 7 4 l S", [((0,3,8,2),BLACK)], resources=b"<< /ExtGState << /GS << /LW 2 /LC 2 /LJ 1 /ML 4 /D [[] 0] /RI /Perceptual /FL 0 /SA false /CA 1 /ca 1 /BM /Normal /SMask /None /OP true /op true /OPM 1 /TR /Identity >> >> >>")
    yield render("opaque-page-group", b"1 0 0 rg 0 0 4 4 re f", [((0,0,4,4),RED)], page_extra=b"/Group << /S /Transparency /I true /K false >>", requirements=("content-opaque-groups",))
    # Clockwise 90 maps (x,y) to top-left device (y,x); 270 maps to (8-y,8-x).
    for rotation, geometry in [(90, ((6,4,2,4),RED)), (180, ((4,0,4,2),RED)), (270, ((0,0,2,4),RED))]:
        yield render(f"rotate-{rotation}", b"1 0 0 rg 0 6 4 2 re f", [geometry], page_extra=f"/Rotate {rotation}".encode(), requirements=("content-page-transform",))
    yield render("crop", b"1 0 0 rg 2 2 4 4 re f", [((0,0,4,4),RED)], width=4, height=4, page_extra=b"/CropBox [2 2 6 6]", requirements=("content-page-transform",))
    yield render("user-unit", b"1 0 0 rg 0 0 2 2 re f", [((0,0,4,4),RED)], width=16, height=16, page_extra=b"/UserUnit 2", requirements=("content-page-transform",))
    yield Case("fractional", single_page([b".25 .5 5 6 re f"], media_box=b"[0 0 8 8]"), RENDER, ("content-fractional",), "Exact products of fractional pixel overlaps", oracle=fractional_oracle(F(1,4),F(1,2),F(21,4),F(13,2)), extra={"api":"compile_page", "tolerance":1})
    for name, command, box in [
        ("fractional-clip", b".25 .5 5 6 re W n .25 .5 5 6 re f", (F(1,4),F(1,2),F(21,4),F(13,2))),
        ("fractional-clip-intersection", b".5 0 .5 1 re W n .25 0 .5 1 re f", (F(1,2),F(0),F(3,4),F(1))),
    ]:
        yield Case(name, single_page([command], media_box=b"[0 0 8 8]"), RENDER, ("content-fractional",), "Clip geometry is intersected before area integration", oracle=fractional_oracle(*box), extra={"api":"compile_page", "tolerance":1})
    for name, pdf in [("form", form_pdf()), ("opaque-form-group", form_pdf(extra=b"/Group << /S /Transparency /I true >>"))]:
        yield Case(name, pdf, RENDER, ("content-forms", "content-opaque-groups"), name, oracle=opaque_rectangles(8,8,[((2,2,4,4),RED)]), extra={"api":"compile_page"})
    yield Case("form-resources", form_pdf(content=b"/C cs 1 sc 0 0 4 4 re f", form_resources=b"<< /ColorSpace << /C /DeviceGray >> >>"), RENDER, ("content-forms",), "Form resources are local", oracle=opaque_rectangles(8,8,[]), extra={"api":"compile_page"})
    content = b"/OC /Off BDC 1 0 0 rg 0 0 8 8 re f EMC /OC /On BDC 0 0 4 4 re f EMC"
    yield Case("optional-default", optional_pdf(content), RENDER, ("content-optional",), "Hidden painting still changes graphics state", oracle=opaque_rectangles(8,8,[((0,0,4,4),RED)]), extra={"api":"compile_page"})
    for policy, visible in [(b"AnyOn", True), (b"AllOn", False), (b"AnyOff", True), (b"AllOff", False)]:
        yield Case("optional-"+policy.decode(), optional_pdf(b"/OC /Members BDC 0 0 8 8 re f EMC", b"<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /"+policy+b" >>"), RENDER, ("content-optional",), "Membership policy", oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)] if visible else []), extra={"api":"compile_page"})
    yield Case("optional-expression", optional_pdf(b"/OC /Members BDC 0 0 8 8 re f EMC", b"<< /Type /OCMD /VE [/And 5 0 R [/Not 6 0 R]] >>"), RENDER, ("content-optional",), "Boolean visibility expression", oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)]), extra={"api":"compile_page"})
    for name, command, tag in [("text", b"(text) Tj", "operator.Tj"), ("inline-image", b"BI", "image.inline"), ("shading", b"/S sh", "shading.paint"), ("text-state", b"/F 12 Tf", "operator.Tf"), ("type3", b"1 2 d0", "operator.d0")]:
        yield unsupported(name, command, tag)
    for name, entry, tag in [("alpha",b"/ca .5","transparency.ca"), ("blend",b"/BM /Multiply","transparency.BM"), ("soft-mask",b"/SMask << >>","transparency.SMask"), ("transfer",b"/TR []","extgstate.TR"), ("font-state",b"/Font [/F 12]","extgstate.Font")]:
        yield unsupported(name,b"/GS gs",tag,resources=b"<< /ExtGState << /GS << "+entry+b" >> >> >>")
    for family, definition in [
        ("Separation", b"[/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 0] /N 1 >>]"),
        ("DeviceN", b"[/DeviceN [/Cyan] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>]"),
        ("Lab", b"[/Lab << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalRGB", b"[/CalRGB << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalGray", b"[/CalGray << /WhitePoint [.9505 1 1.089] >>]"),
    ]:
        yield unsupported("space-"+family, b"/C cs", "colorspace."+family, resources=b"<< /ColorSpace << /C "+definition+b" >> >>")
    yield unsupported("pattern", b"/Pattern cs", "pattern.colorspace")
    yield unsupported("knockout", b"", "transparency.knockout", page_extra=b"/Group << /S /Transparency /K true >>")
    yield unsupported("annotation", b"", "annotation.appearance", page_extra=b"/Annots [<< /AP << /N 4 0 R >> >>]")
    for name, command in [("syntax-unknown", b"future"), ("syntax-arity", b"1 q"), ("syntax-type", b"(bad) w"), ("syntax-Q", b"Q"), ("syntax-q-unclosed", b"q"), ("syntax-l", b"1 1 l"), ("syntax-TJ", b"[/Bad] TJ"), ("syntax-pending-clip", b"W"), ("syntax-reference", b"4 0 R"), ("syntax-negative-width", b"-1 w"), ("syntax-zero-dash", b"[0 0] 0 d")]:
        yield Case(name, single_page([command]), OPEN, ("content-explicit-operators",), "Valid file with malformed page content", extra={"api":"compile_page", "render_error":"Syntax"})
    yield Case("recursive-form", form_pdf(content=b"/Self Do", form_resources=b"<< /XObject << /Self 5 0 R >> >>"), OPEN, ("content-forms",), "Recursive form must return Cycle", extra={"api":"compile_page", "render_error":"Cycle"})
    yield render("annotation-no-normal", b"", page_extra=b"/Annots [<< /AP << >> >>]")
    yield render("annotation-hidden", b"", page_extra=b"/Annots [<< /F 2 /AP << /N 4 0 R >> >>]")
    yield Case("icc-range", icc_case("unused", 3, b"/Range [0 2 0 1 0 1]", b"/ICC cs").pdf, UNSUPPORTED, ("content-colour",), "Nondefault ICC input ranges require unsupported colour handling", feature="colorspace.ICCRange", extra={"api":"compile_page"})
