"""Independent state and structure controls for unmatched EMC (14.6).

The malformed ending has no open sequence to close. The expected pixels are
explicit authored rectangles; they are not inferred from a repaired renderer.
Forms keep their own marked-content stacks, including hidden inherited state.
"""
from dataclasses import replace
from .model import Case, OPEN
from .pdf import single_page, stream
from .recovery import page, render

AREA = 'recovery_marked'
REQUIREMENT = 'recovery-marked-closure'
TAG = 'content.marked-unmatched'
RED = (255, 0, 0)
BLUE = (0, 0, 255)
RED_MARK = b'1 0 0 rg 1 1 4 4 re f '
BLUE_MARK = b'0 0 1 rg 8 8 4 4 re f '
RED_RECT = [((1, 1, 4, 4), RED)]
BLUE_RECT = [((8, 8, 4, 4), BLUE)]


def painted(name, content, rectangles, repairs=(), **options):
    case = render(name, content, rectangles=rectangles, repairs=repairs,
                  requirement=REQUIREMENT, **options)
    return replace(case, extra={**case.extra, 'independent_rectangles': rectangles})


def cases():
    for name, content, repairs, rectangles in [
        ('balanced-control', b'/Span BMC ' + RED_MARK + b'EMC', (), RED_RECT),
        ('unmatched-prefix', b'EMC ' + RED_MARK, (TAG,), RED_RECT),
        ('unmatched-suffix', RED_MARK + b'EMC EMC', (TAG, TAG), RED_RECT),
        ('repeated-before-paint', b'EMC EMC EMC ' + RED_MARK, (TAG, TAG, TAG), RED_RECT),
        ('within-text', b'BT EMC ET ' + RED_MARK, (TAG,), RED_RECT),
        ('within-save', b'q EMC Q ' + RED_MARK, (TAG,), RED_RECT),
        ('retains-path', b'1 0 0 rg 1 1 4 4 re EMC f', (TAG,), RED_RECT),
        ('retains-pending-clip', b'2 2 4 4 re W EMC n 0 0 1 rg 0 0 8 8 re f',
         (TAG,), [((2, 2, 4, 4), BLUE)]),
        ('retains-state', b'2 2 4 4 re W n 0 0 1 rg 1 0 0 1 1 1 cm EMC 0 0 8 8 re f',
         (TAG,), [((2, 2, 4, 4), BLUE)]),
        ('after-balanced', b'/Span BMC ' + RED_MARK + b'EMC EMC ' + BLUE_MARK,
         (TAG,), RED_RECT + BLUE_RECT),
        ('after-nested', b'/Span BMC /Artifact BMC ' + RED_MARK + b'EMC EMC EMC ' + BLUE_MARK,
         (TAG,), RED_RECT + BLUE_RECT),
    ]:
        yield painted(name, content, rectangles, repairs)

    # Source Type3 /A has a unit-square outline and authoritative glyph name.
    # Four-point text at(1,1) therefore has the literal rectangle above.
    font = b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << /A 6 0 R >> /Encoding << /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [1] /Resources << >> >>'
    glyph = stream(b'1 0 0 0 1 1 d1 0 0 1 1 re f')
    text = b'1 0 0 rg BT /F 4 Tf 1 0 0 1 1 1 Tm (A) Tj ET '
    properties = b'/Span << /MCID 7 /ActualText (replacement) >> BDC '
    yield painted('actual-text', b'EMC ' + properties + text + b'EMC EMC', RED_RECT,
                  (TAG, TAG), objects=[font, glyph], resources=b'<< /Font << /F 5 0 R >> >>')

    form = stream(b'EMC ' + text,
                  b'/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Resources << /Font << /F 5 0 R >> >> ')
    yield painted('form-keeps-caller-marked', properties + b'/Child Do EMC ' + BLUE_MARK,
                  RED_RECT + BLUE_RECT, (TAG,), objects=[font, glyph, form],
                  resources=b'<< /Font << /F 5 0 R >> /XObject << /Child 7 0 R >> >>')

    # A form under hidden marked content inherits false visibility. Its
    # unmatched EMC cannot restore true visibility or pop the caller's scope.
    ocg = b'<< /Type /OCG /Name (hidden) >>'
    hidden_form = stream(b'EMC ' + RED_MARK,
                         b'/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Resources << >> ')
    yield painted('hidden-form-keeps-visibility', b'/OC /Off BDC /Child Do EMC ' + BLUE_MARK,
                  BLUE_RECT, (TAG,), objects=[ocg, hidden_form],
                  resources=b'<< /Properties << /Off 5 0 R >> /XObject << /Child 6 0 R >> >>',
                  catalog=b'/OCProperties << /OCGs [5 0 R] /D << /BaseState /ON /OFF [5 0 R] >> >> ')

    split = [b'/Span BMC ' + RED_MARK, b'EMC EMC ' + BLUE_MARK]
    case = painted('split-content-sequence', b'', RED_RECT + BLUE_RECT, (TAG,))
    yield replace(case, pdf=single_page(split, media_box=b'[0 0 16 16]'))

    # A repair never authorizes a later incomplete operation, unknown resource
    # or recursive required content to produce a successful partial page.
    for name, suffix in [
        ('unfinished-text', b'BT'),
        ('unfinished-path', b'1 1 m'),
        ('unfinished-clip', b'W'),
        ('unfinished-operands', b'1 2'),
        ('unfinished-compatibility', b'BX'),
        ('unknown-operator', b'notAnOperator'),
        ('undefined-resource', b'/Missing Do'),
    ]:
        yield Case(name, page(RED_MARK + b'EMC ' + suffix), OPEN, (REQUIREMENT,),
                   'Later syntax or resource failure remains an error after the reported ending',
                   strict_valid=False, repairs=(TAG,), extra={'render_error': 'Syntax'})

    recursive = stream(b'/Self Do', b'/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Resources << /XObject << /Self 5 0 R >> >>')
    yield Case('required-cycle', page(RED_MARK + b'EMC /Self Do', [recursive],
               resources=b'<< /XObject << /Self 5 0 R >> >>'), OPEN, (REQUIREMENT,),
               'Required recursive form remains a Cycle after the reported ending; no successful partial page',
               strict_valid=False, repairs=(TAG,), extra={'render_error': 'Cycle'})
