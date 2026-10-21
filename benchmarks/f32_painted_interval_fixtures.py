"""Two dense dynamic-source controls with constant, independent RGB values."""
from fixturegen.pdf import document, stream

CASES = ("f32-shading-dense", "f32-pattern-dense")
SIDE = 512


def fixture(name):
    if name not in CASES:
        raise ValueError("unknown F32 dynamic-source fixture")
    shade = (b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 512 0] "
             b"/Extend [true true] /Function << /FunctionType 2 /Domain [0 1] "
             b"/C0 [0 0 0] /C1 [0 0 0] /N 1 >> >>")
    if name == "f32-shading-dense":
        resource = b"/Shading << /S " + shade + b" >>"
        content = b"/Quarter gs /S sh"
    else:
        resource = b"/Pattern << /P << /PatternType 2 /Shading " + shade + b" >> >>"
        content = b"/Quarter gs /Pattern cs /P scn 0 0 512 512 re f"
    resources = b"<< /ExtGState << /Quarter << /ca .25 >> >> " + resource + b" >>"
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 512 512] /Resources "
        + resources + b" /Contents 4 0 R >>",
        stream(content),
    ])
    # Constant black, opacity1/4, opaque white backdrop:255*(1-1/4)=191.25.
    # Nearest RGB8 is191 everywhere. No PDF parsing, function evaluation,
    # renderer output, coverage algorithm or generated geometry feeds this oracle.
    return pdf, b"P6\n512 512\n255\n" + bytes([191]) * (SIDE * SIDE * 3)
