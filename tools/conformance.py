"""Check every generated PDF case against its independent expectation.

Runs the release `pagura` binary over corpus/manifests/*.json:
  render       pixels equal the oracle within the case's tolerance; repairs
               and strict validity match
  unsupported  rendering fails with the case's Feature tag
  open         opens with the expected page count and repairs; with
               `render_error`, rendering then fails with that error kind
  invalid      opening fails (with the expected error kind, if given)

"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import subprocess
import sys
import tempfile
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = ROOT / "corpus/manifests"
BINARY = ROOT / "target/release/pagura"


def pagura(*args: str, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run([str(BINARY), *args], capture_output=True, timeout=timeout, cwd=ROOT)


def password_args(case: dict) -> list[str]:
    return ["--password", case["password"]] if case.get("password") else []


def scan(case: dict) -> dict:
    dpi = [str(case.get("dpi", 72))] if "page_outcomes" in case else []
    result = pagura("scan", *password_args(case), str(ROOT / case["path"]), *dpi)
    if result.returncode != 0:
        raise AssertionError(f"scan crashed with exit {result.returncode}: {result.stderr.decode(errors='replace')[-500:]}")
    return json.loads(result.stdout)


def check(case: dict) -> list[str]:
    """Problems with one case; empty when it meets its expectation."""
    problems = []
    if "page_outcomes" in case:
        page_outcomes(case)
    report = scan(case)
    expected = case["expected"]
    if expected == "invalid":
        if report["open"] is None and report["pages"] is not None:
            return ["opened, but the case is invalid"]
        wanted = case.get("error")
        actual = (report["open"] or {}).get("kind") or (report["failures"][0]["error"]["kind"] if report.get("failures") else None)
        if wanted and actual != wanted:
            problems.append(f"error kind {actual}, expected {wanted}")
        return problems
    if report["open"] is not None:
        return [f"failed to open: {report['open']}"]
    if case.get("pages") is not None and report["pages"] != case["pages"]:
        problems.append(f"{report['pages']} pages, expected {case['pages']}")
    if sorted(report["repairs"]) != sorted(case.get("repairs", [])):
        problems.append(f"repairs {report['repairs']}, expected {case.get('repairs', [])}")
    if sorted(report.get("information", [])) != sorted(case.get("information", [])):
        problems.append(f"information {report.get('information', [])}, expected {case.get('information', [])}")
    if report["strict"] != case.get("strict_valid", True):
        problems.append(f"strict open {'succeeded' if report['strict'] else 'failed'}, expected the opposite")
    if "page_outcomes" in case:
        return problems + check_page_outcomes(case, report)
    if expected == "unsupported":
        features = [failure["error"]["feature"] for failure in report["failures"]]
        if case["feature"] not in features:
            problems.append(f"feature tags {features}, expected {case['feature']}")
    elif expected == "render":
        if report["failures"]:
            problems.append(f"render failures {report['failures']}")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "page.ppm"
            result = pagura("render", *password_args(case), str(ROOT / case["path"]), str(output),
                            str(case.get("dpi", 72)))
            if result.returncode != 0:
                problems.append(f"render failed: {result.stderr.decode(errors='replace').strip()}")
            else:
                problem = compare_images(output.read_bytes(), (ROOT / case["oracle"]).read_bytes(),
                                         case.get("tolerance", 0))
                if problem:
                    problems.append(problem)
    elif expected == "open":
        wanted = case.get("render_error")
        kinds = [failure["error"]["kind"] for failure in report["failures"]]
        if wanted and wanted.lower() not in kinds:
            problems.append(f"render errors {kinds}, expected {wanted.lower()}")
        if not wanted and report["failures"]:
            problems.append(f"unexpected render failures {report['failures']}")
    else:
        problems.append(f"unknown expectation {expected}")
    return problems



def page_outcomes(case: dict) -> list[dict]:
    """The full, explicit page contract, or an assertion for malformed metadata."""
    pages = case.get("pages")
    assert type(pages) is int and pages > 0, "page outcomes require a known positive page count"
    assert case["expected"] in ("open", "render"), "page outcomes require open or render"
    outcomes = case["page_outcomes"]
    assert isinstance(outcomes, list) and len(outcomes) == pages, "incomplete page outcome coverage"
    seen = set()
    for row in outcomes:
        assert isinstance(row, dict), "page outcome must be an object"
        page = row.get("page")
        assert type(page) is int and 0 <= page < pages and page not in seen, "invalid or duplicate page index"
        seen.add(page)
        if row.get("expected") == "render":
            assert set(row) == {"page", "expected", "oracle", "oracle_sha256", "width", "height", "tolerance"}, "invalid render page fields"
            assert isinstance(row["oracle"], str) and row["oracle"], "missing oracle path"
            assert isinstance(row["oracle_sha256"], str) and re.fullmatch("[0-9a-f]{64}", row["oracle_sha256"]), "invalid oracle hash"
            for field in ("width", "height"):
                assert type(row[field]) is int and row[field] > 0, "invalid oracle dimensions"
            assert type(row["tolerance"]) is int and 0 <= row["tolerance"] <= 255, "invalid oracle tolerance"
        elif row.get("expected") == "refused":
            assert set(row) == {"page", "expected", "error"}, "invalid refusal page fields"
            error = row["error"]
            assert isinstance(error, dict) and set(error) == {"kind", "message", "feature", "repair"}, "incomplete error contract"
            assert all(isinstance(error[k], str) and error[k] for k in ("kind", "message")), "invalid error identity"
            assert all(error[k] is None or isinstance(error[k], str) and error[k] for k in ("feature", "repair")), "invalid error tag"
            assert case["expected"] == "open", "a render document cannot contain refused pages"
        else:
            raise AssertionError("unknown page outcome")
    assert seen == set(range(pages)), "incomplete page outcome coverage"
    dpi = case.get("dpi", 72)
    assert type(dpi) in (int, float) and math.isfinite(dpi) and dpi > 0, "invalid page-contract DPI"
    return sorted(outcomes, key=lambda row: row["page"])


def page_report_problems(case: dict, report: dict) -> list[str]:
    """Match every structured scan failure to exactly one contracted page."""
    outcomes = page_outcomes(case)
    problems = []
    if type(report.get("pages")) is not int or report["pages"] != case["pages"]:
        problems.append("page contract scan count mismatch")
    expected = {row["page"]: row["error"] for row in outcomes if row["expected"] == "refused"}
    failures = report.get("failures")
    if not isinstance(failures, list):
        return problems + ["missing scan failures list"]
    actual = {}
    for failure in failures:
        if not isinstance(failure, dict) or set(failure) != {"page", "error"}:
            problems.append("malformed scan failure")
            continue
        page = failure["page"]
        if type(page) is not int or not 0 <= page < case["pages"] or page in actual:
            problems.append("unexpected, duplicate or out-of-range scan failure")
            continue
        actual[page] = failure["error"]
    if actual != expected:
        problems.append(f"per-page scan failures {actual}, expected {expected}")
    rendered = sum(row["expected"] == "render" for row in outcomes)
    if type(report.get("rendered")) is not int or report["rendered"] != rendered:
        problems.append(f"scan rendered {report.get('rendered')}, expected {rendered}")
    return problems


def check_page_outcomes(case: dict, report: dict) -> list[str]:
    """Full independent positive pixels and no-output negative page checks."""
    outcomes = page_outcomes(case)
    problems = page_report_problems(case, report)
    for row in outcomes:
        page = row["page"]
        oracle = None
        if row["expected"] == "render":
            path = ROOT / row["oracle"]
            assert path.resolve().is_relative_to(ROOT.resolve()), "oracle outside repository"
            assert path.is_file(), "missing page oracle"
            oracle = path.read_bytes()
            assert hashlib.sha256(oracle).hexdigest() == row["oracle_sha256"], "oracle hash mismatch"
            assert parse_ppm(oracle)[0] == (row["width"], row["height"], 255), "oracle dimension mismatch"
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "page.ppm"
            result = pagura("render", *password_args(case), str(ROOT / case["path"]), str(output),
                            str(case.get("dpi", 72)), str(page))
            if row["expected"] == "render":
                if result.returncode != 0 or not output.is_file():
                    problems.append(f"page {page} render failed: {result.stderr.decode(errors='replace').strip()}")
                else:
                    assert oracle is not None
                    problem = compare_images(output.read_bytes(), oracle, row["tolerance"])
                    if problem:
                        problems.append(f"page {page}: {problem}")
            else:
                if result.returncode != 1:
                    problems.append(f"page {page} refusal exit {result.returncode}, expected 1")
                if output.exists():
                    problems.append(f"page {page} refusal created partial output")
                if row["error"]["message"] not in result.stderr.decode(errors="replace"):
                    problems.append(f"page {page} refusal message mismatch")
    return problems


def parse_ppm(data: bytes) -> tuple[tuple[int, int, int], bytes]:
    """Splits a binary PPM (P6) into (width, height, maxval) and pixel bytes.

    The header is four whitespace-separated tokens (comments are not used by
    Pagura's oracles) followed by exactly one whitespace byte; pixel bytes
    may themselves be whitespace values, so they are never tokenized.
    """
    tokens, position = [], 0
    while len(tokens) < 4:
        while position < len(data) and data[position] in b" \t\r\n\x0b\x0c":
            position += 1
        start = position
        while position < len(data) and data[position] not in b" \t\r\n\x0b\x0c":
            position += 1
        if start == position:
            raise ValueError("truncated PPM header")
        tokens.append(data[start:position])
    if tokens[0] != b"P6" or position >= len(data):
        raise ValueError("not a binary PPM")
    width, height, maxval = (int(token) for token in tokens[1:])
    return (width, height, maxval), data[position + 1:]


def compare_images(actual: bytes, expected: bytes, tolerance: int) -> str | None:
    """None when two binary PPMs match within `tolerance` per channel."""
    try:
        (actual_header, actual_pixels), (expected_header, expected_pixels) = parse_ppm(actual), parse_ppm(expected)
    except ValueError as error:
        return f"invalid image: {error}"
    if actual_header != expected_header:
        return f"image header {actual_header} differs from the oracle's {expected_header}"
    width, height, _ = expected_header
    if len(actual_pixels) != len(expected_pixels) or len(expected_pixels) != width * height * 3:
        return "image payload length differs from its header"
    worst = max((abs(a - b) for a, b in zip(actual_pixels, expected_pixels)), default=0)
    if worst > tolerance:
        return f"pixels differ from the oracle by up to {worst} (tolerance {tolerance})"
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if not arguments.no_build:
        subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "-q", "--bin", "pagura"], cwd=ROOT, check=True)
    cases = [case for path in sorted(MANIFESTS.glob("*.json")) for case in json.loads(path.read_text())["cases"]
             if case["kind"] == "pdf"]

    def run(case: dict) -> tuple[dict, list[str]]:
        try:
            return case, check(case)
        except (AssertionError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
            return case, [str(error)]

    with ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        results = list(pool.map(run, cases))
    outcome = Counter()
    for case, problems in results:
        if problems:
            outcome["fail"] += 1
            print(f"FAIL {case['id']}: {'; '.join(problems)}")
        else:
            outcome["pass"] += 1
    print(f"Conformance: {outcome['pass']} passed, {outcome['fail']} failed of {len(cases)} cases")
    return 1 if outcome["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
