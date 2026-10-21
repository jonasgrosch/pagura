"""Bind benchmark reports to independently pinned cooperative-runner receipts.

This verifies recorded execution provenance, not dedicated hardware, thermal
stability, or authenticity against a malicious runner. The receipt hash must be
provided by the caller independently of the candidate report.
"""
import hashlib
import json
import os
import re
import stat
import uuid
from datetime import datetime
from pathlib import Path

from .config import ROOT


def coordination_pointer(environment=None):
    """Read only the wrapper's two explicit identifiers; absence grants nothing."""
    environment = os.environ if environment is None else environment
    run_id, path = environment.get("PAGURA_RUN_ID"), environment.get("PAGURA_RUN_RECEIPT")
    if run_id is None and path is None:
        return None
    if not isinstance(run_id, str) or not isinstance(path, str):
        raise ValueError("incomplete runner coordination environment")
    try:
        if str(uuid.UUID(run_id)) != run_id or not Path(path).is_absolute():
            raise ValueError("noncanonical runner identity")
    except (ValueError, AttributeError) as error:
        raise ValueError("invalid runner coordination environment") from error
    return {"run_id": run_id, "receipt": str(Path(path).resolve())}


def _bounded(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("evidence must be a regular file")
        with os.fdopen(descriptor, "rb", closefd=False) as handle:
            data = handle.read(limit + 1)
    finally:
        os.close(descriptor)
    if len(data) > limit:
        raise ValueError("evidence file exceeds budget")
    return data


def read_bound_report(path):
    """Reject special files and oversized candidate reports before JSON parsing."""
    report = json.loads(_bounded(path, 64 * 1024 * 1024))
    if type(report) is not dict:
        raise ValueError("benchmark report must be an object")
    return report


def verify_runner_receipt(report, *, report_path, receipt_path, receipt_sha256, source_sha256):
    """Return validated receipt data or raise; never set runner.controlled.

    The completed wrapper binds the final report bytes after its child exits.
    Reports contain only a run ID and receipt path, avoiding a circular hash.
    """
    if (report_path is None or receipt_path is None or type(receipt_sha256) is not str
            or re.fullmatch(r"[0-9a-f]{64}", receipt_sha256) is None):
        raise ValueError("independently pinned runner receipt and report path required")
    receipt_path, report_path = Path(receipt_path).resolve(), Path(report_path).resolve()
    raw = _bounded(receipt_path, 4 * 1024 * 1024)
    if hashlib.sha256(raw).hexdigest() != receipt_sha256:
        raise ValueError("runner receipt differs from independent pin")
    receipt = json.loads(raw)
    if type(receipt) is not dict or type(receipt.get("schema")) is not int or receipt["schema"] != 1:
        raise ValueError("unsupported runner receipt")
    runner = report.get("runner")
    if type(runner) is not dict:
        raise ValueError("missing runner identity")
    pointer = runner.get("coordination")
    expected = coordination_pointer({"PAGURA_RUN_ID": receipt.get("run_id"),
                                     "PAGURA_RUN_RECEIPT": str(receipt_path)})
    if pointer != expected:
        raise ValueError("report and runner identities differ")
    if receipt.get("environment_overrides") != {
            "PAGURA_RUN_ID": expected["run_id"], "PAGURA_RUN_RECEIPT": expected["receipt"]}:
        raise ValueError("runner environment binding differs")
    if receipt.get("kind") != "benchmark" or receipt.get("status") != "completed":
        raise ValueError("runner receipt is not a completed benchmark")
    for key in ("exit_code", "wrapper_exit_code"):
        if type(receipt.get(key)) is not int or receipt[key] != 0:
            raise ValueError("runner did not complete successfully")
    if (receipt.get("cancellation_signal") is not None or receipt.get("errors") != []
            or receipt.get("artifact_errors") != {} or receipt.get("source_unchanged") is not True):
        raise ValueError("runner evidence is incomplete or invalid")
    for key in ("duration_ns", "child_pid"):
        if type(receipt.get(key)) is not int or receipt[key] <= 0:
            raise ValueError("invalid runner execution identity")
    try:
        times = [datetime.fromisoformat(receipt[key]) for key in
                 ("started_at", "ended_at", "finalized_at")]
        if any(value.tzinfo is None for value in times) or not times[0] <= times[1] <= times[2]:
            raise ValueError("unordered runner timestamps")
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("invalid runner timestamps") from error
    coordination = receipt.get("coordination")
    if (receipt.get("cwd") != str(ROOT.resolve()) or receipt.get("source_root") != str(ROOT.resolve())
            or type(coordination) is not dict or coordination.get("scope") != "cooperative"):
        raise ValueError("wrong runner source root or coordination scope")
    for key, expected_path in (("lock_file", "/tmp/pagura-bench.lock"),
                               ("lock_directory", "/tmp/pagura-bench.lockdir")):
        path = coordination.get(key)
        if type(path) is not str or Path(path).resolve() != Path(expected_path).resolve():
            raise ValueError("runner did not use the shared coordination paths")
    argv = receipt.get("argv")
    if type(argv) is not list or not argv or any(type(value) is not str for value in argv):
        raise ValueError("invalid runner command")
    for key in ("source_before", "source_after"):
        source = receipt.get(key)
        if (type(source) is not dict or source.get("sha256") != source_sha256
                or source.get("sha256") != report.get("source_sha256")
                or not source.get("commit") or source.get("commit") != report.get("commit")):
            raise ValueError("runner source differs from measured source")
    report_bytes = _bounded(report_path, 64 * 1024 * 1024)
    if json.loads(report_bytes) != report:
        raise ValueError("loaded report differs from bound report bytes")
    artifacts = receipt.get("artifacts")
    if type(artifacts) is not dict or artifacts.get(str(report_path)) != hashlib.sha256(report_bytes).hexdigest():
        raise ValueError("runner does not bind these report bytes")
    return receipt
