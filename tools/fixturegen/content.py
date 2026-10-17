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
    # A synthetic later-version header exercises PDF's alternate rule.
    # ICC tag definitions await primary-text review; these are not vendor bytes.
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ColorSpace << /" + key + b" [/ICCBased 5 0 R] >> >> /Contents 4 0 R >>",
        stream(command), stream(icc_profile(count, version=5), b"/N %d " % count + alternate),
    ])
    return Case(name, pdf, RENDER, ("content-colour",), "ICC stream Alternate/N fallback for a synthetic later-version profile; primary ICC-text review pending", oracle=opaque_rectangles(8,8,[((0,0,8,8),RED)]), extra={"api":"compile_page"})


def optional_pdf(content, ocmd=b"<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AnyOn >>", configuration=b"/BaseState /ON /OFF [6 0 R]", usage_on=b"", usage_off=b""):
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
    yield icc_case("icc-alternate", 3, b"/Alternate /DeviceRGB", b"/ICC cs 1 0 0 scn 0 0 8 8 re f")
    yield icc_case("icc-default", 4, b"", b"/ICC cs 0 1 1 0 scn 0 0 8 8 re f")
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
    # Malformed text and image contexts are syntax errors: showing text outside
    # BT (9.4.1), an unterminated inline image (8.9.7), an undefined font
    # resource (7.8.3), glyph metrics outside a Type 3 glyph (9.6.4), and an
    # ExtGState Font entry that is not [font-reference size] (Table 57).
    for name, command, resources in [("text", b"(text) Tj", b"<< >>"), ("inline-image", b"BI", b"<< >>"),
                                     ("text-state", b"/F 12 Tf", b"<< >>"), ("type3", b"1 2 d0", b"<< >>"),
                                     ("font-state", b"/GS gs", b"<< /ExtGState << /GS << /Font [/F 12] >> >> >>")]:
        yield Case(name, single_page([command], resources=resources), OPEN, ("content-explicit-operators",),
                   "Malformed text or image context", extra={"api": "compile_page", "render_error": "Syntax"})
    for family, definition in [
        ("Separation", b"[/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 0] /N 1 >>]"),
        ("DeviceN", b"[/DeviceN [/Cyan] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>]"),
        ("Lab", b"[/Lab << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalRGB", b"[/CalRGB << /WhitePoint [.9505 1 1.089] >>]"),
        ("CalGray", b"[/CalGray << /WhitePoint [.9505 1 1.089] >>]"),
    ]:
        if family in ("CalGray","CalRGB","Lab","Separation","DeviceN"):
            yield render("space-"+family,b"/C cs",resources=b"<< /ColorSpace << /C "+definition+b" >> >>")
        else:
            yield unsupported("space-"+family, b"/C cs", "colorspace."+family, resources=b"<< /ColorSpace << /C "+definition+b" >> >>")

    yield render("knockout", b"", page_extra=b"/Group << /S /Transparency /K true >>")
    yield from annotation_cases()
    yield from transparency_cases()
    yield from shading_cases()
    yield from mask_geometry_cases()
    yield from dash_space_cases()
    yield from standard_semantics_cases()
    yield from icc_cases()
    yield from invalid_compositing_cases()
    for name, command in [("syntax-unknown", b"future"), ("syntax-arity", b"1 q"), ("syntax-type", b"(bad) w"), ("syntax-Q", b"Q"), ("syntax-q-unclosed", b"q"), ("syntax-l", b"1 1 l"), ("syntax-TJ", b"[/Bad] TJ"), ("syntax-pending-clip", b"W"), ("syntax-reference", b"4 0 R"), ("syntax-zero-dash", b"[0 0] 0 d"), ("transfer", b"/GS gs")]:
        yield Case(name, single_page([command], media_box=b"[0 0 8 8]" if name=="transfer" else b"[0 0 64 64]", resources=b"<< /ExtGState << /GS << /TR [] >> >> >>" if name=="transfer" else b"<< >>"), OPEN, ("content-explicit-operators",), "Valid file with malformed page content", extra={"api":"compile_page", "render_error":"Syntax"})
    yield Case("recursive-form", form_pdf(content=b"/Self Do", form_resources=b"<< /XObject << /Self 5 0 R >> >>"), OPEN, ("content-forms",), "Recursive form must return Cycle", extra={"api":"compile_page", "render_error":"Cycle"})
    yield render("annotation-no-normal", b"", page_extra=b"/Annots [<< /Subtype /Widget /Rect [0 0 0 0] >>]")
    yield render("annotation-hidden", b"", page_extra=b"/Annots [<< /F 2 /AP << /N 4 0 R >> >>]")
    yield Case("icc-range", icc_case("unused", 3, b"/Range [0 2 0 1 0 1]", b"/ICC cs").pdf, UNSUPPORTED, ("content-colour",), "Nondefault ICC input ranges require unsupported colour handling", feature="colorspace.ICCRange", extra={"api":"compile_page"})
    yield render("rgb-page-group", b"1 0 0 rg 0 0 4 4 re f", [((0,0,4,4),RED)], page_extra=b"/Group << /S /Transparency /CS /DeviceRGB /I true /K false >>", requirements=("content-opaque-groups",))
    yield Case("rgb-form-group", form_pdf(extra=b"/Group << /S /Transparency /CS /DeviceRGB >>"), RENDER, ("content-opaque-groups",), "Opaque RGB form group paints directly", oracle=opaque_rectangles(8,8,[((2,2,4,4),RED)]))
    # Construct the page group independently rather than rewriting object bytes
    # (which would invalidate xref offsets).
    yield Case("icc-rgb-group", document([
        b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Group << /S /Transparency /CS [/ICCBased 5 0 R] >> /Contents 4 0 R >>",
        stream(b"1 0 0 rg 0 0 8 8 re f"), stream(icc_profile(3, version=5), b"/N 3 /Alternate /DeviceRGB"),
    ]), RENDER, ("content-opaque-groups",), "RGB ICC alternate is the device blend space", oracle=opaque_rectangles(8,8,[((0,0,8,8),RED)]))
    for space in (b"DeviceGray", b"DeviceCMYK"):
        yield render("group-"+space.decode(), b"", page_extra=b"/Group << /S /Transparency /CS /"+space+b" >>")
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


def cie_rgb(family, values, white=(.96422, 1, .82521), gamma=1, ranges=(-100,100,-100,100), matrix=(1,0,0,0,1,0,0,0,1)):
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
    if family=='CalRGB':
        gammas=(gamma,)*3 if isinstance(gamma,(float,int)) else gamma
        decoded=[min(1,max(0,v))**g for v,g in zip(values,gammas)]
        xyz=[sum(matrix[row+3*col]*decoded[col] for col in range(3)) for row in range(3)]
    elif family=='CalGray':
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
        command=b'/C cs '+b' '.join((' '.join(map(str,v))+f' scn {i} 0 1 8 re f').encode() for i,v in enumerate(values))
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
        ('function-undefined',b'<< /FunctionType 2 /Domain [0 1] /N -1 >>',b'/C cs 0 scn 0 0 8 8 re f','Syntax'),
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
    configuration=b'/BaseState /ON /OFF [6 0 R] /AS [<< /Event /View /OCGs [5 0 R 6 0 R] /Category [/View] >>]'
    yield Case('usage-View',optional_pdf(command,configuration=configuration,usage_on=usage_on,usage_off=usage_off),RENDER,('content-usage',),'View usage overrides explicit initial ON/OFF',oracle=opaque_rectangles(8,8,[((0,0,4,4),BLUE)]))
    for name,configuration,usage,visible in [
        ('usage-Print-ignored',b'/BaseState /ON /OFF [6 0 R] /AS [<< /Event /Print /OCGs [5 0 R] /Category [/View] >>]',usage_on,True),
        ('usage-no-OCGs',b'/BaseState /ON /OFF [6 0 R] /AS [<< /Event /View /Category [/View] >>]',usage_on,True),
        ('usage-Zoom-on',b'/BaseState /ON /OFF [5 0 R 6 0 R] /AS [<< /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /Zoom << /min 1 /max 2 >> >> ',True),
        ('usage-Zoom-off',b'/BaseState /ON /AS [<< /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /Zoom << /max 1 >> >> ',False),
        ('usage-conjunction',b'/BaseState /ON /AS [<< /Event /View /OCGs [5 0 R] /Category [/View] >> << /Event /View /OCGs [5 0 R] /Category [/Zoom] >>]',b'/Usage << /View << /ViewState /ON >> /Zoom << /min 2 >> >> ',False),
    ]:
        yield Case(name,optional_pdf(b'/OC /On BDC 0 0 8 8 re f EMC',configuration=configuration,usage_on=usage),RENDER,('content-usage',),'Initial View usage recommendation with AND aggregation',oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)] if visible else []))


