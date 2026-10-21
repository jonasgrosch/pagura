"""Hosted structural checks; expose baseline reader limitations without hiding regressions."""
import os
import re
import subprocess
from pathlib import Path


def run(*args):
    result = subprocess.run(args, text=True, capture_output=True, check=False)
    print("$ " + " ".join(map(str, args)), flush=True)
    print(result.stdout + result.stderr, flush=True)
    return result


def main():
    root = Path(os.environ["PAGURA_EDIT_OUTPUT"])
    files = sorted(root.glob("*.pdf"))
    assert len(files) >= 10, "missing saved fixture output"
    run("qpdf", "--version")
    run("pdfinfo", "-v")
    accepted, incompatible = [], []
    for saved in files:
        original = root / "originals" / saved.name
        assert original.is_file(), f"missing independent baseline for {saved.name}"
        for path in (original, saved):
            assert run("qpdf", "--check", path).returncode == 0, f"qpdf rejected {path}"
        before = run("pdfinfo", original)
        after = run("pdfinfo", saved)
        if before.returncode == 0:
            assert after.returncode == 0, f"Poppler regression: {saved.name}"
            def pages(result):
                found = re.search(r"^Pages:\s+(\d+)$", result.stdout, re.MULTILINE)
                assert found, f"pdfinfo printed no page count for {saved.name}"
                return found.group(1)
            assert pages(before) == pages(after), f"page count changed: {saved.name}"
            accepted.append(saved.name)
        elif after.returncode == 0:
            accepted.append(saved.name)
        else:
            incompatible.append(saved.name)
            print(f"BASELINE INCOMPATIBILITY (not a Poppler pass): {saved.name}", flush=True)
    assert accepted, "no saved files accepted by both readers"
    print(f"qpdf accepted all {len(files)} saved files; Poppler accepted {len(accepted)}; "
          f"baseline incompatibilities: {incompatible}", flush=True)


if __name__ == "__main__":
    main()
