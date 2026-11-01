"""Explicit curved-stroke exclusions, preserving original independent bounds.

This focused annotation extraction excludes the separate source-curve stroke
work from PR #256. A refused appearance never silently paints approximate
geometry. Reference vectors retain their unchanged independent oracle bytes and
tolerance for that follow-up; they are not accuracy passes for a refusal.
"""
from dataclasses import replace
from .model import Case, Vector, UNSUPPORTED


def refused(case):
    assert isinstance(case, Case) and case.oracle is not None
    bound = Vector(case.name+'-accuracy-bound', {'.ppm': case.oracle}, case.requirements,
                   'Unchanged independent accuracy bound for explicitly deferred curved-stroke support',
                   extra={'deferred_case': case.name, 'tolerance': case.extra.get('tolerance', 0),
                          'dpi': case.dpi, 'qualification': 'accuracy follow-up; current rendering refuses'})
    return replace(case, expected=UNSUPPORTED, oracle=None, feature='annotation.appearance',
                   description=case.description+'; current focused extraction explicitly refuses this curved stroke'), bound
