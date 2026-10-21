#!/usr/bin/env python3
"""Archive final integration receipts and their bytes; never run product code.

This is a retrospective evidence collector, not a test runner or proof that
unbound local helper files had these same bytes at execution time.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import stat

LOCAL = Path("benchmarks/results/local/stack-resource-integration")
BASE = "bc2add8ed3e0bea129b66d2868c22bd72ef7d5dc"
ATTEMPTS = (
    ("full-check-001", "full_check.py", "full-check-001.log", "failed", 130),
    ("stroke-check-001", "stroke_check.py", "stroke-check-001.log", "completed", 0),
    ("reproducer-001", "reproduce.py", "reproducer-001.json", "failed", 1),
    ("reproducer-002", "reproduce_resume.py", "reproducer-002.json", "completed", 0),
    ("fixed-001", "verify_fix.py", "fixed-001.json", "completed", 0),
    ("clippy-001", "clippy.py", "clippy-001.log", "failed", 1),
    ("full-check-002", "full_check_v2.py", "full-check-002.json", None, None),
)
HELPERS = [str(LOCAL / row[1]) for row in ATTEMPTS] + [
    str(LOCAL / "run_wasi_test.mjs"), str(LOCAL / "run_wasi_tests.mjs"),
    "benchmarks/results/local/glyph-memo/idle_then.py",
    "benchmarks/results/local/glyph-memo/load_observer.py",
    "tools/runner.py", "tools/check.py",
]
EXTRAS = ["final-inputs.json", "full-check-001-stop-reason.txt",
          "imported-before-glyph-fix.patch", "glyph-budget.patch",
          "product-only.patch", "tests-only.patch", "glyphs-reproducer.rs"]
PER_FILE = 64 * 1024 * 1024
TOTAL = 256 * 1024 * 1024


def read_regular(path):
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError(f"not a regular file: {path}")
        raw = stream.read(PER_FILE + 1)
    if len(raw) > PER_FILE:
        raise ValueError(f"file exceeds archival limit: {path}")
    return raw


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def collect(root, output, allow_pending):
    root = root.resolve(strict=True)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "created_at": datetime.now(timezone.utc).isoformat(),
              "root": str(root), "base_commit": BASE, "status": "incomplete",
              "final_validation": "pending", "attempts": [], "files": {},
              "errors": [], "warnings": [], "performance_claim": False,
              "helper_archive_scope": "Current retained helper bytes copied exactly. Local helpers were outside source_digest; retrospective hashes alone do not prove execution-time bytes.",
              "receipt_rule": "Only finalized outer receipts are authoritative. Same-run internal receipt pins are recorded as nonauthoritative running snapshots."}
    held = 0

    def retain(path, expected=None):
        nonlocal held
        path = path.resolve(strict=True)
        require(path.is_relative_to(root), "evidence source escapes checkout")
        relative = path.relative_to(root).as_posix()
        raw = read_regular(path)
        digest = sha(raw)
        require(expected is None or digest == expected, f"changed artifact: {relative}")
        if relative in report["files"]:
            require(report["files"][relative]["sha256"] == digest, f"source changed while archiving: {relative}")
        else:
            require(held + len(raw) <= TOTAL, "archive exceeds total byte limit")
            destination = output / "archive" / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("xb") as stream:
                stream.write(raw)
            report["files"][relative] = {"sha256": digest, "bytes": len(raw),
                                          "archive": destination.relative_to(output).as_posix()}
            held += len(raw)
        return raw

    for name, helper, result_name, expected_status, expected_code in ATTEMPTS:
        row = {"id": name, "status": "invalid-evidence"}
        report["attempts"].append(row)
        try:
            receipt_path = root / LOCAL / f"{name}-receipt.json"
            initial = read_regular(receipt_path)
            receipt = json.loads(initial)
            require(type(receipt) is dict and type(receipt.get("schema")) is int and receipt["schema"] == 1,
                    "invalid runner receipt schema")
            row.update(reported_runner_status=receipt.get("status"),
                       reported_exit_code=receipt.get("exit_code"),
                       reported_wrapper_exit_code=receipt.get("wrapper_exit_code"))
            if receipt.get("status") == "running":
                require(name == "full-check-002" and allow_pending, "unfinished outer receipt")
                row.update(status="pending", run_id=receipt.get("run_id"),
                           note="No live receipt or live result bytes archived or given final hashes.")
                continue
            require(receipt.get("status") in ("completed", "failed", "cancelled"), "unknown terminal receipt status")
            require(receipt.get("finalized_at") and receipt.get("ended_at"), "receipt not finalized")
            require(type(receipt.get("exit_code")) is int and type(receipt.get("wrapper_exit_code")) is int,
                    "missing final exit codes")
            require(receipt["source_root"] == str(root) and receipt["cwd"] == str(root), "wrong source root")
            require(receipt["source_before"]["commit"] == BASE, "unexpected integration base")
            require(receipt.get("source_unchanged") is True and receipt["source_before"] == receipt["source_after"],
                    "source changed during attempt")
            require(not receipt.get("errors") and not receipt.get("artifact_errors"), "runner reported evidence errors")
            expected_argv = ["python3", "benchmarks/results/local/glyph-memo/idle_then.py",
                             str(LOCAL / f"{name}-idle.json"), "python3",
                             "benchmarks/results/local/glyph-memo/load_observer.py", "--evidence-dir",
                             str(LOCAL / f"{name}-observer"), "--", "python3", str(LOCAL / helper)]
            require(receipt.get("argv") == expected_argv, "unexpected runner argv")
            if expected_status is not None:
                require(receipt["status"] == expected_status and receipt["exit_code"] == expected_code
                        and receipt["wrapper_exit_code"] == expected_code, "historical outcome changed")
            require(retain(receipt_path) == initial, "receipt changed while archiving")
            row.update(status=receipt["status"], run_id=receipt["run_id"],
                       exit_code=receipt["exit_code"], wrapper_exit_code=receipt["wrapper_exit_code"],
                       receipt=str(LOCAL / receipt_path.name), final_receipt_sha256=sha(initial),
                       source=receipt["source_before"], started_at=receipt["started_at"], ended_at=receipt["ended_at"])
            result_path = root / LOCAL / result_name
            guard_path = root / LOCAL / f"{name}-observer/guard.json"
            artifacts = receipt["artifacts"]
            require(type(artifacts) is dict and {str(result_path), str(guard_path)} <= set(artifacts),
                    "missing required outer receipt artifacts")
            for artifact, digest in artifacts.items():
                retain(Path(artifact), digest)
            guard = json.loads(retain(guard_path))
            require(guard.get("exit_code") == receipt["exit_code"], "guard exit differs from receipt")
            require(guard.get("status") != "running", "unfinished guard record")
            row["guard_status"] = guard["status"]
            row["observed_contention"] = any(event.get("matches") for event in guard.get("events", []))
            row["guard_error"] = guard.get("error")
            for event in guard.get("events", []):
                snapshot = event.get("snapshot")
                require(type(snapshot) is str and Path(snapshot).name == snapshot, "invalid process snapshot path")
                retain(guard_path.parent / snapshot)
            retain(root / LOCAL / f"{name}-idle.json")
            if result_path.suffix == ".json":
                result = json.loads(retain(result_path))
                row["result_status"] = result.get("status")
                row["child_exit_codes"] = result.get("exit_codes")
                row["internal_running_receipt_pins"] = []
                for basename, digest in result.get("sha256", {}).items():
                    require(Path(basename).name == basename, "invalid result artifact path")
                    target = root / LOCAL / basename
                    if basename == f"{name}-receipt.json":
                        # This file necessarily changed when the outer wrapper
                        # finalized. Keep the original claim; never rewrite it.
                        actual = sha(initial)
                        row["internal_running_receipt_pins"].append({"path": basename,
                            "claimed_sha256": digest, "final_sha256": actual,
                            "matches_final": digest == actual, "authoritative": False})
                    else:
                        retain(target, digest)
                if name == "full-check-002":
                    final_pass = (receipt["status"] == "completed" and receipt["exit_code"] == 0
                        and receipt["wrapper_exit_code"] == 0
                        and result.get("status") == "passed-full-check-and-wasi-regressions"
                        and result.get("exit_codes") == {"full": 0, "wasm-build": 0, "wasm-run": 0}
                        and result.get("inputs_unchanged") is True
                        and result.get("inputs_sha256") == sha(retain(root / LOCAL / "final-inputs.json"))
                        and not row["observed_contention"] and not row["guard_error"])
                    report["final_validation"] = "passed" if final_pass else "not-passed"
        except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
            row.update(status="invalid-evidence", error=f"{type(error).__name__}: {error}")
            report["errors"].append(f"{name}: {error}")
            if name == "full-check-002":
                report["final_validation"] = "not-established"

    for relative in HELPERS + [str(LOCAL / name) for name in EXTRAS]:
        try:
            retain(root / relative)
        except (OSError, ValueError) as error:
            report["errors"].append(f"{relative}: {error}")
    try:
        inputs = json.loads(retain(root / LOCAL / "final-inputs.json"))
        for relative, digest in inputs.items():
            require(type(relative) is str and not Path(relative).is_absolute(), "invalid frozen source path")
            retain(root / relative, digest)
        report["final_source_inputs_verified"] = True
    except (OSError, ValueError, TypeError) as error:
        report["final_source_inputs_verified"] = False
        report["errors"].append(f"final source inputs: {error}")
    report["archived_bytes"] = held
    if not report["errors"]:
        report["status"] = "partial-pending-final-check" if any(row["status"] == "pending" for row in report["attempts"]) else "complete-evidence-collection"
    if report["errors"]:
        report["final_validation"] = "not-established"
    # The builder itself is retained verbatim and separately hashed; the
    # manifest never attempts to hash its own bytes.
    builder = read_regular(Path(__file__))
    (output / "build_manifest.py").write_bytes(builder)
    report["builder_sha256"] = sha(builder)
    with (output / "manifest.json").open("x") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")
    print(json.dumps({"status": report["status"], "final_validation": report["final_validation"],
                      "errors": report["errors"], "manifest": str(output / "manifest.json")}, indent=2))
    return 2 if report["errors"] else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path, help="new archive directory; never reused")
    parser.add_argument("--allow-pending", action="store_true", help="archive only finalized attempts; do not pin live full-check002 bytes")
    args = parser.parse_args()
    return collect(args.root, args.output_dir, args.allow_pending)


if __name__ == "__main__":
    raise SystemExit(main())
