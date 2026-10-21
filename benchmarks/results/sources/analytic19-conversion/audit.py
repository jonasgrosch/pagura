#!/usr/bin/env python3
"""Audit the fixed analytic19 conversion pilot; never grant release admission.

Repository modules are imported only by main, from the explicitly selected
frozen source root. Image parsing, comparisons and point estimates below are
independent of the benchmark's image/statistics modules. No renderer is run.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import stat
import statistics
import sys

MANIFEST_SHA = "278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3"
REFERENCES_SHA = "924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf"
RIVALS = ("pdfium", "mupdf", "pdfjs", "hayro", "quartz", "poppler")
SAMPLES, WARMUPS, SEED = 5, 2, 20261021
MAX_FILE = 64 * 1024 * 1024
MAX_PIXELS = 16_000_000
MAX_INTEGER = (1 << 63) - 1


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def digest(value):
    return sha(canonical(value))


def same(first, second):
    """JSON equality must distinguish false/0 and true/1 in evidence."""
    return canonical(first) == canonical(second)


def read_regular(path, limit=MAX_FILE):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "expected regular file")
        data = stream.read(limit + 1)
    require(len(data) <= limit, "file exceeds audit byte budget")
    return data


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def parse_json(data):
    def invalid_constant(value):
        raise ValueError("nonfinite JSON constant: " + value)
    return json.loads(data, object_pairs_hook=unique_object, parse_constant=invalid_constant)


def contained(root, relative):
    require(type(relative) is str and relative and "\\" not in relative and "\0" not in relative
            and all(part not in ("", ".", "..") for part in relative.split("/")),
            "noncanonical artifact path")
    path = (root / relative).resolve(strict=True)
    require(path.is_relative_to(root), "artifact escapes store")
    return path


def image(raw):
    """Return dimensions, RGB-over-white bytes, and observed alpha metadata."""
    alpha_kind = None
    if raw.startswith(b"P6"):
        tokens, cursor = [], 0
        while len(tokens) < 4:
            while cursor < len(raw):
                if raw[cursor] in b" \t\r\n\v\f":
                    cursor += 1
                elif raw[cursor] == 35:
                    end = raw.find(b"\n", cursor)
                    require(end >= 0, "unterminated P6 comment")
                    cursor = end + 1
                else:
                    break
            start = cursor
            while cursor < len(raw) and raw[cursor] not in b" \t\r\n\v\f#":
                cursor += 1
            require(cursor > start and cursor <= 4096, "invalid P6 header")
            tokens.append(raw[start:cursor])
        require(tokens[0] == b"P6" and tokens[3] == b"255", "expected RGB8 P6")
        width, height = int(tokens[1]), int(tokens[2])
        require(cursor < len(raw) and raw[cursor] in b" \t\r\n\v\f", "missing P6 separator")
        cursor += 2 if raw[cursor:cursor + 2] == b"\r\n" else 1
        pixels, channels = raw[cursor:], 3
    elif raw.startswith(b"RGBA8 "):
        end = raw.find(b"\n", 0, 4097)
        require(end >= 0, "invalid RGBA8 header")
        fields = raw[:end].split()
        require(len(fields) == 4 and fields[3] in (b"straight", b"premultiplied"), "invalid alpha declaration")
        width, height = int(fields[1]), int(fields[2])
        alpha_kind = fields[3].decode("ascii")
        pixels, channels = raw[end + 1:], 4
    elif raw.startswith((b"P7\n", b"P7\r\n")):
        cursor, fields = raw.index(b"\n") + 1, {}
        while True:
            end = raw.find(b"\n", cursor, 4097)
            require(end >= 0, "invalid P7 header")
            line = raw[cursor:end].split(b"#", 1)[0].strip()
            cursor = end + 1
            if line == b"ENDHDR":
                break
            if line:
                pair = line.split()
                require(len(pair) == 2 and pair[0] not in fields, "invalid P7 field")
                fields[pair[0]] = pair[1]
        require(set(fields) == {b"WIDTH", b"HEIGHT", b"DEPTH", b"MAXVAL", b"TUPLTYPE"}, "invalid P7 fields")
        width, height, channels = (int(fields[key]) for key in (b"WIDTH", b"HEIGHT", b"DEPTH"))
        require(fields[b"MAXVAL"] == b"255" and (channels, fields[b"TUPLTYPE"]) in
                ((3, b"RGB"), (4, b"RGB_ALPHA")), "unsupported P7 encoding")
        pixels = raw[cursor:]
        alpha_kind = "straight" if channels == 4 else None
    else:
        raise ValueError("unsupported raw image container")
    require(0 < width and 0 < height and width * height <= MAX_PIXELS, "invalid image dimensions")
    require(len(pixels) == width * height * channels, "image payload length differs")
    if channels == 3:
        return width, height, pixels, {"status": "unavailable", "reason": "RGB-only container"}
    alpha = pixels[3::4]
    rgb = bytearray(width * height * 3)
    for index in range(width * height):
        a = pixels[4 * index + 3]
        for channel in range(3):
            c = pixels[4 * index + channel]
            if alpha_kind == "premultiplied":
                require(c <= a, "invalid premultiplied channel")
                value = c + 255 - a
            else:
                value = (c * a + 127) // 255 + 255 - a
            rgb[3 * index + channel] = value
    return width, height, bytes(rgb), {
        "status": "encoded", "representation": alpha_kind, "sha256": sha(alpha),
        "minimum": min(alpha), "maximum": max(alpha), "nonopaque_pixels": len(alpha) - alpha.count(255),
        "accuracy": "no independent alpha oracle in this campaign"}


def compare(actual, expected):
    if actual[:2] != expected[:2]:
        return {"status": "dimension-mismatch", "actual": list(actual[:2]), "expected": list(expected[:2])}
    differences = [0, 0, 0]
    maxima = [0, 0, 0]
    different, total = 0, 0
    for index, (a, b) in enumerate(zip(actual[2], expected[2])):
        error = abs(a - b)
        different += error != 0
        total += error
        differences[index % 3] += error
        maxima[index % 3] = max(maxima[index % 3], error)
    return {"status": "correct" if different == 0 else "incorrect", "tolerance": 0,
            "different_channels": different, "maximum_channel_error": max(maxima),
            "sum_channel_errors": total, "channel_maxima": maxima, "channel_sums": differences}


def unbound(value):
    if type(value) is dict:
        return {key: unbound(item) for key, item in value.items() if key != "artifact_binding"}
    if type(value) is list:
        return [unbound(item) for item in value]
    return value


def stages(measurement, engine):
    values = {"elapsed_ns": measurement.get("elapsed_ns")}
    require(type(values["elapsed_ns"]) is int and 0 < values["elapsed_ns"] <= MAX_INTEGER, "invalid process timing")
    record = measurement.get("stages")
    if record is None:
        return values, "unavailable"
    require(type(record) is dict, "invalid stage object")
    keys = ("open_ns", "render_ns", "emit_ns", "engine_ns", "total_ns")
    require(all(type(record.get(key)) is int and record[key] >= 0 for key in keys), "invalid stage values")
    require(record["engine_ns"] == record["open_ns"] + record["render_ns"], "engine timing identity differs")
    require(record["engine_ns"] + record["emit_ns"] <= record["total_ns"] <= values["elapsed_ns"],
            "impossible stage accounting")
    require(engine != "poppler", "unexpected Poppler stage timing")
    values.update({key: record[key] for key in keys})
    return values, "available"


def point_estimates(values):
    require(values and all(type(value) is int and 0 < value <= MAX_INTEGER for value in values), "unavailable point-estimate inputs")
    ordered = sorted(values)
    position = 0.95 * (len(ordered) - 1)
    low, high = math.floor(position), math.ceil(position)
    return {"count": len(values), "median_ns": statistics.median(ordered),
            "p95_ns": ordered[low] + (ordered[high] - ordered[low]) * (position - low),
            "minimum_ns": ordered[0], "maximum_ns": ordered[-1]}


def summarize(samples, qualified):
    """Never drop a missing, failed or zero observation to compute a ratio."""
    output = {}
    for metric in ("elapsed_ns", "engine_ns", "render_ns", "open_ns", "emit_ns"):
        sides = {}
        for side in ("pagura", "baseline"):
            values = [sample[side].get("metrics", {}).get(metric) for sample in samples]
            if len(values) == SAMPLES and all(type(value) is int and 0 < value <= MAX_INTEGER for value in values):
                sides[side] = point_estimates(values)
        item = {"status": "unavailable" if len(sides) != 2 else "all-pair-unqualified", "sides": sides}
        if len(sides) == 2 and qualified:
            item.update(status="accuracy-qualified", median_ratio=sides["pagura"]["median_ns"] / sides["baseline"]["median_ns"],
                        p95_ratio=sides["pagura"]["p95_ns"] / sides["baseline"]["p95_ns"])
        output[metric] = item
    return output


def command_for(engine, base, source, output):
    if engine == "pagura":
        require(len(base) == 1, "unexpected Pagura prefix options")
        return base + ["render", "--timings", str(source), str(output), "72", "0"]
    if engine == "poppler":
        return base + ["-f", "1", "-l", "1", "-singlefile", "-r", "72", str(source), str(output.with_suffix(""))]
    return base + [str(source), str(output), "72", "0"]


class Auditor:
    def __init__(self, root, package, report, policy, helpers):
        self.root, self.package, self.report, self.policy, self.helpers = root, package, report, policy, helpers
        self.errors, self.attempts, self.cells = [], [], []
        self.seen, self.retained, self.images = set(), {}, {}
        self.oracles, self.references = {}, {}
        self.outer_authenticated = False

    def error(self, label, error):
        self.errors.append({"where": label, "reason": str(error)})

    def stored(self, relative, maximum=MAX_FILE):
        path = contained(self.artifact_root, relative)
        raw = read_regular(path, maximum)
        self.retained[relative] = len(raw)
        require(sum(self.retained.values()) <= self.artifact_maximum, "artifact store exceeds declared budget")
        return raw

    def load_inputs(self):
        manifest_raw = read_regular(self.package / "manifest.json", 1024 * 1024)
        reference_raw = read_regular(self.package / "references.json", 4 * 1024 * 1024)
        require(sha(manifest_raw) == MANIFEST_SHA and sha(reference_raw) == REFERENCES_SHA, "external protocol pins differ")
        manifest, reference = parse_json(manifest_raw), parse_json(reference_raw)
        self.cases = {case["id"]: case for case in reference["cases"]}
        ids = reference["selector"]["case_ids"]
        require(len(ids) == len(self.cases) == 19 and list(self.cases) == ids, "external case inventory differs")
        require([case["id"] for case in manifest["documents"]] == ids, "external manifest order differs")
        for case in self.cases.values():
            require(case["dpi"] == 72 and case["pages"] == 1 and case.get("tolerance", 0) == 0, "external settings differ")
            raw = read_regular(self.package / "oracles" / Path(case["oracle"]).name)
            require(sha(raw) == case["oracle_sha256"], "frozen oracle hash differs")
            expected = image(raw)
            require(expected[:2] == (case["width"], case["height"]), "frozen oracle dimensions differ")
            require(sha(read_regular(self.root / case["path"])) == case["sha256"], "current source PDF differs from external pin")
            self.oracles[case["id"]] = expected
            if "render_references" in case:
                metadata, pin = self.helpers["validate_case_reference"](case, "exact")
                self.helpers["load_reference_sources"](self.root, metadata)
                settings = self.helpers["invocation_settings"](case["sha256"], page=0, dpi=72, accuracy="exact",
                    entrypoint="render_page", validation="full",
                    render_policy_sha256=self.helpers["current_render_policy_sha256"](self.root))
                require(same(settings, metadata["settings"]) and metadata["oracle_sha256"] == sha(raw),
                        "external reference invocation/oracle differs")
                self.references[case["id"]] = {"status": "source-and-settings-verified", "sha256": pin,
                    "model": metadata["reference"]["model"], "scope": "declared sampling policy; not universal PDF mandate"}

    def prepare_report(self):
        report = self.report
        require(type(report) is dict and same(report.get("schema"), 2) and report.get("operation") == "conversion", "wrong report schema/operation")
        require(same([report.get("samples_requested"), report.get("warmups"), report.get("seed")],
                     [SAMPLES, WARMUPS, SEED]), "report protocol differs")
        require(report.get("input_sha256") == MANIFEST_SHA, "report input manifest differs")
        require(report.get("settings") == {"background": "white", "normalization": "RGB8 over white", "page_process": "reopen per page"},
                "report conversion settings differ")
        require(report.get("document_cells") == [], "unexpected document-mode cells")
        require(type(report.get("engine_identities")) is dict and set(report["engine_identities"]) == {"pagura", *RIVALS},
                "engine inventory differs")
        require(report.get("policy_sha256") == sha(read_regular(self.root / "benchmarks/policy.json")), "policy pin differs")
        require(report.get("matrix_sha256") == sha(read_regular(self.root / "benchmarks/release-matrix.json")), "matrix pin differs")
        require(report.get("manifest_sha256") == self.helpers["manifest_digest"](), "generated manifest pin differs")
        require(report.get("gate", {}).get("pass") is False, "exploratory report claims admission")
        self.identity_valid = {}
        for engine, identity in report["engine_identities"].items():
            if type(identity) is dict and identity.get("status") == "unavailable":
                require(type(identity.get("reason")) is str and identity["reason"], "unavailable identity lacks reason")
                self.identity_valid[engine] = False
            else:
                try:
                    self.helpers["verify_identity"](identity)
                    self.identity_valid[engine] = True
                except (OSError, ValueError, TypeError, KeyError) as error:
                    self.identity_valid[engine] = False
                    self.error("identity/" + engine, error)
        run = report.get("artifact_run")
        require(type(run) is dict and same(run.get("schema"), 1), "missing artifact store")
        pointer = report.get("runner", {}).get("coordination", {})
        require(run.get("run_id") == pointer.get("run_id") and run.get("receipt_path") == pointer.get("receipt"), "artifact/runner identity differs")
        require(run.get("normalizer_sha256") == self.helpers["normalizer_sha256"](), "normalizer pin differs")
        self.artifact_root = Path(run["root"]).resolve(strict=True)
        require(Path(run["root"]).is_absolute() and self.artifact_root.is_dir(), "invalid artifact store path")
        self.artifact_maximum = self.policy.get("maximum_artifact_bytes", 1024 * 1024 * 1024)
        require(type(run.get("maximum_artifact_bytes")) is int and run["maximum_artifact_bytes"] == self.artifact_maximum,
                "artifact allowance differs")
        require(type(run.get("stored_bytes")) is int and 0 <= run["stored_bytes"] <= self.artifact_maximum, "invalid retained byte count")
        require(same(run.get("maximum_input_bytes"), self.policy.get("maximum_input_bytes", MAX_FILE)), "input allowance differs")
        require(type(report.get("execution_contexts")) is dict, "missing execution context inventory")
        if run.get("status") != "complete" or run.get("errors"):
            self.error("artifact-store", "incomplete: " + str(run.get("errors", run.get("status"))))

    def attempt(self, case, rival, pair, order, engine, measurement):
        label = f"{case['id']}/{rival}/{pair}/{engine}"
        result = {"case": case["id"], "competitor": rival, "pair": pair, "engine": engine,
                  "phase": "warmup" if pair < 0 else "sample", "reported_status": measurement.get("status"),
                  "classification": "invalid-evidence", "binding_valid": False, "rgb": {"status": "unavailable"}}
        self.attempts.append(result)
        try:
            binding = measurement.get("artifact_binding")
            if binding is None:
                if measurement.get("status") == "invalid-evidence" and measurement.get("exit_code") is None:
                    result.update(classification="prelaunch-refused", reason=measurement.get("artifact_error", "invalid evidence"))
                raise ValueError("attempt has no immutable execution receipt")
            require(type(binding) is dict and same(binding.get("schema"), 1) and same(binding.get("page"), 0), "invalid execution binding")
            key = binding["context_sha256"]
            require(key not in self.seen, "execution context reused by another attempt")
            context = self.report["execution_contexts"][key]
            self.seen.add(key)
            require(type(context) is dict and digest(context) == key and binding.get("run_id") == self.report["artifact_run"]["run_id"],
                    "execution context digest/run differs")
            identity = self.report["engine_identities"][engine]
            settings = ({"accuracy": "exact", "entrypoint": "render_page", "validation": "full", "background": "white",
                         "output": "RGB8 PPM", "box": "engine-default", "annotations": True} if engine == "pagura" else
                        {"background": "white", "output": "RGB8 over white", "scope": "adapter-request"})
            expected = {"schema": 1, "run_id": binding["run_id"], "operation": "conversion", "case": case["id"],
                "source_pdf_sha256": case["sha256"], "pages": [0], "dpi": 72, "engine": engine,
                "engine_identity_sha256": digest(identity), "comparison_engine": rival, "pair": pair,
                "phase": result["phase"], "order": order, "render_settings": settings, "references": {},
                "input_snapshot": f"inputs/{case['sha256']}.pdf"}
            require(all(same(context.get(field), value) for field, value in expected.items()), "context differs from external invocation")
            paths = context["output_paths"]
            require(type(paths) is dict and set(paths) == {"0"} and type(paths["0"]) is str and Path(paths["0"]).is_absolute(),
                    "invalid invocation output paths")
            command = command_for(engine, identity["command"], self.artifact_root / expected["input_snapshot"], Path(paths["0"]))
            expected.update(output_paths=paths, command=command, command_sha256=digest(command))
            require(same(context, expected) and self.identity_valid[engine], "command/context/engine binding differs")
            source = self.stored(expected["input_snapshot"])
            require(sha(source) == case["sha256"], "input snapshot differs from independent PDF pin")
            receipt_name = f"receipts/{binding['receipt_sha256']}.json"
            require(binding.get("receipt") == receipt_name, "noncanonical receipt name")
            raw_receipt = self.stored(receipt_name, 16 * 1024 * 1024)
            require(sha(raw_receipt) == binding["receipt_sha256"], "execution receipt hash differs")
            receipt = parse_json(raw_receipt)
            require(type(receipt) is dict and same(receipt.get("schema"), 1) and same(receipt.get("context"), expected)
                    and same(receipt.get("measurement"), unbound(measurement)), "receipt differs from bound context/measurement")
            outputs = receipt["outputs"]
            require(type(outputs) is dict and set(outputs) == {"0"} and type(outputs["0"]) is dict, "receipt output inventory differs")
            output = outputs["0"]
            result.update(binding_valid=True, classification="launched", receipt_sha256=binding["receipt_sha256"],
                          exit_code=measurement.get("exit_code"), diagnostic_retention="failure tail only; successful raw log unavailable")
            for field in ("diagnostic", "feature", "timing_note"):
                if field in measurement:
                    result[field] = measurement[field]
            if "blob" in output:
                require(output["blob"] == f"blobs/{output['raw_sha256']}.raw", "noncanonical raw blob name")
                raw = self.stored(output["blob"], self.policy["maximum_output_bytes"])
                require(sha(raw) == output["raw_sha256"] and same(len(raw), output["raw_bytes"]), "raw blob hash/length differs")
                result["raw_sha256"] = sha(raw)
                try:
                    actual = self.images.get(sha(raw))
                    if actual is None:
                        actual = image(raw)
                        self.images[sha(raw)] = actual
                except ValueError as error:
                    result.update(rgb={"status": "invalid-output", "reason": str(error)}, classification="invalid-output")
                    require("error" in output or measurement.get("status") == "invalid-output", "unreported malformed output")
                else:
                    normalized_ppm = f"P6\n{actual[0]} {actual[1]}\n255\n".encode() + actual[2]
                    normal = {"width": actual[0], "height": actual[1], "pixels_sha256": sha(actual[2]),
                              "normalized_ppm_sha256": sha(normalized_ppm), "normalization": "rgb8-over-white-v1",
                              "normalizer_sha256": self.report["artifact_run"]["normalizer_sha256"]}
                    if "error" not in output:
                        require(all(same(output.get(field), value) for field, value in normal.items()), "normalized output receipt differs")
                    result.update(rgb=compare(actual, self.oracles[case["id"]]), alpha=actual[3],
                                  normalized_rgb_sha256=normal["pixels_sha256"])
                    if measurement.get("status") in ("ok", "unverified", "incorrect"):
                        require(all(same(measurement.get(field), value) for field, value in {
                            "width": actual[0], "height": actual[1], "pixels_sha256": normal["pixels_sha256"], "output_sha256": sha(raw)}.items()),
                            "measurement output fields differ")
                        require(measurement["status"] == ("ok" if result["rgb"]["status"] == "correct" else "incorrect"),
                                "recorded correctness differs from independent oracle")
            else:
                require("error" in output, "missing output without failure record")
                result.update(rgb={"status": "missing-output", "reason": str(output["error"])}, classification="missing-output")
            if "error" in output:
                result["retention_error"] = str(output["error"])
                self.error(label, "output retention/normalization incomplete: " + str(output["error"]))
            reported = measurement.get("status")
            if reported not in ("ok", "incorrect") or not same(measurement.get("exit_code"), 0):
                result["classification"] = str(reported or "unknown-process-failure")
            else:
                result["classification"] = result["rgb"]["status"]
            try:
                result["metrics"], result["stage_status"] = stages(measurement, engine)
                if reported in ("ok", "incorrect"):
                    expected_stage = "unavailable" if engine == "poppler" else "available"
                    require(result["stage_status"] == expected_stage and measurement.get("stages_status") == expected_stage,
                            "completed stage status differs from engine capability")
            except ValueError as error:
                self.error(label + "/timing", error)
                result["timing_error"] = str(error)
            rss = measurement.get("peak_rss_bytes")
            result["peak_rss_bytes"] = rss if type(rss) is int and 0 < rss <= MAX_INTEGER else None
            result["rss_status"] = "available" if result["peak_rss_bytes"] is not None else "unavailable"
        except (OSError, ValueError, KeyError, TypeError, AttributeError, OverflowError) as error:
            self.error(label, error)
            result["evidence_error"] = str(error)
        return result

    def inspect_cells(self):
        by_key = {}
        require(type(self.report.get("cells")) is list, "missing comparison cells")
        for cell in self.report["cells"]:
            require(type(cell) is dict and type(cell.get("case")) is str and type(cell.get("engine")) is str, "invalid cell identity")
            key = (cell["case"], cell["engine"])
            require(key not in by_key, "duplicate comparison cell")
            by_key[key] = cell
        expected_keys = {(name, engine) for name in self.cases for engine in RIVALS}
        if set(by_key) != expected_keys:
            self.error("matrix", f"missing {len(expected_keys - set(by_key))}; unexpected {len(set(by_key) - expected_keys)} cells")
        for name, case in self.cases.items():
            # snapshot() precedes engine availability checks; even an entirely
            # unavailable case can own a retained input in the store inventory.
            try:
                require(sha(self.stored(f"inputs/{case['sha256']}.pdf")) == case["sha256"], "retained case PDF differs")
            except (OSError, ValueError) as error:
                self.error(name + "/input", error)
            for rival in RIVALS:
                cell = by_key.get((name, rival))
                if cell is None:
                    self.cells.append({"case": name, "competitor": rival, "classification": "missing-cell",
                        "accuracy_qualified": False, "planned_invocations": 2 * (SAMPLES + WARMUPS),
                        "observed_attempts": 0})
                    continue
                summary = {"case": name, "competitor": rival, "reported_status": cell.get("status")}
                self.cells.append(summary)
                errors_before = len(self.errors)
                try:
                    require(same(cell.get("page"), 0) and same(cell.get("dpi"), 72) and cell.get("operation") == "conversion"
                            and cell.get("pdf_sha256") == case["sha256"] and cell.get("oracle_pixels_sha256") == sha(self.oracles[name][2])
                            and cell.get("verification") == "independent-oracle", "cell differs from frozen case")
                    unavailable = [engine for engine in ("pagura", rival)
                                   if self.report["engine_identities"][engine].get("status") == "unavailable"]
                    if unavailable:
                        require(cell.get("status") == "unavailable" and cell.get("samples") == []
                                and cell.get("warmup_samples") == [], "unavailable engine has measured samples")
                        summary.update(classification="unavailable", engines=unavailable,
                            reasons={engine: self.report["engine_identities"][engine]["reason"] for engine in unavailable},
                            planned_invocations=2 * (SAMPLES + WARMUPS), launched_invocations=0, accuracy_qualified=False)
                        continue
                    if cell.get("status") != "measured":
                        self.error(name + "/" + rival, "available cell was not measured: " + str(cell.get("reason", cell.get("status"))))
                    measured, all_attempts = [], []
                    for field, expected_pairs in (("warmup_samples", list(range(-WARMUPS, 0))), ("samples", list(range(SAMPLES)))):
                        rows = cell.get(field)
                        require(type(rows) is list, "invalid pair inventory")
                        if (len(rows) != len(expected_pairs)
                                or not all(type(row) is dict and type(row.get("pair")) is int for row in rows)
                                or sorted(row["pair"] for row in rows) != expected_pairs):
                            self.error(name + "/" + rival + "/" + field, "missing/duplicate/unexpected pair or warmup")
                        for row in rows:
                            require(type(row) is dict and type(row.get("pair")) is int, "invalid pair record")
                            require(row.get("order") in (["pagura", rival], [rival, "pagura"]), "invalid paired order")
                            pair = {}
                            for side, engine in (("pagura", "pagura"), ("baseline", rival)):
                                require(type(row.get(side)) is dict, "missing attempt measurement")
                                pair[side] = self.attempt(case, rival, row["pair"], row["order"], engine, row[side])
                                all_attempts.append(pair[side])
                            if row["pair"] >= 0:
                                measured.append(pair)
                    qualified = (len(self.errors) == errors_before and self.outer_authenticated
                                 and all(attempt["binding_valid"] and attempt["classification"] == "correct" for attempt in all_attempts))
                    summary.update(classification="audited", accuracy_qualified=qualified, metrics=summarize(measured, qualified),
                                   attempts=len(all_attempts), planned_invocations=2 * (SAMPLES + WARMUPS),
                                   unrecorded_invocations=max(0, 2 * (SAMPLES + WARMUPS) - len(all_attempts)),
                                   classifications=dict(Counter(item["classification"] for item in all_attempts)))
                    rss = {side: [sample[side].get("peak_rss_bytes") for sample in measured] for side in ("pagura", "baseline")}
                    if all(len(values) == SAMPLES and all(type(value) is int and value > 0 for value in values) for values in rss.values()):
                        summary["process_rss"] = {"max_bytes": {side: max(values) for side, values in rss.items()}, "scope": "whole process"}
                        if qualified:
                            summary["process_rss"]["ratio"] = max(rss["pagura"]) / max(rss["baseline"])
                    else:
                        summary["process_rss"] = {"status": "unavailable"}
                except (ValueError, KeyError, TypeError, AttributeError) as error:
                    summary.update(classification="invalid-evidence", accuracy_qualified=False)
                    self.error(name + "/" + rival, error)
        extra = set(self.report["execution_contexts"]) - self.seen
        if extra:
            self.error("contexts", f"{len(extra)} contexts are not bound to expected attempts")
        if sum(self.retained.values()) != self.report["artifact_run"]["stored_bytes"]:
            self.error("artifact-store", "retained byte accounting differs from complete report inventory")

    def result(self):
        # A global binding failure invalidates every quality-qualified ratio.
        if self.errors:
            for cell in self.cells:
                cell["accuracy_qualified"] = False
                for metric in cell.get("metrics", {}).values():
                    metric.pop("median_ratio", None)
                    metric.pop("p95_ratio", None)
                    if metric["status"] == "accuracy-qualified":
                        metric["status"] = "all-pair-unqualified"
                cell.get("process_rss", {}).pop("ratio", None)
        complete = not self.errors
        all_correct = complete and len(self.cells) == 114 and all(cell.get("accuracy_qualified") for cell in self.cells)
        return {"schema": 1, "status": "audited" if complete else "invalid-or-incomplete-evidence",
                "outer_receipt_authenticated": self.outer_authenticated,
                "all_cases_correct_under_frozen_rgb_policy": all_correct, "release_admitted": False,
                "manifest_sha256": MANIFEST_SHA, "references_sha256": REFERENCES_SHA,
                "protocol": {"cases": 19, "competitors": list(RIVALS), "samples": SAMPLES, "warmups": WARMUPS, "seed": SEED,
                             "planned_invocations": 19 * len(RIVALS) * 2 * (SAMPLES + WARMUPS)},
                "attempts_observed": len(self.attempts), "attempt_classifications": dict(Counter(row["classification"] for row in self.attempts)),
                "unavailable_cells": sum(cell.get("classification") == "unavailable" for cell in self.cells),
                "retained_unique_bytes": sum(self.retained.values()), "reference_profiles": self.references,
                "errors": self.errors, "cells": self.cells, "attempts": self.attempts,
                "limits": ["Comparison is to a declared independent sampling policy, not universal PDF conformance.",
                           "PPM alpha and successful original diagnostic logs are unavailable.",
                           "Stored stages are authenticated and checked arithmetically; original success logs cannot be reparsed.",
                           "Five pairs provide point estimates, not calibrated tail/confidence or release claims.",
                           "Runner receipts describe cooperative execution, not dedicated hardware or hostile-runner attestation."]}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--runner-receipt", type=Path, required=True)
    parser.add_argument("--runner-receipt-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    root, package = args.source_root.resolve(strict=True), Path(__file__).resolve().parent
    sys.path.insert(0, str(root / "tools"))
    from benchkit import artifacts, config, corpus, reference_cases, runner_evidence
    require(config.ROOT.resolve() == root, "imported helpers are outside selected source root")
    helpers = {name: getattr(reference_cases, name) for name in (
        "validate_case_reference", "load_reference_sources", "invocation_settings", "current_render_policy_sha256")}
    helpers.update(verify_identity=artifacts.verify_identity, normalizer_sha256=artifacts.normalizer_sha256,
                   manifest_digest=corpus.manifest_digest)
    report_raw = read_regular(args.report)
    report = parse_json(report_raw)
    policy = parse_json(read_regular(root / "benchmarks/policy.json", 1024 * 1024))
    auditor = Auditor(root, package, report, policy, helpers)
    source_before = config.source_digest()
    code_before = read_regular(Path(__file__))
    runtime_path = Path(sys.executable).resolve(strict=True)
    runtime_before = read_regular(runtime_path)
    try:
        runner_evidence.verify_runner_receipt(report, report_path=args.report, receipt_path=args.runner_receipt,
            receipt_sha256=args.runner_receipt_sha256, source_sha256=source_before)
        auditor.outer_authenticated = True
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        auditor.error("outer-runner", error)
    try:
        auditor.load_inputs()
        auditor.prepare_report()
        auditor.inspect_cells()
    except (OSError, ValueError, KeyError, TypeError, AttributeError, OverflowError, RecursionError) as error:
        auditor.error("audit", error)
    try:
        require(config.source_digest() == source_before, "source changed during audit")
        require(read_regular(args.report) == report_raw, "report changed during audit")
        require(read_regular(Path(__file__)) == code_before, "auditor changed during audit")
        require(read_regular(runtime_path) == runtime_before, "auditor executable changed during audit")
        require(sha(read_regular(package / "manifest.json")) == MANIFEST_SHA
                and sha(read_regular(package / "references.json")) == REFERENCES_SHA, "external protocol changed during audit")
        for engine, valid in getattr(auditor, "identity_valid", {}).items():
            if valid:
                artifacts.verify_identity(report["engine_identities"][engine])
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        auditor.error("final-bindings", error)
    result = auditor.result()
    result.update(report_sha256=sha(report_raw), runner_receipt_sha256=args.runner_receipt_sha256,
                  audit_source_sha256=source_before, auditor_sha256=sha(code_before),
                  python_executable=str(runtime_path), python_executable_sha256=sha(runtime_before), python_version=sys.version)
    with args.output.open("x") as stream:
        json.dump(result, stream, indent=2, allow_nan=False)
        stream.write("\n")
    return 2 if auditor.errors else 0 if result["all_cases_correct_under_frozen_rgb_policy"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
