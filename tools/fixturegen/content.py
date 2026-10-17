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


def optional_pdf(content, ocmd=b"<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AnyOn >>", configuration=b"/BaseState /OFF /ON [5 0 R]", usage_on=b"", usage_off=b""):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << " + configuration + b" >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Properties << /On 5 0 R /Off 6 0 R /Members 7 0 R >> >> /Contents 4 0 R >>",
        stream(content), b"<< /Type /OCG /Name (on) "+usage_on+b">>", b"<< /Type /OCG /Name (off) "+usage_off+b">>", ocmd,
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
    for name, entry, tag in [("alpha",b"/ca .5","transparency.ca"), ("blend",b"/BM /Multiply","transparency.BM"), ("soft-mask",b"/SMask << >>","transparency.SMask"), ("font-state",b"/Font [/F 12]","extgstate.Font")]:
        yield unsupported(name,b"/GS gs",tag,resources=b"<< /ExtGState << /GS << "+entry+b" >> >> >>")
    for family, definition in [
        ("Separation", b"[/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 0] /N 1 >>]"),
        ("DeviceN", b"[/DeviceN [/Cyan] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>]"),
        ("Lab", b"[/Lab << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalRGB", b"[/CalRGB << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalGray", b"[/CalGray << /WhitePoint [.9505 1 1.089] >>]"),
    ]:
        if family in ("CalGray","Lab","Separation","DeviceN"):
            yield render("space-"+family,b"/C cs",resources=b"<< /ColorSpace << /C "+definition+b" >> >>")
        else:
            yield unsupported("space-"+family, b"/C cs", "colorspace."+family, resources=b"<< /ColorSpace << /C "+definition+b" >> >>")
    yield unsupported("pattern", b"/Pattern cs", "pattern.colorspace")
    yield unsupported("knockout", b"", "transparency.knockout", page_extra=b"/Group << /S /Transparency /K true >>")
    yield unsupported("annotation", b"", "annotation.appearance", page_extra=b"/Annots [<< /AP << /N 4 0 R >> >>]")
    for name, command in [("syntax-unknown", b"future"), ("syntax-arity", b"1 q"), ("syntax-type", b"(bad) w"), ("syntax-Q", b"Q"), ("syntax-q-unclosed", b"q"), ("syntax-l", b"1 1 l"), ("syntax-TJ", b"[/Bad] TJ"), ("syntax-pending-clip", b"W"), ("syntax-reference", b"4 0 R"), ("syntax-negative-width", b"-1 w"), ("syntax-zero-dash", b"[0 0] 0 d"), ("transfer", b"/GS gs")]:
        yield Case(name, single_page([command], media_box=b"[0 0 8 8]" if name=="transfer" else b"[0 0 64 64]", resources=b"<< /ExtGState << /GS << /TR [] >> >> >>" if name=="transfer" else b"<< >>"), OPEN, ("content-explicit-operators",), "Valid file with malformed page content", extra={"api":"compile_page", "render_error":"Syntax"})
    yield Case("recursive-form", form_pdf(content=b"/Self Do", form_resources=b"<< /XObject << /Self 5 0 R >> >>"), OPEN, ("content-forms",), "Recursive form must return Cycle", extra={"api":"compile_page", "render_error":"Cycle"})
    yield render("annotation-no-normal", b"", page_extra=b"/Annots [<< /AP << >> >>]")
    yield render("annotation-hidden", b"", page_extra=b"/Annots [<< /F 2 /AP << /N 4 0 R >> >>]")
    yield Case("icc-range", icc_case("unused", 3, b"/Range [0 2 0 1 0 1]", b"/ICC cs").pdf, UNSUPPORTED, ("content-colour",), "Nondefault ICC input ranges require unsupported colour handling", feature="colorspace.ICCRange", extra={"api":"compile_page"})
    yield render("rgb-page-group", b"1 0 0 rg 0 0 4 4 re f", [((0,0,4,4),RED)], page_extra=b"/Group << /S /Transparency /CS /DeviceRGB /I true /K false >>", requirements=("content-opaque-groups",))
    yield Case("rgb-form-group", form_pdf(extra=b"/Group << /S /Transparency /CS /DeviceRGB >>"), RENDER, ("content-opaque-groups",), "Opaque RGB form group paints directly", oracle=opaque_rectangles(8,8,[((2,2,4,4),RED)]))
    # Construct the page group independently rather than rewriting object bytes
    # (which would invalidate xref offsets).
    yield Case("icc-rgb-group", document([
        b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Group << /S /Transparency /CS [/ICCBased 5 0 R] >> /Contents 4 0 R >>",
        stream(b"1 0 0 rg 0 0 8 8 re f"), stream(b"unsupported-profile", b"/N 3 /Alternate /DeviceRGB"),
    ]), RENDER, ("content-opaque-groups",), "RGB ICC alternate is the device blend space", oracle=opaque_rectangles(8,8,[((0,0,8,8),RED)]))
    for space in (b"DeviceGray", b"DeviceCMYK"):
        yield unsupported("group-"+space.decode(), b"", "transparency.group-colorspace", page_extra=b"/Group << /S /Transparency /CS /"+space+b" >>")
    yield render("rgb-device-hints", b"/GS gs 1 0 0 rg 0 0 8 8 re f", [((0,0,8,8),RED)], resources=b"<< /ExtGState << /GS << /SM .5 /TK false /BG << /FunctionType 2 /Domain [0 1] /N 1 >> /BG2 /Default /UCR << /FunctionType 2 /Domain [0 1] /N 1 >> /UCR2 /Default >> >> >>")
    for name,command,resources in [
        ('syntax-BG-default',b'/GS gs',b'<< /ExtGState << /GS << /BG /Default >> >> >>'),
        ('syntax-UCR-default',b'/GS gs',b'<< /ExtGState << /GS << /UCR /Default >> >> >>'),
        ('syntax-Lab-arity',b'/C cs',b'<< /ColorSpace << /C [/Lab] >> >>'),
    ]:
        yield Case(name,single_page([command],media_box=b'[0 0 8 8]',resources=resources),OPEN,BASE,'Invalid parameter shape; Default is permitted only in BG2/UCR2',extra={'render_error':'Syntax'})

    yield render("stroke-adjust-horizontal",b"/GS gs .4 w 1.2 4.2 m 6.8 4.2 l S",[((1,4,6,1),BLACK)],resources=b"<< /ExtGState << /GS << /SA true >> >> >>")
    yield render("stroke-adjust-vertical",b"/GS gs .4 w 4.2 1.2 m 4.2 6.8 l S",[((4,1,1,6),BLACK)],resources=b"<< /ExtGState << /GS << /SA true >> >> >>")
    yield Case("decimal-stroke-half-tie",single_page([b".06 0 0 .06 0 0 cm 6 w 21 52 m 107 52 l S"],media_box=b"[0 0 8 8]"),RENDER,("content-fractional",),"Rational stroke edges x=1.26..6.42, y=2.94..3.30; area .30 rounds alpha 76.5 to 77",oracle=fractional_oracle(F(126,100),F(294,100),F(642,100),F(330,100)),extra={"tolerance":1})
    yield from cie_cases()
    yield from function_cases()
    yield from usage_cases()


