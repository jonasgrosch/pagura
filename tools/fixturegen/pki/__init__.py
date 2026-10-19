"""The pki fixture area: independent ASN.1 envelopes and certification paths.

Standard library only; never imports or runs Pagura. `fixtures` writes one DER
or BER value per parsing case. The path generators (`paths`, `graphs`,
`path_inputs`, `crls`, `path_security`) build synthetic certificates and CRLs
for RFC 5280 6 validation; their tab-separated records are stored as text
fixtures that crates/pagura-pki reads. `pkits` and `revocation` read the private
PKITS cache instead and are run by tools/pki_vectors.py.
"""
from __future__ import annotations

import io
from collections.abc import Callable, Iterator
from contextlib import redirect_stdout

from ..model import Vector
from . import crls, fixtures, graphs, path_inputs, path_security, paths

AREA = "pki"

PATHS = ("pki-path-validation",)
REVOCATION = ("pki-path-validation", "pki-crl-revocation")


def records(main: Callable[[], None]) -> bytes:
    """The tab-separated records a path generator prints."""
    output = io.StringIO()
    with redirect_stdout(output):
        main()
    return output.getvalue().encode()


def cases() -> Iterator[Vector]:
    yield from fixtures.cases()
    for name, generator, requirements, description in (
        ("paths", paths.main, PATHS,
         "anchor, issuer and target triples for RFC 5280 6.1 basic checks and RFC 9549/9598 names"),
        ("graphs", graphs.main, PATHS,
         "an issuer pool with shared names, cycles and an unknown matching rule for path building"),
        ("path-inputs", path_inputs.main, PATHS,
         "caller inputs: initial name constraints, denied name forms and conservative name matching"),
        ("crls", crls.main, REVOCATION,
         "CRLs for RFC 5280 6.3 scope, reasons, freshness, delta CRLs and critical extensions"),
        ("path-security", path_security.main, REVOCATION,
         "signed certificates and CRLs whose signer and issuer bindings must not be bypassed"),
    ):
        yield Vector(name, {".txt": records(generator)}, requirements, description, {"structure": "records"})
