"""The fixture record shared by all area generators."""
from __future__ import annotations

from dataclasses import dataclass, field

# Expected outcomes understood by tools/conformance.py.
RENDER = "render"            # renders; output must equal the oracle exactly
UNSUPPORTED = "unsupported"  # opens; rendering fails with the given feature tag
OPEN = "open"                # opens/pages; scan must render unless extra.render_error is set
INVALID = "invalid"          # opening fails even with recovery


@dataclass(frozen=True)
class Case:
    name: str
    pdf: bytes
    expected: str
    requirements: tuple[str, ...]
    description: str
    oracle: bytes | None = None          # binary PPM (P6) for RENDER cases
    dpi: int = 72
    pages: int | None = 1
    feature: str | None = None           # Feature tag for UNSUPPORTED cases
    repairs: tuple[str, ...] = ()        # Repair tags expected in recovery mode
    strict_valid: bool = True            # whether Recovery::Strict also opens it
    extra: dict = field(default_factory=dict)
    information: tuple[str, ...] = ()     # Informational tags in either mode


@dataclass(frozen=True)
class Vector:
    """A non-PDF test vector, e.g. encoded input plus expected decoded output.

    ``files`` maps a file suffix (``".in"``, ``".out"``, ``".pgm"``...) to
    contents; the driver writes ``corpus/generated/<area>/<name><suffix>``.
    """
    name: str
    files: dict[str, bytes]
    requirements: tuple[str, ...]
    description: str
    extra: dict = field(default_factory=dict)