def cie_rgb(family, values, white=(.96422, 1, .82521), gamma=1, ranges=(-100,100,-100,100)):
    """Independent XYZ -> Bradford-adapted sRGB oracle; inverse matrices are solved.

    sRGB primaries and D65 determine the XYZ matrix. Gaussian elimination derives
    inverses instead of reusing the product's rounded inverse constants.
    """
    def inverse(matrix):
        a=[list(row)+[int(i==j) for j in range(3)] for i,row in enumerate(matrix)]
        for i in range(3):
            pivot=max(range(i,3),key=lambda j:abs(a[j][i]))
            a[i],a[pivot]=a[pivot],a[i]
            divisor=a[i][i];a[i]=[v/divisor for v in a[i]]
            for j in range(3):
                if j!=i:
                    multiplier=a[j][i];a[j]=[x-multiplier*y for x,y in zip(a[j],a[i])]
        return [row[3:] for row in a]
    def apply(matrix, vector):
        return [sum(a*b for a,b in zip(row,vector)) for row in matrix]
    if family=='CalGray':
        xyz=[w*min(1,max(0,values[0]))**gamma for w in white]
    else:
        lightness=min(100,max(0,values[0]));a=min(ranges[1],max(ranges[0],values[1]));b=min(ranges[3],max(ranges[2],values[2]))
        m=(lightness+16)/116
        def f(t): return t**3 if t>=6/29 else 3*(6/29)**2*(t-4/29)
        xyz=[white[0]*f(m+a/500),f(m),white[2]*f(m-b/200)]
    bradford=[[.8951,.2664,-.1614],[-.7502,1.7135,.0367],[.0389,-.0685,1.0296]]
    d65=[.95047,1,1.08883]
    source=apply(bradford,white);target=apply(bradford,d65)
    xyz=apply(inverse(bradford),[v*t/s for v,t,s in zip(apply(bradford,xyz),target,source)])
    primaries=[(.64,.33),(.30,.60),(.15,.06)]
    columns=[[x/y,1,(1-x-y)/y] for x,y in primaries]
    basis=[list(v) for v in zip(*columns)]
    scale=apply(inverse(basis),d65)
    rgb_matrix=[[entry*scale[i] for i,entry in enumerate(row)] for row in basis]
    linear=apply(inverse(rgb_matrix),xyz)
    def encode(v):
        v=min(1,max(0,v));v=12.92*v if v<=.0031308 else 1.055*v**(1/2.4)-.055
        return int(v*255+.5)
    return tuple(map(encode,linear))


