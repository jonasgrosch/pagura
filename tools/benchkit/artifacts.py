"""Durable output evidence, outside measurement and without release admission.

The trusted local harness freezes invocation contexts before launch. A separate
coordinator receipt must authenticate the containing report before a gate treats
those contexts as expected inputs. Hashes here detect missing/changed evidence;
they do not attest a hostile runner or provide an exclusive-compute guarantee.
"""
import json
import hashlib
import os
import re
import shutil
import stat
import uuid
from pathlib import Path

from .config import ROOT, sha
from .images import normalize
from .reference_cases import (current_render_policy_sha256, invocation_settings,
                              validate_case_reference, verify_case_reference)

DEFAULT_MAXIMUM_ARTIFACT_BYTES = 1024 * 1024 * 1024
DEFAULT_MAXIMUM_INPUT_BYTES = 64 * 1024 * 1024
MAXIMUM_RECEIPT_BYTES = 16 * 1024 * 1024
MAXIMUM_ENGINE_ARTIFACT_BYTES = 1024 * 1024 * 1024
NORMALIZATION = "rgb8-over-white-v1"


def canonical_bytes(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def digest(value):
    return sha(canonical_bytes(value))


def _copy(value):
    return json.loads(canonical_bytes(value))


def _unbound(value):
    if isinstance(value, dict):
        return {key: _unbound(item) for key, item in value.items() if key != "artifact_binding"}
    if isinstance(value, list):
        return [_unbound(item) for item in value]
    return value


def _limit(value, label):
    if type(value) is not int or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


def read_bounded_regular(path, limit):
    if type(limit) is not int or limit < 0:
        raise ValueError("read limit must be a nonnegative integer")
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("artifact must be a regular file")
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("artifact exceeds byte budget")
    return data


def _contained(root, relative):
    if (type(relative) is not str or not relative or "\\" in relative or "\0" in relative
            or any(part in ("", ".", "..") for part in relative.split("/"))):
        raise ValueError("artifact path must be canonical and relative")
    base = Path(root).resolve(strict=True)
    path = (base / relative).resolve(strict=True)
    if not path.is_relative_to(base) or not path.is_file():
        raise ValueError("artifact escapes store or is not a file")
    return path


def normalizer_sha256():
    # Include the container/normalization declaration as well as image code.
    return sha(Path(__file__).read_bytes() + b"\0" + Path(__file__).with_name("images.py").read_bytes())


def _image_record(raw, normalizer):
    width, height, pixels = normalize(raw)
    ppm = f"P6\n{width} {height}\n255\n".encode() + pixels
    return {"raw_sha256": sha(raw), "raw_bytes": len(raw),
            "normalization": NORMALIZATION, "normalizer_sha256": normalizer,
            "pixels_sha256": sha(pixels), "normalized_ppm_sha256": sha(ppm),
            "width": width, "height": height}, ppm


def hash_regular_file(path, maximum=MAXIMUM_ENGINE_ARTIFACT_BYTES):
    """Bounded-memory, bounded-byte hashing, refusing FIFOs and device files."""
    _limit(maximum, "maximum hash bytes")
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("engine artifact must be a regular file")
        if metadata.st_size > maximum:
            raise ValueError("engine artifact exceeds byte budget")
        actual, total = hashlib.sha256(), 0
        while chunk := stream.read(min(1024 * 1024, maximum - total + 1)):
            total += len(chunk)
            if total > maximum:
                raise ValueError("engine artifact exceeds byte budget")
            actual.update(chunk)
    return actual.hexdigest()


def verify_identity(identity, command=None):
    """Check every declared artifact and the actual executed program/prefix."""
    if type(identity) is not dict:
        raise ValueError("invalid engine identity")
    if (identity.get("status") != "available" or not isinstance(identity.get("artifacts"), dict)
            or not identity["artifacts"]):
        raise ValueError("engine has no pinned executable/artifact identity")
    base = identity.get("command")
    if command is None:
        command = base
    if (type(base) is not list or not base or any(type(item) is not str for item in base)
            or command[:len(base)] != base):
        raise ValueError("executed command does not begin with pinned engine command")
    if any(type(path) is not str or not path or type(expected) is not str
           or re.fullmatch(r"[0-9a-f]{64}", expected) is None
           for path, expected in identity["artifacts"].items()):
        raise ValueError("invalid pinned engine artifact identity")
    executable = shutil.which(base[0])
    if executable is None or Path(executable).resolve() not in {Path(path).resolve() for path in identity["artifacts"]}:
        raise ValueError("executed program is absent from pinned engine artifacts")
    for path, expected in identity["artifacts"].items():
        if hash_regular_file(path) != expected:
            raise ValueError("engine artifact changed before invocation")


def _reference_result(reference, ppm, *, source_bytes):
    return verify_case_reference(ROOT, reference["case"], reference["mode"], ppm,
                                 settings=reference["settings"], source_bytes=source_bytes)


def _check_pagura_request(context, base, snapshot):
    """Current harness requests use default Exact/full CLI rendering only."""
    if any(argument.startswith("-") for argument in base[1:]):
        raise ValueError("Pagura engine prefix contains unsupported rendering options")
    arguments = context["command"][len(base):]
    if arguments[:1] == ["--password"]:
        arguments = arguments[2:]
    if not arguments or arguments[0] not in ("render", "convert"):
        raise ValueError("unsupported Pagura invocation contract")
    operation, arguments = arguments[0], arguments[1:]
    if arguments[:1] == ["--timings"]:
        arguments = arguments[1:]
    if len(arguments) != (4 if operation == "render" else 5):
        raise ValueError("unexpected Pagura arguments or rendering options")
    source, output, dpi, *pages = arguments
    expected_pages = ([str(context["pages"][0])] if operation == "render" else
                      [str(context["pages"][0]), str(context["pages"][-1])])
    if (source != str(snapshot)
            or dpi != str(context["dpi"]) or pages != expected_pages):
        raise ValueError("Pagura arguments differ from frozen page/DPI")
    paths = context["output_paths"]
    if operation == "render":
        valid_output = len(paths) == 1 and output == next(iter(paths.values()))
    else:
        valid_output = all(str(Path(path).parent) == output for path in paths.values())
    if not valid_output:
        raise ValueError("Pagura output arguments differ from frozen paths")


class ArtifactStore:
    """One bounded store per report; all successful warmups are retained too."""

    @classmethod
    def for_report(cls, report, policy, *, directory=None):
        return cls(report, policy, directory=directory)

    def __init__(self, report, policy, *, directory=None):
        self.report = report
        self.policy = policy
        self.maximum = _limit(policy.get("maximum_artifact_bytes", DEFAULT_MAXIMUM_ARTIFACT_BYTES),
                              "maximum_artifact_bytes")
        self.output_limit = _limit(policy["maximum_output_bytes"], "maximum_output_bytes")
        self.input_limit = _limit(policy.get("maximum_input_bytes", DEFAULT_MAXIMUM_INPUT_BYTES), "maximum_input_bytes")
        run_id = os.environ.get("PAGURA_RUN_ID") or str(uuid.uuid4())
        if re.fullmatch(r"[A-Za-z0-9_-]{1,128}", run_id) is None:
            raise ValueError("invalid artifact run ID")
        parent = Path(directory) if directory is not None else ROOT / "benchmarks/results/local/artifacts"
        parent.mkdir(parents=True, exist_ok=True)
        self.root = parent / (run_id + "-" + uuid.uuid4().hex)
        self.root.mkdir(mode=0o700)
        self.root = self.root.resolve()
        self.run = {"schema": 1, "run_id": run_id, "root": str(self.root.resolve()),
                    "receipt_path": os.environ.get("PAGURA_RUN_RECEIPT"), "status": "recording",
                    "maximum_artifact_bytes": self.maximum, "stored_bytes": 0,
                    "maximum_input_bytes": self.input_limit,
                    "normalizer_sha256": normalizer_sha256()}
        report["artifact_run"] = self.run
        report["execution_contexts"] = {}
        self._references = {}

    def incomplete(self, error):
        self.run["status"] = "incomplete"
        message = str(error)
        errors = self.run.setdefault("errors", [])
        if message not in errors:
            errors.append(message)

    def finish(self):
        if self.run["status"] == "recording":
            self.run["status"] = "complete"

    def _store(self, category, suffix, data):
        relative = f"{category}/{sha(data)}.{suffix}"
        path = self.root / relative
        if path.exists():
            if read_bounded_regular(path, len(data)) != data:
                raise ValueError("existing content-addressed artifact changed")
            return relative
        if self.run["stored_bytes"] + len(data) > self.maximum:
            raise ValueError("total artifact byte budget exhausted")
        path.parent.mkdir(mode=0o700, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(data)
        path.chmod(0o400)
        self.run["stored_bytes"] += len(data)
        return relative

    def snapshot(self, case):
        """Own the bytes passed to children; this is outside every timed region."""
        if self.run["status"] != "recording":
            return None
        try:
            data = read_bounded_regular(ROOT / case["path"], min(self.input_limit, self.maximum))
            if sha(data) != case["sha256"]:
                raise ValueError("PDF changed after manifest validation")
            relative = self._store("inputs", "pdf", data)
            return self.root / relative
        except (OSError, ValueError) as error:
            self.incomplete(error)
            return None

    def freeze_execution(self, *, cases, snapshot, engine, comparison_engine, pair,
                         order, command, output_paths):
        """Freeze actual argv/request, identity and reviewed pins before launch."""
        if self.run["status"] != "recording":
            return None
        try:
            return self._freeze_execution(cases=cases, snapshot=snapshot, engine=engine,
                comparison_engine=comparison_engine, pair=pair, order=order, command=command,
                output_paths=output_paths)
        except (OSError, ValueError) as error:
            self.incomplete(error)
            return None

    def _freeze_execution(self, *, cases, snapshot, engine, comparison_engine, pair,
                          order, command, output_paths):
        first = cases[0]
        references, pins = {}, {}
        settings = {"background": "white", "output": "RGB8 over white", "scope": "adapter-request"}
        if engine == "pagura":
            settings = {"accuracy": "exact", "entrypoint": "render_page", "validation": "full",
                        "background": "white", "output": "RGB8 PPM", "box": "engine-default", "annotations": True}
            for case in cases:
                if "render_references" not in case or "oracle" not in case:
                    continue
                _, pin = validate_case_reference(case, "exact")
                actual = invocation_settings(case["sha256"], page=case.get("page", 0), dpi=case["dpi"],
                    accuracy="exact", entrypoint="render_page", validation="full",
                    render_policy_sha256=current_render_policy_sha256(ROOT))
                page = str(case.get("page", 0))
                references[page] = {"case": _copy(case), "mode": "exact", "settings": actual}
                pins[page] = {"sha256": pin, "settings": actual}
        context = {"schema": 1, "run_id": self.run["run_id"], "operation": self.report["operation"],
            "case": first["id"], "source_pdf_sha256": first["sha256"],
            "pages": [case.get("page", 0) for case in cases], "dpi": first["dpi"],
            "engine": engine, "engine_identity_sha256": digest(self.report["engine_identities"][engine]),
            "comparison_engine": comparison_engine, "pair": pair, "phase": "warmup" if pair < 0 else "sample",
            "order": list(order), "command": list(command), "command_sha256": digest(command),
            "render_settings": settings, "references": pins,
            "input_snapshot": str(Path(snapshot).relative_to(self.root)),
            "output_paths": {str(page): str(path) for page, path in output_paths.items()}}
        context = _copy(context)
        key = digest(context)
        if key in self.report["execution_contexts"]:
            raise ValueError("duplicate execution context")
        self.report["execution_contexts"][key] = context
        self._references[key] = references
        return context

    def before_launch(self, context, command):
        """Refuse changed input/engine/command before starting the timer."""
        try:
            if self.run["status"] != "recording":
                raise ValueError("artifact evidence is incomplete; further execution refused")
            if context is None:
                raise ValueError("execution context could not be frozen")
            key = digest(context)
            if self.report["execution_contexts"].get(key) != context or list(command) != context["command"]:
                raise ValueError("invocation differs from its frozen context")
            identity = self.report["engine_identities"][context["engine"]]
            if digest(identity) != context["engine_identity_sha256"]:
                raise ValueError("engine identity changed after context freeze")
            verify_identity(identity, list(command))
            path = _contained(self.root, context["input_snapshot"])
            if str(path) not in command or sha(read_bounded_regular(path, min(self.input_limit, self.maximum))) != context["source_pdf_sha256"]:
                raise ValueError("invocation does not use the pinned input snapshot")
            if context["engine"] == "pagura":
                _check_pagura_request(context, identity["command"], path)
        except (OSError, ValueError) as error:
            self.incomplete(error)
            raise

    def capture(self, context, result, outputs, *, page):
        """Persist every raw output, then one immutable execution receipt."""
        try:
            context_key = digest(context)
            if self.report["execution_contexts"].get(context_key) != context:
                raise ValueError("execution context changed before capture")
            records = {}
            references = self._references[context_key]
            source = read_bounded_regular(_contained(self.root, context["input_snapshot"]), min(self.input_limit, self.maximum))
            if sha(source) != context["source_pdf_sha256"]:
                raise ValueError("input snapshot changed during execution")
            for index in context["pages"]:
                path = outputs.get(index)
                if path is None or str(path) != context["output_paths"].get(str(index)):
                    raise ValueError("output path differs from frozen invocation")
                try:
                    raw = read_bounded_regular(path, self.output_limit)
                    blob = self._store("blobs", "raw", raw)
                    records[str(index)] = {"raw_sha256": sha(raw), "raw_bytes": len(raw), "blob": blob}
                    record, _ = _image_record(raw, self.run["normalizer_sha256"])
                    record["blob"] = blob
                    if str(index) in references:
                        # This reviewed reference profile requires native P6.
                        # Generic competitor normalization is not an adapter
                        # permission for a pinned Pagura invocation contract.
                        record["reference"] = _reference_result(references[str(index)], raw, source_bytes=source)
                    records[str(index)] = record
                except (OSError, ValueError) as error:
                    self.incomplete(error)
                    records.setdefault(str(index), {})["error"] = str(error)
            receipt = {"schema": 1, "context": context, "measurement": _unbound(result), "outputs": records}
            data = canonical_bytes(receipt)
            if len(data) > MAXIMUM_RECEIPT_BYTES:
                raise ValueError("execution receipt exceeds byte budget")
            relative = self._store("receipts", "json", data)
            binding = {"schema": 1, "run_id": self.run["run_id"], "context_sha256": context_key,
                       "receipt": relative, "receipt_sha256": sha(data), "page": page}
            result["artifact_binding"] = binding
            for item in result.get("pages", []):
                item["artifact_binding"] = {**binding, "page": item["page"]}
        except (OSError, ValueError) as error:
            self.incomplete(error)
            result["artifact_error"] = str(error)


def verify_artifact_binding(binding, *, expected_context, claimed_measurement,
                            artifact_root, policy, references=None):
    """Reread raw bytes against independently authenticated expected context.

    `references`, when claimed, must come from the frozen fixture manifest,
    never the receipt. Results prove only retained RGB output comparisons.
    """
    try:
        if (type(binding) is not dict or type(binding.get("schema")) is not int or binding["schema"] != 1
                or binding.get("context_sha256") != digest(expected_context)
                or binding.get("run_id") != expected_context["run_id"]):
            raise ValueError("artifact binding differs from expected context")
        output_limit = _limit(policy["maximum_output_bytes"], "maximum_output_bytes")
        maximum = _limit(policy.get("maximum_artifact_bytes", DEFAULT_MAXIMUM_ARTIFACT_BYTES), "maximum_artifact_bytes")
        input_limit = _limit(policy.get("maximum_input_bytes", DEFAULT_MAXIMUM_INPUT_BYTES), "maximum_input_bytes")
        raw_receipt = read_bounded_regular(_contained(artifact_root, binding["receipt"]), min(maximum, MAXIMUM_RECEIPT_BYTES))
        if (sha(raw_receipt) != binding["receipt_sha256"]
                or binding["receipt"] != f"receipts/{sha(raw_receipt)}.json"):
            raise ValueError("execution receipt hash mismatch")
        receipt = json.loads(raw_receipt)
        if (type(receipt.get("schema")) is not int or receipt["schema"] != 1
                or canonical_bytes(receipt.get("context")) != canonical_bytes(expected_context)
                or expected_context["command_sha256"] != digest(expected_context["command"])):
            raise ValueError("execution receipt context mismatch")
        source = read_bounded_regular(_contained(artifact_root, expected_context["input_snapshot"]), min(input_limit, maximum))
        if (sha(source) != expected_context["source_pdf_sha256"]
                or expected_context["input_snapshot"] != f"inputs/{sha(source)}.pdf"):
            raise ValueError("input snapshot hash mismatch")
        references = {str(key): value for key, value in (references or {}).items()}
        if set(references) != set(expected_context["references"]):
            raise ValueError("independent reference inputs are missing or unexpected")
        outputs = receipt["outputs"]
        if set(outputs) != {str(page) for page in expected_context["pages"]}:
            raise ValueError("receipt output pages differ from invocation")
        verified = {}
        normalizer = normalizer_sha256()
        retained = {binding["receipt"]: len(raw_receipt), expected_context["input_snapshot"]: len(source)}
        for index, stored in outputs.items():
            if "error" in stored:
                raise ValueError("execution output evidence is incomplete")
            raw = read_bounded_regular(_contained(artifact_root, stored["blob"]), output_limit)
            actual, _ = _image_record(raw, normalizer)
            actual["blob"] = f"blobs/{sha(raw)}.raw"
            if index in references:
                reference = references[index]
                _, pin = validate_case_reference(reference["case"], reference["mode"])
                if expected_context["references"][index] != {"sha256": pin, "settings": reference["settings"]}:
                    raise ValueError("reference differs from independently selected case/settings")
                actual["reference"] = _reference_result(reference, raw, source_bytes=source)
            if canonical_bytes(actual) != canonical_bytes(stored):
                raise ValueError("raw output differs from retained hashes/dimensions/reference")
            retained[actual["blob"]] = len(raw)
            if sum(retained.values()) > maximum:
                raise ValueError("retained execution exceeds total artifact byte budget")
            verified[index] = actual
        measurement = receipt["measurement"]
        page = binding.get("page")
        if page is not None:
            if type(page) is not int or page not in expected_context["pages"]:
                raise ValueError("artifact page is outside invocation")
            if "pages" in measurement:
                matches = [item for item in measurement["pages"] if item.get("page") == page]
                if len(matches) != 1:
                    raise ValueError("measurement page is missing or duplicated")
                measurement = matches[0]
        if canonical_bytes(_unbound(claimed_measurement)) != canonical_bytes(measurement):
            raise ValueError("claimed measurement differs from execution receipt")
        rows = receipt["measurement"].get("pages")
        if rows is None:
            if len(verified) != 1:
                raise ValueError("single-page measurement has multiple outputs")
            rows = [{**receipt["measurement"], "page": expected_context["pages"][0]}]
        if {row["page"] for row in rows} != set(expected_context["pages"]) or len(rows) != len(verified):
            raise ValueError("measurement pages differ from retained outputs")
        for row in rows:
            actual = verified[str(row["page"])]
            if any(row.get(key) != actual[other] for key, other in (
                    ("output_sha256", "raw_sha256"), ("pixels_sha256", "pixels_sha256"),
                    ("width", "width"), ("height", "height"))):
                raise ValueError("measurement pixels/dimensions differ from raw bytes")
        return {"context": expected_context, "measurement": measurement,
                "outputs": verified, "receipt_sha256": sha(raw_receipt)}
    except (OSError, KeyError, TypeError, OverflowError, AttributeError) as error:
        raise ValueError("missing or malformed artifact evidence") from error
