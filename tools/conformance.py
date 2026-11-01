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
import json
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
    recovery = case.get("scan_recovery")
    if recovery not in (None, "repair"):
        raise AssertionError(f"unknown scan_recovery: {recovery!r}")
    result = pagura("scan", *(["--repair"] if recovery else []),
                    *password_args(case), str(ROOT / case["path"]),
                    *([str(case.get("dpi", 72))] if recovery else []))
    if result.returncode != 0:
        raise AssertionError(f"scan crashed with exit {result.returncode}: {result.stderr.decode(errors='replace')[-500:]}")
    report = json.loads(result.stdout)
    if recovery == "repair":
        if report.get("schema") != 2 or report.get("recovery") != "repair" or report.get("strict", False) is not None:
            raise AssertionError("explicit Repair scan requires schema 2, recovery=repair and strict=null")
    elif report.get("schema") != 1 or type(report.get("strict")) is not bool or "recovery" in report:
        raise AssertionError("default scan requires schema 1 and a boolean Strict audit")
    return report


def check_strict_render(case: dict) -> list[str]:
    """Run an actual Strict render for an explicit-Repair scan fixture.

    Scan JSON supports only kind/feature/repair/message. Full error positions
    and object identity remain public-API/parity contracts.
    """
    contract = case.get("mode_errors", {}).get("strict")
    succeeds = case.get("strict_valid", True) and case["expected"] == "render"
    problems = []
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / "strict.ppm"
        result = pagura("render", "--strict", *password_args(case),
                        str(ROOT / case["path"]), str(output), str(case.get("dpi", 72)), str(case.get("page", 0)))
        if succeeds:
            if result.returncode != 0:
                return [f"Strict render failed: {result.stderr.decode(errors='replace').strip()}"]
            problem = compare_images(output.read_bytes(), (ROOT / case["oracle"]).read_bytes(), case.get("tolerance", 0))
            return [f"Strict {problem}"] if problem else []
        if result.returncode != 1 or output.exists():
            problems.append("Strict refusal must exit 1 without an output file")
    if contract:
        message = result.stderr.decode(errors="replace")
        if contract.get("message") and contract["message"] not in message:
            problems.append(f"Strict error omitted declared message: {message.strip()}")
        if contract.get("repair") is None:
            # An untagged original rendering error cannot trigger default
            # scan's tagged-repair retry. That report exposes these fields.
            default = scan({key: value for key, value in case.items() if key != "scan_recovery"})
            errors = [default["open"]] if default["open"] else [failure["error"] for failure in default.get("failures", [])
                                                                          if failure["page"] in (None, case.get("page", 0))]
            fields = {key: ({"MissingObject": "missing-object", "InvalidArgument": "invalid-argument"}.get(value, value.lower())
                            if key == "kind" else value) for key, value in contract.items()
                      if key in {"kind", "feature", "repair", "message"}}
            if not any(all(error.get(key) == value for key, value in fields.items()) for error in errors):
                problems.append(f"Strict structured error did not match {fields}: {errors}")
        elif contract["repair"] not in message:
            problems.append("Strict refusal omitted its declared repair tag")
    elif not case.get("strict_valid", True):
        problems.append("explicit Repair fixtures with invalid Strict rendering must declare mode_errors.strict")
    return problems


def check(case: dict) -> list[str]:
    """Problems with one case; empty when it meets its expectation."""
    problems = []
    report = scan(case)
    if case.get("scan_recovery") == "repair":
        problems.extend(check_strict_render(case))
    expected = case["expected"]
    if expected == "invalid":
        if report["open"] is None and report["pages"] is not None:
            return [*problems, "opened, but the case is invalid"]
        wanted = case.get("error")
        actual = (report["open"] or {}).get("kind") or (report["failures"][0]["error"]["kind"] if report.get("failures") else None)
        if wanted and actual != wanted:
            problems.append(f"error kind {actual}, expected {wanted}")
        return problems
    if report["open"] is not None:
        return [*problems, f"failed to open: {report['open']}"]
    if case.get("pages") is not None and report["pages"] != case["pages"]:
        problems.append(f"{report['pages']} pages, expected {case['pages']}")
    if sorted(report["repairs"]) != sorted(case.get("repairs", [])):
        problems.append(f"repairs {report['repairs']}, expected {case.get('repairs', [])}")
    if sorted(report.get("information", [])) != sorted(case.get("information", [])):
        problems.append(f"information {report.get('information', [])}, expected {case.get('information', [])}")
    if case.get("scan_recovery") != "repair" and report["strict"] != case.get("strict_valid", True):
        problems.append(f"strict open {'succeeded' if report['strict'] else 'failed'}, expected the opposite")
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
                            str(case.get("dpi", 72)),
                            *([str(case.get("page", 0))] if case.get("scan_recovery") == "repair" else []))
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