def annotation_cases():
    def pdf(extra=b'',form=b'1 0 0 rg 0 0 2 4 re f',box=b'[0 0 2 4]',matrix=b'[1 0 0 1 0 0]',page=b'0 0 1 rg 0 0 8 8 re f',normal=b'5 0 R',rect=b'[2 1 6 7]'):
        return document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << >> /Contents 4 0 R /Annots [<< /Subtype /Stamp /Rect '+rect+b' /AP << /N '+normal+b' >> '+extra+b' >>] >>',stream(page),stream(form,b'/Type /XObject /Subtype /Form /BBox '+box+b' /Matrix '+matrix+b' /Resources << >>')])
    for name,options,visible in [
        ('annotation',{},True),
        ('annotation-rotated',{'matrix':b'[0 1 -1 0 10 -3]'},True),
        ('annotation-reversed-rect',{'rect':b'[6 7 2 1]'},True),
        ('annotation-print-flag',{'extra':b'/F 4'},True),
        ('annotation-no-print-flag',{'extra':b'/F 0'},True),
        ('annotation-no-view',{'extra':b'/F 32'},False),
        ('annotation-styling-ignored',{'extra':b'/CA .1 /ca .1 /BM /Multiply /C [0 1 0]'},True),
        ('annotation-state',{'normal':b'<< /Chosen 5 0 R >>','extra':b'/AS /Chosen'},True),
        ('annotation-undefined-state',{'normal':b'<< /Chosen 5 0 R >>','extra':b'/AS /Other'},False),
    ]:
        geometry=[((0,0,8,8),BLUE)]+([((2,1,4,6),RED)] if visible else [])
        yield Case(name,pdf(**options),RENDER,('content-annotations',),'Normal appearance fitted by transformed BBox envelope after page content',oracle=opaque_rectangles(8,8,geometry))
    from .raster import clipped_area
    polygon=[(F(14,3),F(1)),(F(6),F(3)),(F(10,3),F(7)),(F(2),F(5))]
    pixels=bytearray()
    for row in range(8):
        for x in range(8):
            alpha=int(clipped_area(polygon,x,7-row)*255+F(1,2))
            pixels.extend((alpha,0,255-alpha))
    yield Case('annotation-sheared',pdf(matrix=b'[1 1 -1 1 0 0]'),RENDER,('content-annotations',),'All four transformed corners determine the fitted envelope; exact rational pixel intersections',oracle=b'P6\n8 8\n255\n'+pixels,extra={'tolerance':1})
    yield Case('annotation-state-isolation',pdf(form=b'0 0 2 4 re f',page=b'0 0 1 rg 0 0 8 8 re f 4 0 0 4 2 2 cm'),RENDER,('content-annotations',),'Appearance starts in default graphics state rather than page final colour and CTM',oracle=opaque_rectangles(8,8,[((0,0,8,8),BLUE),((2,1,4,6),BLACK)]))
    yield render('annotation-link-borderless',b'',page_extra=b'/Annots [<< /Subtype /Link /Rect [0 0 8 8] /Border [0 0 0] >>]',requirements=('content-annotations',))
    yield Case('annotation-flags',pdf(extra=b'/F 8'),UNSUPPORTED,('content-annotations',),'Fixed display-size annotation needs view fitting',feature='annotation.flags')
    for name,options in [
        ('annotation-invalid-state',{'normal':b'<< /Chosen 5 0 R >>'}),
        ('annotation-singular-matrix',{'matrix':b'[0 0 0 1 0 0]'}),
    ]:
        yield Case(name,pdf(**options),OPEN,('content-annotations',),'Malformed appearance geometry or missing required AS',extra={'render_error':'Syntax'})
    yield render('halftone-continuous',b'/G gs .25 .5 .75 rg 0 0 8 8 re f',[((0,0,8,8),(64,128,191))],resources=b'<< /ExtGState << /G << /HT << /HalftoneType 1 /Frequency 60 /Angle 15 /SpotFunction /Round >> >> >> >>',requirements=('content-halftone',))
    for name,gamma,matrix in [('cie-CalRGB',(1,1,1),(1,0,0,0,1,0,0,0,1)),('cie-CalRGB-matrix',(1.8,2.2,2.4),(.436,.222,.014,.385,.717,.097,.143,.061,.714))]:
        values=[(0,0,0),(1,1,1),(1,0,0),(0,1,0),(0,0,1),(.25,.5,.75),(-1,2,.5),(.6,.2,.3)]
        command=b'/C cs '+b' '.join((' '.join(map(str,v))+f' sc {i} 0 1 8 re f').encode() for i,v in enumerate(values))
        params=(' /Gamma ['+' '.join(map(str,gamma))+'] /Matrix ['+' '.join(map(str,matrix))+']').encode()
        colours=[cie_rgb('CalRGB',v,gamma=gamma,matrix=matrix) for v in values]
        yield Case(name,single_page([command],media_box=b'[0 0 8 8]',resources=b'<< /ColorSpace << /C [/CalRGB << /WhitePoint [.96422 1 .82521] '+params+b' >>] >> >>'),RENDER,('content-cie',),'CalRGB gamma/matrix to XYZ with independently derived sRGB matrix',oracle=opaque_rectangles(8,8,[((i,0,1,8),c) for i,c in enumerate(colours)]),extra={'tolerance':1})


def blend_colour(mode, back, source):
    """Independent continuous colour arithmetic from 11.3.5, Tables 134–135."""
    def luminosity(c): return sum(a*b for a,b in zip(c,(.3,.59,.11)))
    def saturation(c): return max(c)-min(c)
    def set_sat(c, target):
        low, high = min(c), max(c)
        return [(v-low)*target/(high-low) if high>low else 0 for v in c]
    def set_lum(c, target):
        shift=target-luminosity(c);c=[v+shift for v in c]
        low,high=min(c),max(c)
        if low<0: c=[target+(v-target)*target/(target-low) for v in c]
        if high>1: c=[target+(v-target)*(1-target)/(high-target) for v in c]
        return c
    if mode=='Hue': return set_lum(set_sat(source,saturation(back)),luminosity(back))
    if mode=='Saturation': return set_lum(set_sat(back,saturation(source)),luminosity(back))
    if mode=='Color': return set_lum(source,luminosity(back))
    if mode=='Luminosity': return set_lum(back,luminosity(source))
    def hard(b,s): return 2*b*s if s<=.5 else 1-2*(1-b)*(1-s)
    def component(b,s):
        if mode in ('Normal','Compatible'): return s
        if mode=='Multiply': return b*s
        if mode=='Screen': return 1-(1-b)*(1-s)
        if mode=='Overlay': return hard(s,b)
        if mode=='HardLight': return hard(b,s)
        if mode=='Darken': return min(b,s)
        if mode=='Lighten': return max(b,s)
        if mode=='Difference': return abs(b-s)
        if mode=='Exclusion': return b+s-2*b*s
        if mode=='ColorDodge': return 0 if b==0 else 1 if b+s>=1 else b/(1-s)
        if mode=='ColorBurn': return 1 if b==1 else 0 if b+s<=1 else 1-(1-b)/s
        if mode=='SoftLight':
            if s<=.5: return b-2*(.5-s)*b*(1-b)
            d=16*b**3-12*b*b+4*b if b<=.25 else b**.5
            return b+(2*s-1)*(d-b)
        raise ValueError(mode)
    return [component(b,s) for b,s in zip(back,source)]


