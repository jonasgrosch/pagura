"""Independent integer-rectangle fixtures for ordinary clip preparation.

Import after the benchmark driver adds the repository's tools directory to
sys.path, as benchmarks/scene_index.py already does. This module does not run
the renderer or write files. fixture(name) has that driver's (PDF, P6) API.

Render contract: page zero, 72 DPI, full validation through compile_page, then
DisplayList::render/render_tile under either accuracy mode, opaque white
background, default overprint policy. The caller separately checks every raw
RGBA alpha byte against EXPECTED_ALPHA; a PPM alone cannot establish alpha.

Derivation (ISO 32000-2, 8.5.4, 11.3.3, 11.6.5.1): distinct positive-area
integer clips intersect larger integer marks. At 72 DPI each covered pixel
belongs to one mark, with no fractional coverage or overlapping composition.
Quarter-opacity black over white has channel 255*(1-1/4)=191.25, rounding to
191. Opaque black gives zero. An isolated Alpha soft-mask group starts fully
transparent; its opaque white marks give mask alpha one inside and zero
outside, so an opaque black page mark gives the same binary black/white image.
The page backdrop is opaque throughout, hence output alpha is always 255.

These are synthetic mechanism/control fixtures, not a representative corpus
or a release-performance claim. No measured output sets the oracle or bound.
"""
from fixturegen.pdf import document, opaque_rectangles, stream

CASES = (
    "clip-churn", "clip-shared", "clip-soft-source",
    "clip-opaque", "clip-tiny", "clip-dense",
)
EXPECTED_ALPHA = 255
DPI = 72


def _geometry(name):
    if name == "clip-tiny":
        return 64, [(24, 24, 8, 2)]
    if name == "clip-dense":
        # Four independent, nearly full-area marks: a pixel-dominated control,
        # not an all-overlap scene. Painted area is 4*254**2 of 512**2 pixels.
        return 512, [(x, y, 256, 1) for y in (0, 256) for x in (0, 256)]
    # 4096 independently scoped hard clips. q/Q prevents a growing clip chain.
    return 512, [(x, y, 8, 2) for y in range(0, 512, 8) for x in range(0, 512, 8)]


def _rectangle(x, y, width, height):
    return f"{x} {y} {width} {height} re ".encode("ascii")


def _marks(size, geometry, *, shared=False, white=False):
    content = bytearray(b"1 g\n" if white else b"0 g\n")
    if shared:
        content.extend(_rectangle(0, 0, size, size) + b"W n\n")
    for x, y, side, inset in geometry:
        inner = _rectangle(x + inset, y + inset, side - 2 * inset, side - 2 * inset)
        if shared:
            # Same expected pixels and command count, one shared clip identity.
            content.extend(inner + b"f\n")
        else:
            content.extend(b"q " + inner + b"W n "
                           + _rectangle(x, y, side, side) + b"f Q\n")
    return bytes(content)


def fixture(name):
    """Return independent PDF and exact RGB8 P6 bytes for a declared case."""
    if name not in CASES:
        raise ValueError("unknown clip-viewport fixture")
    size, geometry = _geometry(name)
    if name == "clip-soft-source":
        # Object 5 is evaluated in the compositor's protected soft_source path.
        mask = stream(
            _marks(size, geometry, white=True),
            b"/Type /XObject /Subtype /Form /FormType 1 "
            + b"/BBox [0 0 %d %d] " % (size, size)
            + b"/Resources << >> /Group << /S /Transparency /CS /DeviceGray /I true /K false >> ",
        )
        resources = b"<< /ExtGState << /Paint << /ca 1 /SMask << /S /Alpha /G 5 0 R >> >> >> >>"
        content = b"/Paint gs 0 g " + _rectangle(0, 0, size, size) + b"f\n"
        extras, gray = [mask], 0
    else:
        opaque = name == "clip-opaque"
        alpha = b"1" if opaque else b".25"
        resources = b"<< /ExtGState << /Paint << /ca " + alpha + b" >> >> >>"
        content = b"/Paint gs\n" + _marks(size, geometry, shared=name == "clip-shared")
        extras, gray = [], 0 if opaque else 191
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] " % (size, size)
        + b"/Resources " + resources + b" /Contents 4 0 R >>",
        stream(content), *extras,
    ])
    rectangles = [
        ((x + inset, y + inset, side - 2 * inset, side - 2 * inset), (gray,) * 3)
        for x, y, side, inset in geometry
    ]
    return pdf, opaque_rectangles(size, size, rectangles)
