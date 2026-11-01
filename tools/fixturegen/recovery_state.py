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
        ('unmatched-Q-suffix', marks + b'Q Q', ('content.graphics-unmatched', 'content.graphics-unmatched')),
        ('unmatched-ET-prefix', b'ET ' + marks, ('content.text-unmatched',)),
        ('unmatched-ET-suffix', marks + b'ET ET', ('content.text-unmatched', 'content.text-unmatched')),
        ('unmatched-Q-in-text', b'BT Q ET ' + marks, ('content.graphics-unmatched',)),
        ('unmatched-ET-in-saved-state', b'q ET Q ' + marks, ('content.text-unmatched',)),
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
    # The valid glyph control has independently stated rectangle geometry.
    font = b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << /A 6 0 R >> /Encoding << /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [1] /Resources << >> >>'
    glyph = stream(b'1 0 0 0 1 1 d1 0 0 1 1 re f')
    yield render('closed-text-glyph-control', b'1 0 0 rg BT /F 4 Tf 1 0 0 1 1 1 Tm (A) Tj ET',
                 [font, glyph], b'<< /Font << /F 5 0 R >> >>', rectangle,
                 requirement='recovery-content-state')