def transparency_cases():
    byte=lambda v:int(min(1,max(0,v))*255+.5)
    modes=('Normal','Compatible','Multiply','Screen','Overlay','Darken','Lighten',
           'ColorDodge','ColorBurn','HardLight','SoftLight','Difference','Exclusion',
           'Hue','Saturation','Color','Luminosity')
    pairs=[((.2,.4,.6),(.8,.3,.1)),((0,0,0),(1,1,1)),((1,1,1),(0,0,0)),
           ((.1,.2,.9),(.9,.8,.1)),((.25,.25,.25),(.7,.3,.5)),
           ((.9,.1,.5),(.2,.8,.6)),((.75,.5,.2),(.4,.6,.8)),((.01,.99,.2),(.99,.01,.8))]
    for mode in modes:
        background=b' '.join((' '.join(map(str,b))+f' rg {i} 0 1 8 re f').encode() for i,(b,_) in enumerate(pairs))
        foreground=b' '.join((' '.join(map(str,s))+f' rg {i} 0 1 8 re f').encode() for i,(_,s) in enumerate(pairs))
        colours=[tuple(byte((b+v)/2) for b,v in zip(back,blend_colour(mode,back,source))) for back,source in pairs]
        yield Case('blend-'+mode,single_page([background+b' /G gs '+foreground],media_box=b'[0 0 8 8]',resources=b'<< /ExtGState << /G << /ca .5 /BM /'+mode.encode()+b' >> >> >>'),RENDER,('content-transparency',),'Independent blend formulas with half opacity and PDF 2.0 boundary cases',oracle=opaque_rectangles(8,8,[((i,0,1,8),c) for i,c in enumerate(colours)]),extra={'tolerance':1})
    yield render('alpha',b'/G gs 1 0 0 rg 0 0 8 8 re f',[((0,0,8,8),(255,128,128))],resources=b'<< /ExtGState << /G << /ca .5 >> >> >>',requirements=('content-transparency',))
    yield render('alpha-stroke',b'/G gs 0 0 1 RG 2 w 1 4 m 7 4 l S',[((1,3,6,2),(191,191,255))],resources=b'<< /ExtGState << /G << /CA .25 >> >> >>',requirements=('content-transparency',))
    yield render('blend',b'/G gs',resources=b'<< /ExtGState << /G << /BM /Multiply >> >> >>',requirements=('content-transparency',))
    yield render('alpha-combined',b'/G gs 1 0 0 rg 0 G 2 w 2 2 4 4 re B',[((1,1,6,6),(128,128,128)),((3,3,2,2),(255,128,128))],resources=b'<< /ExtGState << /G << /ca .5 /CA .5 >> >> >>',requirements=('content-transparency',))
    yield render('blend-isolated-page',b'/G gs .25 .5 .75 rg 0 0 8 8 re f',[((0,0,8,8),(159,191,223))],resources=b'<< /ExtGState << /G << /ca .5 /BM /Screen >> >> >>',requirements=('content-transparency',))
    for isolated in (False,True):
        for knockout in (False,True):
            flag=(b'true' if isolated else b'false')
            group=b'/Group << /S /Transparency /I '+flag+b' /K '+(b'true' if knockout else b'false')+b' >>'
            form=b'/G gs 1 0 0 rg 0 0 6 8 re f 0 0 1 rg 2 0 6 8 re f'
            pdf=form_pdf(content=form,extra=group,form_resources=b'<< /ExtGState << /G << /ca .5 >> >> >>',page_content=b'/Fm Do')
            # form_pdf clips to a 4x4 box then translates it to [2,2,6,6].
            overlap=(128,128,255) if knockout else (128,64,191)
            geometry=[((2,2,2,4),(255,128,128)),((4,2,2,4),overlap)]
            yield Case(f'group-i{int(isolated)}-k{int(knockout)}',pdf,RENDER,('content-transparency',),'Shape replacement versus source-over, with group defaults reset',oracle=opaque_rectangles(8,8,geometry),extra={'tolerance':1})
    for model,content,colour in [
        ('DeviceGray',b'1 0 0 rg 0 0 4 4 re f /G gs 0 0 1 rg 0 0 4 4 re f',(52,52,52)),
        ('DeviceCMYK',b'1 0 0 1 k 0 0 4 4 re f /G gs 0 0 0 0 k 0 0 4 4 re f',(0,128,128)),
    ]:
        pdf=form_pdf(content=content,extra=b'/Group << /S /Transparency /I true /CS /'+model.encode()+b' >>',form_resources=b'<< /ExtGState << /G << /ca .5 >> >> >>')
        yield Case('group-colour-'+model,pdf,RENDER,('content-transparency',),'Composite native components before converting the group to RGB',oracle=opaque_rectangles(8,8,[((2,2,4,4),colour)]),extra={'tolerance':1})
    for method,options,inside,outside in [
        ('Alpha',b'',.5,0),
        ('Alpha',b'/TR << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>',.5,1),
        ('Luminosity',b'/BC [.25]',.625,.25),
        ('Luminosity',b'/BC [.25] /TR << /FunctionType 2 /Domain [0 1] /N 2 >>',.625**2,.25**2),
    ]:
        suffix='-transfer' if b'/TR' in options else ''
        pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ExtGState << /M << /SMask << /S /'+method.encode()+b' /G 5 0 R '+options+b' >> >> >> >> /Contents 4 0 R >>',stream(b'/M gs 1 0 0 rg 0 0 8 8 re f'),stream(b'/A gs 1 g 0 0 4 8 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 4 8] /Resources << /ExtGState << /A << /ca .5 >> >> >> /Group << /S /Transparency /I true /CS /DeviceGray >>')])
        colours=[((0,0,4,8),(255,byte(1-inside),byte(1-inside))),((4,0,4,8),(255,byte(1-outside),byte(1-outside)))]
        yield Case('soft-'+method+suffix,pdf,RENDER,('content-soft-masks',),'Group alpha/luminosity, backdrop and transfer including outside the BBox',oracle=opaque_rectangles(8,8,colours),extra={'tolerance':1})
    yield Case('soft-mask',single_page([b'/G gs'],media_box=b'[0 0 8 8]',resources=b'<< /ExtGState << /G << /SMask << >> >> >> >>'),OPEN,('content-soft-masks',),'A soft-mask dictionary requires S and G',extra={'render_error':'Syntax'})


