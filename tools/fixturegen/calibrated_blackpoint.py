"""CIE BlackPoint metadata under Pagura's existing disabled-compensation policy.

ISO 32000-2 Tables 62--64 define nonnegative XYZ metadata without an upper
bound. Their component-to-XYZ formulas do not contain BlackPoint. Clause
8.6.5.9 forbids BPC under OFF and leaves Default to the processor; Pagura's
existing Default disables it. ON remains an explicit unsupported feature.
Independent output comes from the author-side CIE equations and solved
Bradford/sRGB matrices in content.cie_rgb, never a renderer.
"""
from .content import cie_rgb
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import opaque_rectangles, single_page

AREA = 'calibrated_blackpoint'
REQUIREMENT = ('content-cie', 'content-extgstate-extensions')


def space(family, black):
    params = b'/Gamma 2.2' if family == 'CalGray' else b'/Gamma [2.2 2.2 2.2]' if family == 'CalRGB' else b'/Range [-100 100 -100 100]'
    return b'[/' + family.encode() + b' << /WhitePoint [.96422 1 .82521] /BlackPoint ' + black + b' ' + params + b' >>]'


def resource(family, black, compensation=b''):
    return b'<< /ColorSpace << /C ' + space(family, black) + b' >> /ExtGState << /GS << ' + compensation + b' >> >> >>'


def values(family):
    if family == 'CalGray':
        return [(v,) for v in (0, .01, .18, .5, .75, 1, 2, -1)]
    if family == 'CalRGB':
        return [(0, 0, 0), (.1, .2, .3), (.25, .5, .75), (.5, .5, .5), (.8, .2, .4), (1, 1, 1), (2, -.2, 1), (-1, 2, .3)]
    return [(0, 0, 0), (8, 0, 0), (50, 0, 0), (75, 0, 0), (100, 0, 0), (50, 40, -20), (50, -70, 30), (50, 200, -200)]


def oracle(family):
    gamma = 1.0 if family == 'Lab' else 2.2
    colors = [cie_rgb(family, v, gamma=gamma) for v in values(family)]
    return opaque_rectangles(8, 8, [((x, 0, 1, 8), c) for x, c in enumerate(colors)])


def command(family, prefix=b'/GS gs '):
    return prefix + b'/C cs ' + b' '.join((' '.join(map(str, v)) + f' sc {x} 0 1 8 re f').encode() for x, v in enumerate(values(family)))


def cases():
    black_points = [('zero', b'[0 0 0]'), ('neutral', b'[.1 .1 .1]'), ('chromatic', b'[.2 1 1.7]'),
                    ('at-white', b'[.96422 1 .82521]'), ('above-white', b'[50 50 50]'),
                    ('large-finite', b'[' + b'1' + b'0' * 100 + b'.0 0 0]')]
    # PDF numbers have no exponent syntax: construct a finite large value in
    # ordinary decimal notation so this exercises metadata, not lexical repair.
    for family in ('CalGray', 'CalRGB', 'Lab'):
        expected = oracle(family)
        for mode, state in [('implicit', b''), ('off', b'/UseBlackPtComp /OFF'), ('default', b'/UseBlackPtComp /Default')]:
            for name, black in black_points:
                pdf = single_page([command(family)], media_box=b'[0 0 8 8]', resources=resource(family, black, state))
                yield Case(f'{family}-{name}-{mode}', pdf, RENDER, REQUIREMENT,
                           'Validated diffuse-black metadata with disabled BPC; independent gamma/matrix-to-XYZ, Bradford and sRGB equations',
                           oracle=expected, extra={'api': 'compile_page', 'tolerance': 1})
        # Explicit ON still refuses before any paint; valid BlackPoint is not
        # permission to silently substitute OFF for an actual BPC request.
        yield Case(f'{family}-on', single_page([command(family)], media_box=b'[0 0 8 8]', resources=resource(family, b'[.1 .1 .1]', b'/UseBlackPtComp /ON')),
                   UNSUPPORTED, REQUIREMENT, 'Explicit compensation remains unsupported', feature='extgstate.UseBlackPtComp', extra={'api': 'compile_page'})
        for name, black in [('negative', b'[-.1 0 0]'), ('short', b'[0 0]'), ('long', b'[0 0 0 0]'), ('name', b'/Bad'), ('null', b'null'), ('nonnumeric', b'[0 /Bad 0]')]:
            yield Case(f'{family}-{name}', single_page([command(family)], media_box=b'[0 0 8 8]', resources=resource(family, black)),
                       OPEN, REQUIREMENT, 'Malformed BlackPoint remains syntax error', extra={'api': 'compile_page', 'render_error': 'Syntax'})