def cie_cases():
    for family, values, params in [
        ('CalGray', [(v,) for v in [0,.003,.18,.5,.75,1,2,-1]], b'/Gamma 1'),
        ('CalGray', [(v,) for v in [0,.01,.18,.5,.75,1,2,-1]], b'/Gamma 2.2'),
        ('Lab', [(0,0,0),(8,0,0),(50,0,0),(75,0,0),(100,0,0),(50,40,-20),(50,-70,30),(50,200,-200)], b''),
    ]:
        name='cie-'+family+('-gamma' if params==b'/Gamma 2.2' else '')
        command=b'/C cs '+b' '.join((' '.join(map(str,v))+f' sc {x} 0 1 8 re f').encode() for x,v in enumerate(values))
        gamma=2.2 if params==b'/Gamma 2.2' else 1
        colours=[cie_rgb(family,v,gamma=gamma) for v in values]
        yield Case(name,single_page([command],media_box=b'[0 0 8 8]',resources=b'<< /ColorSpace << /C [/'+family.encode()+b' << /WhitePoint [.96422 1 .82521] '+params+b' >>] >> >>'),RENDER,('content-cie',), 'Defining CIE formulas with Bradford adaptation to D65 and sRGB',oracle=opaque_rectangles(8,8,[((x,0,1,8),c) for x,c in enumerate(colours)]),extra={'tolerance':1})
    colour=cie_rgb('Lab',(128/255*100,-100+128/255*200,-100+128/255*200))
    yield render('cie-indexed-Lab',b'/C cs 0 sc 0 0 8 8 re f',[((0,0,8,8),colour)],resources=b'<< /ColorSpace << /C [/Indexed [/Lab << /WhitePoint [.96422 1 .82521] >>] 0 <808080>] >> >>',requirements=('content-cie',))
    yield unsupported('cie-blackpoint',b'/C cs','colorspace.BlackPoint',resources=b'<< /ColorSpace << /C [/CalGray << /WhitePoint [.96422 1 .82521] /BlackPoint [.1 .1 .1] >>] >> >>')