def shading_cases():
    from math import hypot,sqrt
    byte=lambda v:int(min(1,max(0,v))*255+.5)
    function=b'/Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >>'
    def image(sample):
        data=bytearray()
        for row in range(8):
            for x in range(8):
                colour=sample(x+.5,7.5-row)
                data.extend((255,255,255) if colour is None else tuple(map(byte,colour)))
        return b'P6\n8 8\n255\n'+data
    def shade(name,dictionary,sample,content=b'/S sh',extra=b''):
        return Case(name,single_page([content],media_box=b'[0 0 8 8]',resources=b'<< /Shading << /S << /ColorSpace /DeviceRGB '+dictionary+b' >> >> '+extra+b' >>'),RENDER,('content-shadings',),'Independent analytic function at device sample locations',oracle=image(sample),extra={'tolerance':1})
    axial=b'/ShadingType 2 /Coords [0 0 8 0] '+function
    yield shade('shading-axial',axial,lambda x,y:(1-x/8,0,x/8))
    yield shade('shading-axial-clip',axial+b'/BBox [2 1 6 7]',lambda x,y:(1-x/8,0,x/8) if 2<x<6 and 1<y<7 else None)
    yield shade('shading-background-ignored',b'/ShadingType 2 /Coords [2 0 6 0] /Background [0 1 0] '+function,lambda x,y:(1-(x-2)/4,0,(x-2)/4) if 2<x<6 else None)
    yield shade('shading-axial-extend',b'/ShadingType 2 /Coords [2 0 6 0] /Extend [true true] '+function,lambda x,y:(1-min(1,max(0,(x-2)/4)),0,min(1,max(0,(x-2)/4))))
    yield shade('shading-axial-empty',b'/ShadingType 2 /Coords [2 2 2 2] '+function,lambda x,y:None)
    # Compare the 2x2 reconstruction filter with the true integral of t squared.
    yield shade('shading-filter',axial.replace(b'/N 1',b'/N 2')+b' /AntiAlias true',lambda x,y:(1-(x*x+1/12)/64,0,(x*x+1/12)/64))
    for name,coords,extend,reverse in [
        ('shading-radial',b'[4 4 0 4 4 4]',False,False),
        ('shading-radial-extend',b'[4 4 0 4 4 4]',True,False),
        ('shading-radial-reverse',b'[4 4 4 4 4 0]',False,True),
    ]:
        def sample(x,y,extend=extend,reverse=reverse):
            t=hypot(x-4,y-4)/4
            if t>1 and not extend: return None
            t=min(1,t);t=1-t if reverse else t
            return 1-t,0,t
        yield shade(name,b'/ShadingType 3 /Coords '+coords+(b' /Extend [true true] ' if extend else b' ')+function,sample)
    def cone(x,y):
        dx=x-2;dy=y-4;discriminant=(8*dx)**2-48*(dx*dx+dy*dy)
        if discriminant<0:return None
        roots=[(8*dx+sign*sqrt(discriminant))/24 for sign in (-1,1)]
        valid=[t for t in roots if 0<=t<=1]
        if not valid:return None
        t=max(valid);return 1-t,0,t
    yield shade('shading-cone',b'/ShadingType 3 /Coords [2 4 0 6 4 2] '+function,cone)
    def linear_cone(x,y):
        dx=x-2;dy=y-4
        if dx<=0:return None
        t=(dx*dx+dy*dy)/(8*dx)
        return (1-t,0,t) if 0<=t<=1 else None
    yield shade('shading-linear-cone',b'/ShadingType 3 /Coords [2 4 0 6 4 4] '+function,linear_cone)
    program=b'{ add dup 1 exch sub exch 0 }'
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Shading << /S << /ShadingType 1 /ColorSpace /DeviceRGB /Matrix [8 0 0 8 0 0] /Function 5 0 R >> >> >> /Contents 4 0 R >>',stream(b'/S sh'),stream(program,b'/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]')])
    yield Case('shading-function',pdf,RENDER,('content-shadings',),'Two-dimensional calculator with shading Matrix',oracle=image(lambda x,y:(1-min(1,(x+y)/8),min(1,(x+y)/8),0)),extra={'tolerance':1})
    for name,content,definition,sample in [
        ('pattern-shading',b'/Pattern cs /P scn 0 0 8 8 re f',axial,lambda x,y:(1-x/8,0,x/8)),
        ('pattern-anchored',b'1 0 0 1 2 0 cm /Pattern cs /P scn 0 0 4 8 re f',axial,lambda x,y:(1-x/8,0,x/8) if 2<x<6 else None),
        ('pattern-stroke',b'/Pattern CS /P SCN 2 w 1 4 m 7 4 l S',axial,lambda x,y:(1-x/8,0,x/8) if 1<x<7 and 3<y<5 else None),
        ('pattern-background',b'/Pattern cs /P scn 0 0 8 8 re f',b'/ShadingType 2 /Coords [2 0 6 0] /Background [0 1 0] /BBox [1 1 7 7] '+function,lambda x,y:(1-(x-2)/4,0,(x-2)/4) if 2<x<6 and 1<y<7 else (0,1,0)),
    ]:
        pdf=single_page([content],media_box=b'[0 0 8 8]',resources=b'<< /Pattern << /P << /PatternType 2 /Shading << /ColorSpace /DeviceRGB '+definition+b' >> >> >> >>')
        yield Case(name,pdf,RENDER,('content-shadings',),'Shading pattern uses parent initial space and paints the path shape once',oracle=image(sample),extra={'tolerance':1})
    yield render('pattern-initial',b'/Pattern cs 0 0 8 8 re f',requirements=('content-shadings',))
    mesh=stream(bytes([0,0,0,255,0,0,0,255,0,0,255,0,0,0,255,0,0,255]),b'/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerFlag 8 /BitsPerCoordinate 8 /BitsPerComponent 8 /Decode [0 8 0 8 0 1 0 1 0 1]')
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Shading << /S 5 0 R >> >> /Contents 4 0 R >>',stream(b'/S sh'),mesh])
    yield Case('shading',pdf,UNSUPPORTED,('content-shadings',),'Free-form triangle meshes are explicit unsupported input',feature='shading.mesh')
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Pattern << /P 5 0 R >> >> /Contents 4 0 R >>',stream(b'/Pattern cs /P scn 0 0 8 8 re f'),stream(b'0 0 1 1 re f',b'/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 1 1] /XStep 2 /YStep 2 /Resources << >>')])
    yield Case('pattern',pdf,UNSUPPORTED,('content-shadings',),'Tiling patterns are explicit unsupported input',feature='pattern.tiling')


