"""Independent plain-root counterparts of the general precision fixtures.

ISO 32000-2, 11.3.3; project coverage-byte composition contract in ADR 0004.
These PDFs contain solid opaque path marks directly in the page stream. No
knockout Form forces general composition. The exact answers are Fractions,
not renderer output. Accept only bytes within one level of the *unrounded*
answer: tolerance one around the rounded PPM would admit an extra wrong byte.

This is a focused accuracy reproducer, not a representative performance corpus
or proof of continuous geometric accuracy. At 72 DPI, the rounded coverage of
each unit-height rectangle is the declared byte. Alpha must be 255 separately.
"""
from fractions import Fraction as F
from fixturegen.pdf import document, stream

CASES = ("root-two-partial", "root-slivers-1000")


def fixture(name):
    """Return independent PDF, rounded P6 oracle, and exact admissible bounds."""
    if name == "root-two-partial":
        background, source, coverage, count = F(85, 100), F(95, 100), 10, 2
        background_token, source_token = "0.85", "0.95"
    elif name == "root-slivers-1000":
        background, source, coverage, count = F(1), F(0), 1, 1000
        background_token, source_token = "1", "0"
    else:
        raise ValueError("unknown direct-root precision case")
    # A finite decimal lies strictly inside the intended coverage byte's
    # rounding interval; the oracle composes on that discrete byte lattice.
    width = format(coverage / 255, ".12f")
    assert int(F(width) * 255 + F(1, 2)) == coverage
    exact = 255 * (source + (background - source) * F(255 - coverage, 255) ** count)
    rounded = int(exact + F(1, 2))
    admissible = [level for level in range(256) if abs(F(level) - exact) <= 1]
    assert admissible == ([218, 219] if name == CASES[0] else [5, 6])
    content = (background_token + " g 0 0 1 1 re f\n" + source_token + " g\n").encode()
    content += f"0 0 {width} 1 re f\n".encode() * count
    pdf = document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> /Contents 4 0 R >>",
        stream(content),
    ])
    return pdf, b"P6\n1 1\n255\n" + bytes([rounded] * 3), {
        "marks": count + 1,
        "coverage": coverage,
        "width_decimal": width,
        "exact_numerator": str(exact.numerator),
        "exact_denominator": str(exact.denominator),
        "rounded": rounded,
        "admissible": admissible,
        "alpha": 255,
    }
