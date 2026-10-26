"""Bootstrap-era cases: classic xref files with opaque, pixel-aligned content."""
from __future__ import annotations

from .model import INVALID, OPEN, RENDER, UNSUPPORTED, Case
from .pdf import document, opaque_rectangles, single_page, stream

AREA = "classic"

RED, GREEN, BLUE, BLACK = (255, 0, 0), (0, 255, 0), (0, 0, 255), (0, 0, 0)
BASE = ("syntax-classic-xref", "page-tree")


def render(name, description, contents, geometry, size=64, requirements=(), **options):
    return Case(name, single_page(contents, **options), RENDER,
                BASE + ("opaque-rectangle-rendering",) + tuple(requirements), description,
                oracle=opaque_rectangles(size, size, geometry))


def unsupported(name, description, feature, contents=(b"",), **options):
    return Case(name, single_page(list(contents), **options), UNSUPPORTED,
                BASE + ("unsupported-is-explicit",), description, feature=feature, oracle=None)


def cases():
    yield render("blank", "empty content stream", [b""], [])
    yield render("rectangle", "one opaque DeviceRGB rectangle",
                 [b"1 0 0 rg 8 8 48 32 re f\n"], [((8, 8, 48, 32), RED)])
    yield render("graphics-state", "q/Q restores colour and translation",
                 [b"q 1 0 0 1 8 16 cm 0 0 1 rg 0 0 16 16 re f Q 0 g 0 0 8 8 re f\n"],
                 [((8, 16, 16, 16), BLUE), ((0, 0, 8, 8), BLACK)])
    yield render("negative-rectangle", "re with negative width and height",
                 [b"0 1 0 rg 48 48 -32 -16 re f\n"], [((48, 48, -32, -16), GREEN)])
    yield render("inherited-box", "MediaBox inherited from the Pages node",
                 [b"0 g 16 16 32 32 re f\n"], [((16, 16, 32, 32), BLACK)], inherited_box=True)
    yield render("clipped-to-page", "geometry beyond the page is clipped",
                 [b"0 g -16 -16 32 32 re f\n"], [((-16, -16, 32, 32), BLACK)])
    commands, geometry = [], []
    for y in range(0, 512, 16):
        for x in range(0, 512, 16):
            color = RED if (x + y) % 32 else BLUE
            commands.append(b"%d 0 %d rg %d %d 16 16 re f\n" % (color[0] // 255, color[2] // 255, x, y))
            geometry.append(((x, y, 16, 16), color))
    yield render("many-rectangles", "1024 rectangles", [b"".join(commands)], geometry, size=512,
                 media_box=b"[0 0 512 512]")

    # Clause 7.8.2: an array of content streams is one token sequence; a
    # stream may end between any two tokens, even between operands and operator.
    yield render("content-array-split", "Contents array split between tokens and inside an operation",
                 [b"0 g 0 0 8 8 re f", b"q 0 0 1 rg 16 16 16", b"16 re f Q"],
                 [((0, 0, 8, 8), BLACK), ((16, 16, 16, 16), BLUE)], requirements=("content-stream-array",))
    commands, geometry = [], []
    for y in range(128):
        for x in range(128):
            color = RED if (x + y) % 2 else BLUE
            commands.append(b"%d 0 %d rg %d %d 1 1 re f\n" % (color[0] // 255, color[2] // 255, x, y))
            geometry.append(((x, y, 1, 1), color))
    yield render("many-operands", "114,688 numeric operands on one page; budgets are per object",
                 [b"".join(commands)], geometry, size=128, media_box=b"[0 0 128 128]",
                 requirements=("bounded-resource-use",))
    # Clause 7.9.5: readers must normalize rectangles given by other corners.
    yield render("normalized-media-box", "MediaBox given as upper-right then lower-left corner",
                 [b"0 g 8 8 16 16 re f\n"], [((8, 8, 16, 16), BLACK)], media_box=b"[64 64 0 0]",
                 requirements=("rectangle-normalization",))
    # Clause 7.3.10: a reference to an undefined object is the null object.
    yield render("undefined-reference-is-null", "Resources refers to an object that does not exist",
                 [b"0 g 0 0 64 8 re f\n"], [((0, 0, 64, 8), BLACK)], resources=b"99 0 R",
                 requirements=("undefined-reference-null",))
    yield Case("undefined-contents-is-null", document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents 99 0 R >>",
    ]), RENDER, BASE + ("undefined-reference-null",),
        "Contents refers to an object that does not exist: null, so the page has no content (Table 31)",
        oracle=opaque_rectangles(64, 64, []))
    trailing = render("trailing-bytes", "data after the final %%EOF is a reported repair",
                      [b"0 g 0 0 8 8 re f\n"], [((0, 0, 8, 8), BLACK)], tail=b"garbage\n",
                      requirements=("recovery-reported",))
    yield Case(**{**trailing.__dict__, "repairs": ("file.trailing-bytes",), "strict_valid": False})

    tiling = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << /Pattern << /P 5 0 R >> >> /Contents 4 0 R >>",
        stream(b"/Pattern cs /P scn 0 0 64 64 re f\n"),
        stream(b"0 0 1 1 re f", b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /Matrix [.3 .3 .3 .4 0 0] /BBox [0 0 1 1] /XStep 2 /YStep 2 /Resources << >>"),
    ])
    yield Case("pattern-unsupported", tiling, UNSUPPORTED, BASE + ("unsupported-is-explicit",),
               "Table 74 constant-spacing fitting collapses this oblique device lattice; unsupported explicitly", feature="pattern.tiling", oracle=None)
    yield Case("text-undefined-font", single_page([b"BT /F1 12 Tf (hello) Tj ET\n"]), OPEN, BASE,
               "a font name that is not a page resource is malformed content (7.8.3)",
               extra={"render_error": "Syntax"})
    yield render("alpha-unsupported", "Constant half opacity over white (11.3.3)",
                      [b"/GS1 gs 0 0 64 64 re f\n"], [((0,0,64,64),(128,128,128))], resources=b"<< /ExtGState << /GS1 << /ca 0.5 >> >> >>")
    from .content import cie_rgb
    yield render("colorspace-unsupported", "CalRGB default-space gamma/matrix to XYZ (8.6.5.3)",
                 [b"1 0 0 rg 0 0 64 64 re f\n"], [((0,0,64,64),cie_rgb('CalRGB',(1,0,0),white=(.9505,1,1.089)))],
                 resources=b"<< /ColorSpace << /DefaultRGB [/CalRGB << /WhitePoint [0.9505 1 1.089] >>] >> >>")
    yield Case("fractional-rectangle", single_page([b"0 g 0.5 0.5 20 20 re f\n"]), RENDER,
               BASE + ("opaque-rectangle-rendering", "content-fractional"),
               "edges inside pixels: coverage is the product of overlaps",
               oracle=fractional_oracle(), extra={"tolerance": 1})
    yield render("rotate-90-blank", "rotation with empty content gives a blank page", [b""], [],
                 page_extra=b"/Rotate 90 ")
    yield render("opaque-page-group-blank", "an opaque page group with empty content renders like no group",
                 [b""], [], page_extra=b"/Group << /S /Transparency >> ")
    # Two pages with distinguishable content, for navigation tests.
    yield Case("two-pages", two_pages(), RENDER, BASE + ("opaque-rectangle-rendering",),
               "two pages; the oracle is page 1 (page 2 is a blue square at 32 32)",
               oracle=opaque_rectangles(64, 64, [((8, 8, 16, 16), RED)]), pages=2,
               extra={"page_oracles": {"1": "page 2: blue 32x32 square at (32, 32)"}})
    yield Case("rotate-not-multiple-of-90", single_page([b""], page_extra=b"/Rotate 45 "), INVALID,
               BASE, "Rotate must be a multiple of 90 (Table 31)", pages=None)


def two_pages() -> bytes:
    """Catalog, Pages with two kids, and one content stream per page."""
    page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents %d 0 R >>"
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        page % 5,
        page % 6,
        stream(b"1 0 0 rg 8 8 16 16 re f\n"),
        stream(b"0 0 1 rg 32 32 32 32 re f\n"),
    ])


def fractional_oracle() -> bytes:
    """Black rectangle [0.5, 20.5]^2 over white at 72 dpi: each pixel's
    coverage is the product of its horizontal and vertical overlaps."""
    from fractions import Fraction
    size, low, high = 64, Fraction(1, 2), Fraction(41, 2)

    def overlap(start: int) -> Fraction | int:
        return max(Fraction(0), min(high, start + 1) - max(low, start))

    rows = []
    for top in range(size):
        y = size - 1 - top
        rows.append(bytes(v for x in range(size) for v in [round(255 * (1 - overlap(x) * overlap(y)))] * 3))
    return b"P6\n64 64\n255\n" + b"".join(rows)