def mask_geometry_cases():
    yield Case('form-empty-group',form_pdf(content=b'',extra=b'/Group << /S /Transparency /I true >>'),RENDER,('content-forms','content-transparency'),'An empty transparency group contributes no painted marks',oracle=opaque_rectangles(8,8,[]))
    value=int(255*(1-.001)**300+.5)
    yield render('alpha-accumulation',b'/G gs '+b'0 0 8 8 re f '*300,[((0,0,8,8),(value,value,value))],resources=b'<< /ExtGState << /G << /ca .001 >> >> >>',requirements=('content-transparency',))
    resources=b'<< /XObject << /Fm 5 0 R >> /ExtGState << /A << /ca .5 >> >> >>'
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources '+resources+b' /Contents 4 0 R >>',stream(b'/A gs /Fm Do'),stream(b'1 0 0 rg 0 0 8 8 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 8 8] /Resources << >> /Group << /S /Transparency /I true >>')])
    yield Case('group-incoming-alpha',pdf,RENDER,('content-transparency',),'Incoming opacity applies once to the group result and resets inside G',oracle=opaque_rectangles(8,8,[((0,0,8,8),(255,128,128))]))
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ExtGState << /M << /SMask << /S /Alpha /G 5 0 R >> >> >> >> /Contents 4 0 R >>',stream(b'/M gs 1 0 0 1 4 0 cm 1 0 0 rg -4 0 8 8 re f'),stream(b'1 g 0 0 4 8 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 4 8] /Resources << >> /Group << /S /Transparency /I true /CS /DeviceGray >>')])
    yield Case('soft-frozen-transform',pdf,RENDER,('content-soft-masks',),'A later cm changes the painted path but does not move the captured soft mask',oracle=opaque_rectangles(8,8,[((0,0,4,8),RED)]))
    pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /CropBox [0 0 7.5 8] /Resources << /ExtGState << /M << /SMask << /S /Alpha /G 5 0 R >> >> >> >> /Contents 4 0 R >>',stream(b'/M gs 0 0 16 8 re f'),stream(b'1 g 0 0 16 8 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 16 8] /Resources << >> /Group << /S /Transparency /I true /CS /DeviceGray >>')])
    pixels=bytes([0,0,0]*7+[127,127,127])*8
    yield Case('soft-crop-once',pdf,RENDER,('content-soft-masks',),'Mask source remains independent of the page crop; last column has half geometric coverage',oracle=b'P6\n8 8\n255\n'+pixels)
    for ais in (False,True):
        cell=b'0 0 1 rg 0 0 4 4 re f /A gs 1 0 0 rg 0 0 4 4 re f'
        pdf=form_pdf(content=cell,extra=b'/Group << /S /Transparency /I true /K true >>',form_resources=b'<< /ExtGState << /A << /ca .5 /AIS '+(b'true' if ais else b'false')+b' >> >> >>')
        colour=(128,0,128) if ais else (255,128,128)
        yield Case('group-alpha-shape-'+str(int(ais)),pdf,RENDER,('content-transparency',),'Knockout replaces prior contributions using shape separately from opacity',oracle=opaque_rectangles(8,8,[((2,2,4,4),colour)]),extra={'tolerance':1})


def invalid_compositing_cases():
    for name, mask, group in [
        ('soft-invalid-backdrop', b'/S /Luminosity /G 5 0 R /BC [0 1]', b'/CS /DeviceGray'),
        ('soft-missing-space', b'/S /Luminosity /G 5 0 R', b''),
        ('soft-recursive', b'/S /Alpha /G 5 0 R', b'/CS /DeviceGray'),
    ]:
        recursive = name == 'soft-recursive'
        resources = b'/Resources << /ExtGState << /M << /SMask << '+mask+b' >> >> >> >>'
        form = b'/M gs 0 0 8 8 re f' if recursive else b'0 0 8 8 re f'
        pdf = document([b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>', b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] '+resources+b' /Contents 4 0 R >>', stream(b'/M gs 0 0 8 8 re f'), stream(form, b'/Type /XObject /Subtype /Form /BBox [0 0 8 8] '+resources+b' /Group << /S /Transparency '+group+b' >>')])
        yield Case(name,pdf,OPEN,('content-soft-masks',),'Malformed mask backdrop, missing luminosity space or recursive group',extra={'render_error':'Cycle' if recursive else 'Syntax'})
    for name, definition in [
        ('shading-negative-radius', b'/ShadingType 3 /Coords [0 0 -1 4 4 2] /Function << /FunctionType 2 /Domain [0 1] /N 1 >>'),
        ('shading-function-domain', b'/ShadingType 2 /Coords [0 0 8 0] /Domain [0 2] /Function << /FunctionType 2 /Domain [0 1] /N 1 >>'),
    ]:
        pdf=single_page([b'/S sh'],media_box=b'[0 0 8 8]',resources=b'<< /Shading << /S << /ColorSpace /DeviceGray '+definition+b' >> >> >>')
        yield Case(name,pdf,OPEN,('content-shadings',),'Required radius and function-domain invariants',extra={'render_error':'Syntax'})


