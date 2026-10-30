"""Independent rectangles for bounded content-state recovery (8.4.2, 8.5, 9.4.1).

The expected pixels are explicit rectangles, not a second interpretation of the
malformed operators. Every suffix repair has a well-formed control with the
same marks; only the marked deviations change the input.
"""
from .recovery import render
from .pdf import stream

AREA = 'recovery_state'
RED = (255, 0, 0)
BLUE = (0, 0, 255)


def cases():
    marks = b'1 0 0 rg 1 1 4 4 re f '
    rectangle = [((1, 1, 4, 4), RED)]
    for name, content, repairs in [
        ('unmatched-Q-prefix', b'Q ' + marks, ('content.graphics-unmatched',)),
        ('unmatched-Q-suffix', marks + b'Q Q', ('content.graphics-unmatched',)),
        ('unmatched-ET-prefix', b'ET ' + marks, ('content.text-unmatched',)),
        ('unmatched-ET-suffix', marks + b'ET ET', ('content.text-unmatched',)),
        ('unmatched-Q-in-text', b'BT Q ET ' + marks, ('content.graphics-unmatched',)),
        ('unmatched-ET-in-saved-state', b'q ET Q ' + marks, ('content.text-unmatched',)),
        ('unclosed-empty-text', marks + b'BT', ('content.text-unclosed',)),
        ('unclosed-knockout-text', marks + b'/G gs BT', ('content.text-unclosed',)),
        ('unpainted-path', marks + b'8 8 4 4 re', ('content.path-unpainted',)),
        ('unpainted-clipped-path', marks + b'8 8 4 4 re W', ('content.path-unpainted',)),
        ('unpainted-empty-clip', marks + b'W', ('content.path-unpainted',)),
        ('balanced-control', marks + b'BT ET 8 8 4 4 re n', ()),
    ]:
        resources = b'<< /ExtGState << /G << /TK true >> >> >>'
        yield render(name, content, resources=resources, rectangles=rectangle,
                     repairs=repairs, requirement='recovery-content-state')
    # Unmatched closures must retain ink, transform and clip. The blue
    # rectangle is clipped by the independently stated [2,2,4,4] square.
    yield render('unmatched-retains-state',
                 b'2 2 4 4 re W n 0 0 1 rg 1 0 0 1 1 1 cm Q ET 0 0 8 8 re f',
                 rectangles=[((2, 2, 4, 4), BLUE)],
                 repairs=('content.graphics-unmatched', 'content.text-unmatched'),
                 requirement='recovery-content-state')
    # A missing ET must close the existing Type3 glyph clip; the explicit
    # glyph rectangle gives an independent one-byte-free raster oracle.
    font = b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << /A 6 0 R >> /Encoding << /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [1] /Resources << >> >>'
    glyph = stream(b'1 0 0 0 1 1 d1 0 0 1 1 re f')
    for suffix, repairs in [(b'', ('content.text-unclosed',)), (b' ET', ())]:
        name = 'unclosed-text-glyph' if not suffix else 'closed-text-glyph-control'
        yield render(name, b'1 0 0 rg BT /F 4 Tf 1 0 0 1 1 1 Tm (A) Tj' + suffix,
                     [font, glyph], b'<< /Font << /F 5 0 R >> >>', rectangle,
                     repairs=repairs, requirement='recovery-content-state')