def function_cases():
    ts=[0,.125,.25,.5,.625,.75,1,2]
    def function_pdf(space,function,command):
        return document([
            b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ColorSpace << /C '+space+b' >> >> /Contents 4 0 R >>',stream(command),function,
        ])
    def strips(name,space,function,values,colours):
        command=b'/C cs '+b' '.join((' '.join(map(str,v))+f' sc {i} 0 1 8 re f').encode() for i,v in enumerate(values))
        pdf=function_pdf(space,function,command)
        return Case(name,pdf,RENDER,('content-functions',),'Independent numeric tint-transform oracle',oracle=opaque_rectangles(8,8,[((i,0,1,8),c) for i,c in enumerate(colours)]),extra={'tolerance':1})
    def byte(x): return int(min(1,max(0,x))*255+.5)
    for power in [1,2]:
        colours=[(255,byte(1-min(1,t)**power),byte(1-min(1,t)**power)) for t in ts]
        yield strips('function-exp-'+str(power),b'[/Separation /Spot /DeviceRGB 5 0 R]',b'<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0 0] /N '+str(power).encode()+b' >>',[(t,) for t in ts],colours)
    function=b'<< /FunctionType 3 /Domain [0 1] /Range [0 1 0 1 0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0 0] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [0 0 1] /C1 [0 0 0] /N 1 >>] /Bounds [.5] /Encode [0 1 0 1] >>'
    colours=[(255,byte(1-2*t),byte(1-2*t)) if t<.5 else (0,0,byte(2-2*min(1,t))) for t in ts]
    yield strips('function-stitch',b'[/Separation /Spot /DeviceRGB 5 0 R]',function,[(t,) for t in ts],colours)
    values=[(0,0),(1,0),(0,1),(1,1),(.25,.75),(.5,.5),(-1,2),(2,-1)]
    samples=bytes([255,255,255,255,0,0,0,0,255,0,0,0])
    colours=[]
    for x,y in values:
        x=min(1,max(0,x));y=min(1,max(0,y))
        colours.append((byte(1-y),byte((1-x)*(1-y)),byte(1-x)))
    yield strips('function-sampled',b'[/DeviceN [/A /B] /DeviceRGB 5 0 R]',stream(samples,b'/FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8'),values,colours)
    program=b'{ add dup 1 exch sub exch 0 }'
    colours=[(byte(1-min(1,max(0,x))-min(1,max(0,y))),byte(min(1,max(0,x))+min(1,max(0,y))),0) for x,y in values]
    yield strips('function-calculator',b'[/DeviceN [/A /B] /DeviceRGB 5 0 R]',stream(program,b'/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]'),values,colours)
    for bits in [1,2,4,8,12,16,24,32]:
        bitstring=f'{(1<<bits)-1:0{bits}b}'+('0'*bits)
        bitstring+='0'*(-len(bitstring)%8)
        data=int(bitstring,2).to_bytes(len(bitstring)//8,'big')
        yield strips('function-sampled-'+str(bits),b'[/Separation /Spot /DeviceGray 5 0 R]',stream(data,b'/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample '+str(bits).encode()),[(t,) for t in ts],[(byte(1-min(1,t)),)*3 for t in ts])
    gray=b'[/Separation /Spot /DeviceGray 5 0 R]'
    sampled=b'/FunctionType 0 /Domain [0 1] /Range [0 1] /BitsPerSample 8 '
    yield strips('function-order3-small',gray,stream(b'\xff\0',sampled+b'/Size [2] /Order 3'),[(t,) for t in ts],[(byte(1-min(1,t)),)*3 for t in ts])
    for name,function,command,error in [
        ('function-truncated',stream(b'\xff',sampled+b'/Size [2]'),b'/C cs','Syntax'),
        ('function-cycle',b'<< /FunctionType 3 /Domain [0 1] /Functions [5 0 R] /Bounds [] /Encode [0 1] >>',b'/C cs','Cycle'),
        ('function-dimension',b'<< /FunctionType 2 /Domain [0 1 0 1] /N 1 >>',b'/C cs','Syntax'),
        ('function-undefined',b'<< /FunctionType 2 /Domain [0 1] /N -1 >>',b'/C cs 0 sc 0 0 8 8 re f','Syntax'),
        ('function-host-action',stream(b'{ pop (secret) file }',b'/FunctionType 4 /Domain [0 1] /Range [0 1]'),b'/C cs','Syntax'),
    ]:
        yield Case(name,function_pdf(gray,function,command),OPEN,('content-functions',),'Independent invalid function contract',extra={'render_error':error})
    yield Case('function-order3',function_pdf(gray,stream(b'\xff\xaa\x55\0',sampled+b'/Size [4] /Order 3'),b'/C cs'),UNSUPPORTED,('content-functions',),'Cubic interpolation is explicitly unsupported',feature='function.cubic-samples')
    for kind,transform in [('type2',b'<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>'),('type4',b'5 0 R')]:
        if kind=='type2':
            yield render('transfer-'+kind,b'/GS gs .25 g 0 0 8 8 re f',[((0,0,8,8),(191,191,191))],resources=b'<< /ExtGState << /GS << /TR '+transform+b' >> >> >>',requirements=('content-functions',))
        else:
            pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ExtGState << /GS << /TR 5 0 R >> >> >> /Contents 4 0 R >>',stream(b'/GS gs .25 g 0 0 8 8 re f'),stream(b'{1 exch sub}',b'/FunctionType 4 /Domain [0 1] /Range [0 1]')])
            yield Case('transfer-'+kind,pdf,RENDER,('content-functions',),'Calculator transfer after device conversion, before quantization',oracle=opaque_rectangles(8,8,[((0,0,8,8),(191,191,191))]))
    yield render('transfer-precedence',b'/GS gs 0 0 8 8 re f',[((0,0,8,8),BLACK)],resources=b'<< /ExtGState << /GS << /TR [] /TR2 /Identity >> >> >>',requirements=('content-functions',))
    yield render('transfer-array',b'/GS gs .25 .5 .75 rg 0 0 8 8 re f',[((0,0,8,8),(191,128,191))],resources=b'<< /ExtGState << /GS << /TR [<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> /Identity /Identity /Identity] >> >> >>',requirements=('content-functions',))


def usage_cases():
    command=b'/OC /On BDC 1 0 0 rg 0 0 8 8 re f EMC /OC /Off BDC 0 0 1 rg 0 0 4 4 re f EMC'
    usage_on=b'/Usage << /View << /ViewState /OFF >> >> '
    usage_off=b'/Usage << /View << /ViewState /ON >> >> '
    configuration=b'/BaseState /OFF /ON [5 0 R] /AS [<< /Event /View /OCGs [5 0 R 6 0 R] /Category [/View] >>]'
    yield Case('usage-View',optional_pdf(command,configuration=configuration,usage_on=usage_on,usage_off=usage_off),RENDER,('content-usage',),'View usage overrides explicit initial ON/OFF',oracle=opaque_rectangles(8,8,[((0,0,4,4),BLUE)]))
    for name,configuration,usage,visible in [
        ('usage-Print-ignored',b'/BaseState /OFF /ON [5 0 R] /AS [<< /Event /Print /OCGs [5 0 R] /Category [/View] >>]',usage_on,True),
        ('usage-no-OCGs',b'/BaseState /OFF /ON [5 0 R] /AS [<< /Event /View /Category [/View] >>]',usage_on,True),
        ('usage-Zoom-on',b'/BaseState /OFF /AS [<< /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /Zoom << /min 1 /max 2 >> >> ',True),
        ('usage-Zoom-off',b'/BaseState /ON /AS [<< /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /Zoom << /max 1 >> >> ',False),
        ('usage-conjunction',b'/BaseState /ON /AS [<< /Event /View /OCGs [5 0 R] /Category [/View] >> << /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /View << /ViewState /ON >> /Zoom << /min 2 >> >> ',False),
    ]:
        yield Case(name,optional_pdf(b'/OC /On BDC 0 0 8 8 re f EMC',configuration=configuration,usage_on=usage),RENDER,('content-usage',),'Initial View usage recommendation with AND aggregation',oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)] if visible else []))