def dash_space_cases():
    """User-distance intervals, transformed pen geometry and rational pixel areas.

    Coverage and final half-up quantization are project policy; user-space dash
    distances, phase parity and per-subpath restart follow 8.4.3.6 / Table 55.
    """
    from math import hypot
    from .raster import clipped_area
    def intervals(array, phase, length):
        if not array:return [(F(0),length)]
        pattern=list(map(F,array))*2;period=sum(pattern);phase=F(phase)%period
        prefix=[F(0)]
        for v in pattern:prefix.append(prefix[-1]+v)
        return [(max(F(0),k*period+prefix[i]-phase),min(length,k*period+prefix[i+1]-phase))
                for k in range(-1,int(length//period)+2) for i in range(0,len(pattern),2)
                if max(F(0),k*period+prefix[i]-phase)<min(length,k*period+prefix[i+1]-phase)]
    def oracle(matrix, lines, width, array, phase, height):
        a,b,c,d,e,f=map(F,matrix)
        transform=lambda p:(a*p[0]+c*p[1]+e,b*p[0]+d*p[1]+f)
        polygons=[]
        for start,end in lines:
            start,end=tuple(map(F,start)),tuple(map(F,end));dx=end[0]-start[0];dy=end[1]-start[1]
            length=F(hypot(dx,dy));direction=(dx/length,dy/length)
            for low,high in intervals(array,phase,length):
                p=tuple(v+low*t for v,t in zip(start,direction));q=tuple(v+high*t for v,t in zip(start,direction))
                if width:
                    n=(-direction[1]*F(width)/2,direction[0]*F(width)/2)
                    polygon=[transform((p[0]+n[0],p[1]+n[1])),transform((q[0]+n[0],q[1]+n[1])),transform((q[0]-n[0],q[1]-n[1])),transform((p[0]-n[0],p[1]-n[1]))]
                else:
                    p,q=transform(p),transform(q);size=F(hypot(q[0]-p[0],q[1]-p[1]));n=(-(q[1]-p[1])/size/2,(q[0]-p[0])/size/2)
                    polygon=[(p[0]+n[0],p[1]+n[1]),(q[0]+n[0],q[1]+n[1]),(q[0]-n[0],q[1]-n[1]),(p[0]-n[0],p[1]-n[1])]
                polygons.append(polygon)
        pixels=bytearray()
        for row in range(height):
            for x in range(32):
                alpha=int(min(F(1),sum(clipped_area(p,x,height-1-row) for p in polygons))*255+F(1,2));pixels.extend([255-alpha]*3)
        return f'P6\n32 {height}\n255\n'.encode()+pixels
    entries=[
        ('dash-hairline-scaled',(2,0,0,2,0,0),[((1,4),(15,4))],0,[2,2],0,16),
        ('dash-hairline-expanded',(1,0,0,1,0,0),[((2,8),(30,8))],0,[4,4],0,16),
        ('dash-hairline-rotated',(0,2,-2,0,24,0),[((1,4),(15,4))],0,[2,2],0,32),
        ('dash-hairline-anisotropic',(2,0,0,1,0,0),[((1,4),(15,4)),((3,8),(3,15))],0,[2,2],0,16),
        ('dash-width-scaled',(2,0,0,2,0,0),[((1,4),(15,4))],1,[2,2],0,16),
        ('dash-width-expanded',(1,0,0,1,0,0),[((2,8),(30,8))],2,[4,4],0,16),
        ('dash-hairline-rotated-expanded',(1,0,0,1,0,0),[((16,2),(16,30))],0,[4,4],0,32),
        ('dash-width-rotated-expanded',(1,0,0,1,0,0),[((16,2),(16,30))],2,[4,4],0,32),
        ('dash-hairline-oblique',(2,0,0,1,0,0),[((1,1),(10,13))],0,[2,2],0,16),
        ('dash-width-rotated',(0,2,-2,0,24,0),[((1,4),(15,4))],1,[2,2],0,32),
        ('dash-width-anisotropic',(2,0,0,1,0,0),[((1,4),(15,4)),((3,8),(3,15))],1,[2,2],0,16),
    ]
    for phase,label in [(-1,'negative'),(9,'normalized'),(14,'cycle'),(-10000000001,'large-negative')]:
        entries.append(('dash-phase-'+label,(1,0,0,1,0,0),[((1,8),(30,8))],2,[3,2],phase,16))
    for phase,label in [(-2,'negative'),(10,'normalized'),(22,'cycle')]:
        entries.append(('dash-odd-'+label,(1,0,0,1,0,0),[((1,8),(30,8))],2,[2,1,3],phase,16))
    entries.append(('dash-empty-negative',(1,0,0,1,0,0),[((1,8),(30,8))],2,[],-1000000000,16))
    for name,matrix,lines,width,array,phase,height in entries:
        contents=(' '.join(map(str,matrix))+' cm '+str(width)+' w ['+' '.join(map(str,array))+'] '+str(phase)+' d '+ ' '.join(f'{p[0]} {p[1]} m {q[0]} {q[1]} l S' for p,q in lines)).encode()
        pdf=single_page([contents],media_box=f'[0 0 32 {height}]'.encode())
        expected=oracle(matrix,lines,width,array,phase,height)
        yield Case(name,pdf,RENDER,('raster-dashes',),'Independent user-space dash intervals and transformed stroke coverage',oracle=expected,extra={'tolerance':1})
        if name.endswith('-anisotropic'):
            w1,w2=(0,0) if width==0 else (1,2)
            baked=f'{w1} w [4 4] 0 d 2 4 m 30 4 l S {w2} w [2 2] 0 d 6 8 m 6 15 l S'.encode()
            yield Case(name+'-expanded',single_page([baked],media_box=b'[0 0 32 16]'),RENDER,('raster-dashes',),'Equivalent anisotropic geometry has different dash spacing and pen width along each axis',oracle=expected,extra={'tolerance':1})


ICC_D50=(.9642,1,.8249)
ICC_RGB_MATRIX=(.4360747,.2225045,.0139322,.3850649,.7168786,.0971045,.1430804,.0606169,.7141733)


def icc_fixed(value):
    return int(F(value)*65536+F(1,2)) if value>=0 else -int(-F(value)*65536+F(1,2))


def icc_curve(kind, values):
    import struct
    if kind=='sampled':return b'curv'+bytes(4)+struct.pack('>I',len(values))+b''.join(struct.pack('>H',v) for v in values)
    if kind=='gamma':return b'curv'+bytes(4)+struct.pack('>IH',1,int(values[0]*256+.5))
    if kind=='identity':return b'curv'+bytes(4)+bytes(4)
    return b'para'+bytes(4)+struct.pack('>HH',kind,0)+b''.join(struct.pack('>i',icc_fixed(v)) for v in values)


def icc_profile(count=3, version=4, curve=None, lut=None):
    """Construct tag-defined ICC matrix/TRC data; primary ICC-text review pending.

    Version 2.1 uses text/desc metadata; version 4.3 uses mluc. All data is
    independently authored. A version-5 header exercises PDF's later-version
    fallback; it is not a claim of ICCmax conformance.
    """
    import struct
    curve=curve or icc_curve('gamma',(2.2,))
    xyz=lambda v:b'XYZ '+bytes(4)+b''.join(struct.pack('>i',icc_fixed(x)) for x in v)
    def mluc(value):
        data=value.encode('utf-16-be')
        return b'mluc'+bytes(4)+struct.pack('>II',1,12)+b'enUS'+struct.pack('>II',len(data),28)+data
    tags={b'wtpt':xyz(ICC_D50),b'cprt':mluc('Public domain test data'),b'desc':mluc('Independent matrix profile')}
    if version==2:
        text=b'Independent matrix profile\0'
        tags[b'desc']=b'desc'+bytes(4)+struct.pack('>I',len(text))+text+bytes(4+4+2+1+67)
        tags[b'cprt']=b'text'+bytes(4)+b'Public domain test data\0'
    if count==3:
        for i,prefix in enumerate((b'r',b'g',b'b')):
            tags[prefix+b'XYZ']=xyz(ICC_RGB_MATRIX[i*3:i*3+3]);tags[prefix+b'TRC']=curve[i] if isinstance(curve,(tuple,list)) else curve
    elif count==1:tags[b'kTRC']=curve
    if lut is not None:
        name=lut
        matrix=b''.join(struct.pack('>i',65536 if i in (0,4,8) else 0) for i in range(9))
        if lut==b'mft1':data=lut+bytes(4)+bytes([count,3,2,0])+matrix+bytes(range(256))*count+bytes(2**count*3)+bytes(range(256))*3
        elif lut==b'mft2':data=lut+bytes(4)+bytes([count,3,2,0])+matrix+struct.pack('>HH',2,2)+struct.pack('>HH',0,65535)*count+bytes(2**count*3*2)+struct.pack('>HH',0,65535)*3
        else:
            # ICC v4 multi-process-element offsets, identity B curves and CLUT.
            inputs,outputs=(3,count) if lut==b'mBA ' else (count,3)
            curves=icc_curve('identity',())*3
            data=lut+bytes(4)+bytes([inputs,outputs,0,0])+struct.pack('>IIIII',32,0,0,32+len(curves),0)+curves+bytes([2]*inputs+[0]*(16-inputs))+bytes([2,0,0,0])+bytes(2**inputs*outputs*2)
        tags[b'B2A0' if lut==b'mBA ' else b'A2B0']=data
    header=bytearray(128);header[8:12]=bytes([version,0x10 if version==2 else 0x30,0,0]);header[12:16]=b'prtr' if count==4 else b'mntr';header[16:20]={1:b'GRAY',3:b'RGB ',4:b'CMYK'}[count];header[20:24]=b'XYZ ';header[24:36]=struct.pack('>6H',2026,10,17,12,0,0);header[36:40]=b'acsp';header[68:80]=b''.join(struct.pack('>i',icc_fixed(v)) for v in ICC_D50)
    start=132+12*len(tags);entries=bytearray();payload=bytearray()
    for signature, data in sorted(tags.items()):
        entries+=signature+struct.pack('>II',start+len(payload),len(data));payload+=data;payload+=bytes(-len(payload)%4)
    result=header+struct.pack('>I',len(tags))+entries+payload;result[0:4]=struct.pack('>I',len(result));return bytes(result)


def icc_cases():
    from bisect import bisect_right
    def pdf(profile,count=3,command=b'/C cs .5 .5 .5 scn 0 0 8 8 re f',alternate=b'/DeviceRGB'):
        return document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ColorSpace << /C [/ICCBased 5 0 R] >> >> /Contents 4 0 R >>',stream(command),stream(profile,b'/N '+str(count).encode()+b' /Alternate '+alternate)])
    values=[0,.02,.04,.1,.25,.5,.75,1]
    samples=tuple(int((i/16)**2.2*65535+.5) for i in range(17))
    definitions=[('identity','identity',()),('gamma','gamma',(2.2,)),('sampled','sampled',samples),('param0',0,(2.2,)),('param1',1,(2,1,-.1)),('param2',2,(2,1,-.1,.05)),('param3',3,(2,1,0,.2,.2)),('param4',4,(2.4,1/1.055,.055/1.055,1/12.92,.04045,0,0))]
    for name,kind,parameters in definitions:
        def evaluate(x):
            if kind=='identity':return x
            if kind=='gamma':return x**(int(parameters[0]*256+.5)/256)
            if kind=='sampled':
                knots=[i/(len(parameters)-1) for i in range(len(parameters))];j=min(len(knots)-2,bisect_right(knots,x)-1);t=(x-knots[j])/(knots[j+1]-knots[j]);return ((1-t)*parameters[j]+t*parameters[j+1])/65535
            p=[icc_fixed(v)/65536 for v in parameters]+[0]*7;g,a,b,c,d,e,f=p[:7]
            if kind==0:return x**g
            if kind in (1,2):return (max(0,a*x+b)**g)+(c if kind==2 else 0)
            return (a*x+b)**g+(e if kind==4 else 0) if x>=d else c*x+(f if kind==4 else 0)
        for count in (1,3):
            curve=icc_curve(kind,parameters);profile=icc_profile(count,version=2 if kind in ('identity','gamma','sampled') else 4,curve=curve)
            command=b'/C cs '+b' '.join((' '.join([str(v)]*count)+f' scn {i} 0 1 8 re f').encode() for i,v in enumerate(values))
            colours=[cie_rgb('CalGray',(evaluate(v),),white=ICC_D50) if count==1 else cie_rgb('CalRGB',(evaluate(v),)*3,white=ICC_D50,matrix=tuple(icc_fixed(v)/65536 for v in ICC_RGB_MATRIX)) for v in values]
            yield Case(f'icc-{count}-{name}',pdf(profile,count,command,b'/DeviceGray' if count==1 else b'[/Lab << /WhitePoint [.9642 1 .8249] >>]'),RENDER,('content-icc-matrix',),'Tag-defined matrix/TRC PCS conversion; independent ICC formula oracle pending primary-text review',oracle=opaque_rectangles(8,8,[((i,0,1,8),c) for i,c in enumerate(colours)]),extra={'tolerance':1})
    rgb_values=[(0,0,0),(1,1,1),(1,0,0),(0,1,0),(0,0,1),(.25,.5,.75),(.6,.2,.3),(.08,.04,.02)]
    gammas=(1.8,2.2,2.4)
    profile=icc_profile(3,version=2,curve=[icc_curve('gamma',(g,)) for g in gammas])
    command=b'/C cs '+b' '.join((' '.join(map(str,v))+f' scn {i} 0 1 8 re f').encode() for i,v in enumerate(rgb_values))
    colours=[cie_rgb('CalRGB',tuple(x**(int(g*256+.5)/256) for x,g in zip(v,gammas)),white=ICC_D50,matrix=tuple(icc_fixed(v)/65536 for v in ICC_RGB_MATRIX)) for v in rgb_values]
    yield Case('icc-rgb-columns',pdf(profile,command=command),RENDER,('content-icc-matrix',),'Independent unequal per-channel TRCs and PCS matrix columns; primary ICC-text review pending',oracle=opaque_rectangles(8,8,[((i,0,1,8),c) for i,c in enumerate(colours)]),extra={'tolerance':1})
    yield Case('icc-initial-zero',pdf(icc_profile(4,version=5),4,b'/C cs 0 0 8 8 re f',b'/DeviceCMYK'),RENDER,('content-icc-matrix',),'Later-version fallback retains ICCBased all-zero initialization, rather than DeviceCMYK black',oracle=opaque_rectangles(8,8,[]))
    yield Case('icc-simple-operator',pdf(icc_profile(),command=b'/C cs .5 .5 .5 sc'),OPEN,('content-icc-matrix',),'ICCBased retains the SCN/scn applicability rule of Table 73',extra={'render_error':'Syntax'})
    yield Case('icc-invalid-data',pdf(b'not a profile'),OPEN,('content-icc-matrix',),'Non-profile data cannot establish ICC fallback',extra={'render_error':'Syntax'})
    for kind in (b'mft1',b'mft2',b'mAB ',b'mBA '):
        yield Case('icc-lut-'+kind.decode().strip(),pdf(icc_profile(4,lut=kind),4,b'/C cs 0 0 0 0 scn 0 0 8 8 re f',b'/DeviceCMYK'),UNSUPPORTED,('content-icc-matrix',),'Tag-defined LUT profile requires explicit processing rather than alternate substitution; primary ICC review pending',feature='colorspace.ICCBased')


def standard_semantics_cases():
    def invalid(name,command,resources=b'<< >>',page_extra=b'',requirement='content-vector-pipeline'):
        return Case(name,single_page([command],media_box=b'[0 0 8 8]',resources=resources,page_extra=page_extra),OPEN,(requirement,),'Required syntax and source-family constraints',extra={'render_error':'Syntax'})
    for op in ('S','s','f','F','f*','B','B*','b','b*','n'):
        yield invalid('path-undefined-'+{'S':'stroke','s':'close-stroke','f':'fill','F':'fill-alias','f*':'parity-fill','B':'fill-stroke','B*':'parity-fill-stroke','b':'close-fill-stroke','b*':'close-parity-fill-stroke','n':'end'}[op],op.encode())
    yield invalid('path-construction-after-clip',b'0 0 4 4 re W 5 5 l n')
    for name,command in [('parameter-width-clipped',b'-1 w 1 4.5 m 7 4.5 l S'),('parameter-miter-clipped',b'.5 M 0 0 8 8 re f'),('parameter-flatness-high',b'101 i 0 0 8 8 re f'),('parameter-flatness-negative',b'-1 i 0 0 8 8 re f'),('intent-unknown',b'/CustomIntent ri 0 0 8 8 re f')]:
        geometry=[((1,4,6,1),BLACK)] if name=='parameter-width-clipped' else [((0,0,8,8),BLACK)]
        yield render(name,command,geometry)
    for name,command,geometry in [
        ('parameter-cap-low',b'-10 J 2 w 2 4 m 6 4 l S',[((2,3,4,2),BLACK)]),
        ('parameter-cap-high',b'10 J 2 w 2 4 m 6 4 l S',[((1,3,6,2),BLACK)]),
        ('parameter-alpha-high',b'/G gs 1 0 0 rg 0 0 8 8 re f',[((0,0,8,8),RED)]),
    ]:
        yield render(name,command,geometry,resources=b'<< /ExtGState << /G << /ca 2 /CA -1 /SM -5 >> >> >>')
    for suffix in ('f','W n 0 0 8 8 re f'):
        yield render('point-'+('fill' if suffix=='f' else 'clip'),b'4.25 4.25 m h '+suffix.encode(),[((4,4,1,1),BLACK)],requirements=('raster-degenerate',))
    from .raster import sampled
    dot=sampled('circle', '',lambda x,y:(x-4)**2+(y-4)**2<=2.25).files['.pgm'].split(b'\n',3)[-1]
    disk=b'P6\n8 8\n255\n'+bytes(c for alpha in dot for c in [255-alpha]*3)
    for name,tail,cap,adjust,painted in [('stroke-closed-point',b'h S',1,False,True),('stroke-closed-duplicates',b'4 4 l h S',1,False,True),('stroke-square-point',b'4 4 l S',2,False,False),('stroke-open-point',b'S',1,False,False),('stroke-adjusted-open-point',b'S',1,True,False)]:
        command=(b'/G gs ' if adjust else b'')+f'3 w {cap} J 4 4 m '.encode()+tail
        yield Case(name,single_page([command],media_box=b'[0 0 8 8]',resources=b'<< /ExtGState << /G << /SA true >> >> >>'),RENDER,('raster-strokes',),'8.5.3.2 degenerate and trailing open subpaths, with project-policy circle coverage',oracle=disk if painted else opaque_rectangles(8,8,[]),extra={'tolerance':3 if painted else 0})
    yield render('stroke-adjusted-open-oblique',b'1 1 -1 1 4 0 cm /G gs 3 w 1 J 4 4 m S',resources=b'<< /ExtGState << /G << /SA true >> >> >>',requirements=('raster-strokes',))
    for family,command,colour in [('DeviceGray',b'.25 sc',(64,)*3),('DeviceRGB',b'1 0 0 sc',RED),('DeviceCMYK',b'0 1 1 0 sc',RED)]:
        yield render('space-array-'+family,b'/C cs '+command+b' 0 0 8 8 re f',[((0,0,8,8),colour)],resources=b'<< /ColorSpace << /C [/'+family.encode()+b'] >> >>',requirements=('content-colour',))
    cal=b'[/CalGray << /WhitePoint [.95047 1 1.08883] /Gamma 2 >>]'
    tint=b'[/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >>]'
    for name,space,value in [('indexed',b'[/Indexed /DeviceGray 0 <80>]',0),('tint',tint,.5)]:
        colour=cie_rgb('CalGray',(128/255 if name=='indexed' else .5,),white=(.95047,1,1.08883),gamma=2)
        yield render('default-underlying-'+name,f'/C cs {value} scn 0 0 8 8 re f'.encode(),[((0,0,8,8),colour)],resources=b'<< /ColorSpace << /DefaultGray '+cal+b' /C '+space+b' >> >>',requirements=('content-colour',))
    for name,command,resources in [
        ('default-illegal-indexed',b'0 g',b'<< /ColorSpace << /DefaultGray [/Indexed /DeviceRGB 0 <ff0000>] >> >>'),
        ('default-illegal-Lab',b'0 0 0 rg',b'<< /ColorSpace << /DefaultRGB [/Lab << /WhitePoint [.9642 1 .8249] >>] >> >>'),
        ('space-nested-alias',b'/C cs',b'<< /ColorSpace << /C [/Indexed /Base 0 <ff0000>] /Base /DeviceRGB >> >>'),
        ('indexed-excess',b'/C cs',b'<< /ColorSpace << /C [/Indexed /DeviceRGB 0 <ff000000>] >> >>'),
        ('tint-simple-operator',b'/C cs .5 sc',b'<< /ColorSpace << /C '+tint+b' >> >>'),
        ('DeviceN-forbidden-All',b'/C cs',b'<< /ColorSpace << /C [/DeviceN [/All] /DeviceGray << /FunctionType 2 /Domain [0 1] /N 1 >>] >> >>'),
    ]:yield invalid(name,command,resources,requirement='content-colour')
    for exponent,domain,label in [(-1,b'[0 1]','negative'),(.5,b'[-1 1]','fractional')]:
        space=b'[/Separation /Spot /DeviceGray << /FunctionType 2 /Domain '+domain+b' /N '+str(exponent).encode()+b' >>]'
        yield invalid('function-domain-'+label,b'/C cs .5 scn',b'<< /ColorSpace << /C '+space+b' >> >>',requirement='content-functions')
    for boundary in (0,1):
        values=(0,.001,.25,.5,.75,.999,1,2)
        function=b'<< /FunctionType 3 /Domain [0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [1] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [0] /N 1 >>] /Bounds ['+str(boundary).encode()+b'] /Encode [0 1 0 1] >>'
        contents=b'/C cs '+b' '.join(f'{v} scn {i} 0 1 8 re f'.encode() for i,v in enumerate(values))
        geometry=[((i,0,1,8),(255,)*3 if (v==0 if boundary==0 else v<1) else BLACK) for i,v in enumerate(values)]
        yield render('stitch-endpoint-'+str(boundary),contents,geometry,resources=b'<< /ColorSpace << /C [/Separation /Spot /DeviceGray '+function+b'] >> >>',requirements=('content-functions',))
    for boundary in (0,1):
        values=(0,.001,.25,.5,.75,.999,1,2)
        function=b'<< /FunctionType 3 /Domain [0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /N 1 >> << /FunctionType 2 /Domain [0 1] /N 1 >>] /Bounds ['+str(boundary).encode()+b'] /Encode [.25 .75 .5 1] >>'
        contents=b'/C cs '+b' '.join(f'{v} scn {i} 0 1 8 re f'.encode() for i,v in enumerate(values))
        mapped=[(.25 if v==0 else .5+.5*min(1,v)) if boundary==0 else (.25+.5*v if v<1 else .5) for v in values]
        yield render('stitch-endpoint-encode-'+str(boundary),contents,[((i,0,1,8),(int(v*255+.5),)*3) for i,v in enumerate(mapped)],resources=b'<< /ColorSpace << /C [/Separation /Spot /DeviceGray '+function+b'] >> >>',requirements=('content-functions',))
    for intent in ('RelativeColorimetric','AbsoluteColorimetric'):
        yield render('intent-'+intent,('/'+intent+' ri /C cs 1 sc 0 0 8 8 re f').encode(),resources=b'<< /ColorSpace << /C [/CalGray << /WhitePoint [.96422 1 .82521] >>] >> >>',requirements=('content-cie',))
    for names,kind in [(b'[/None /None]',UNSUPPORTED),(b'[/None /All]',OPEN)]:
        name='DeviceN-duplicate-None' if kind==UNSUPPORTED else 'DeviceN-All-after-None'
        pdf=document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /ColorSpace << /C [/DeviceN '+names+b' /DeviceGray 5 0 R] >> >> /Contents 4 0 R >>',stream(b'/C cs'),stream(b'{ pop pop 0 }',b'/FunctionType 4 /Domain [0 1 0 1] /Range [0 1]')])
        yield Case(name,pdf,kind,('content-colour',),'None may repeat but All is forbidden even after a None colourant',feature='colorspace.special-ink' if kind==UNSUPPORTED else None,extra={} if kind==UNSUPPORTED else {'render_error':'Syntax'})
    for label,ocmd in [('missing',b'<< /Type /OCMD >>'),('empty',b'<< /Type /OCMD /OCGs [] /P /AllOff >>'),('null',b'<< /Type /OCMD /OCGs [null 99 0 R] >>')]:
        pdf=optional_pdf(b'/OC /Members BDC 0 0 8 8 re f EMC',ocmd=ocmd)
        yield Case('membership-'+label,pdf,RENDER,('content-optional',),'Empty effective OCG lists impose no visibility restriction',oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)]))
    for policy,visible in [('AnyOn',False),('AllOn',False),('AnyOff',True),('AllOff',True)]:
        pdf=optional_pdf(b'/OC /Members BDC 0 0 8 8 re f EMC',ocmd=b'<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /'+policy.encode()+b' >>',configuration=b'/BaseState /ON /OFF [5 0 R]',usage_off=b'/Intent /Design')
        yield Case('membership-intent-'+policy,pdf,RENDER,('content-optional',),'Nonmatching intents are removed before membership aggregation',oracle=opaque_rectangles(8,8,[((0,0,8,8),BLACK)] if visible else []))
    for name,config in [('base-off',b'/BaseState /OFF'),('intent-empty',b'/Intent []')]:
        yield Case('configuration-invalid-'+name,optional_pdf(b'',configuration=config),OPEN,('content-optional',),'Default configuration constraints from Table 99',extra={'render_error':'Syntax'})
    yield render('marked-OC-other',b'/OC << /Other 1 >> BDC 0 0 8 8 re f EMC',[((0,0,8,8),BLACK)],requirements=('content-optional',))
    hidden=document([b'<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /BaseState /ON /OFF [6 0 R] >> >> >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /F 5 0 R >> >> /Contents 4 0 R >>',stream(b'/F Do'),stream(b'BT ET',b'/Type /XObject /Subtype /Form /BBox [0 0 8 8] /Resources << >> /OC 6 0 R'),b'<< /Type /OCG /Name (hidden) >>'])
    yield Case('form-hidden-body',hidden,RENDER,('content-forms',),'An invisible form skips its entire body, including unsupported text operators',oracle=opaque_rectangles(8,8,[]))
