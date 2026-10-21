"""Frozen start/current collection protocol; no renderer or admission policy."""
import hashlib
import json
import random
import re
import time

SEED, WARMUPS, PAIRS = 20261114, 5, 200
MAX_FILE, MAX_RETAINED, LOG_BUDGET = 64 << 20, 1 << 30, 256 << 10
MAX_EVENT, MAX_JOURNAL = 1 << 20, 256 << 20
BASELINE = "676a872eb9f6c279cbf1e3929ace3a0c99db9fc3"
CANDIDATE = "a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c"
SIDES = {"B": "baseline", "C": "candidate"}
RSS_LINE = re.compile(r"^[ \t]*(\d+)[ \t]+maximum resident set size[ \t]*$", re.MULTILINE)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    return (json.dumps(value, indent=2, allow_nan=False) + "\n").encode()


def cells(opaque, analytic):
    result = [{"suite": "opaque", "case": name, "mode": mode, "operation": operation}
              for name, mode, operation in opaque.CELLS]
    result += [{"suite": "analytic19", "case": name, "mode": mode, "operation": "cold"}
               for name in analytic.CASES for mode in ("exact", "competitive")]
    require(len(opaque.CELLS) == 46 and len(analytic.CASES) == 19 and len(result) == 84
            and len({tuple(c.values()) for c in result}) == 84, "fixed matrix differs")
    return result


def input_key(cell):
    return cell["suite"] + ":" + cell["case"]


def schedule(matrix, phase):
    if phase == "preflight":
        return [{"phase": "preflight", "pair": 0, "cell_index": i, **cell, "order": "BC"}
                for i, cell in enumerate(matrix)]
    require(phase == "confirmation", "unknown phase")
    rng, result = random.Random(SEED), []
    for label, count in (("warmup", WARMUPS), ("measured", PAIRS)):
        orders = []
        for _ in matrix:
            order = ["BC"] * ((count + 1) // 2) + ["CB"] * (count // 2)
            rng.shuffle(order)
            orders.append(order)
        for pair in range(count):
            indices = list(range(len(matrix)))
            rng.shuffle(indices)
            for index in indices:
                result.append({"phase": label, "pair": pair, "cell_index": index,
                               **matrix[index], "order": orders[index][pair]})
    return result


def opaque_bytes(expected):
    width, height, rgb = expected
    pixels = bytearray(width * height * 4)
    for channel in range(3):
        pixels[channel::4] = rgb[channel::3]
    pixels[3::4] = b"\xff" * (width * height)
    return f"RGBA8 {width} {height} premultiplied\n".encode() + pixels


def quality(raw, expected):
    width, height, rgb = expected
    header = f"RGBA8 {width} {height} premultiplied\n".encode()
    result = {"correct": False, "expected_width": width, "expected_height": height,
              "format_valid": raw.startswith(header) and len(raw) == len(header) + width * height * 4}
    if not result["format_valid"]:
        return result
    pixels = raw[len(header):]
    actual = bytearray(width * height * 3)
    for channel in range(3):
        actual[channel::3] = pixels[channel::4]
    result.update(rgb_sha256=sha(actual), rgb_differing_channels=sum(a != b for a, b in zip(actual, rgb)),
                  rgb_maximum_error=max((abs(a - b) for a, b in zip(actual, rgb)), default=0),
                  nonopaque_pixels=sum(value != 255 for value in pixels[3::4]))
    result["correct"] = result["rgb_differing_channels"] == result["nonopaque_pixels"] == 0
    return result


def classification(pair):
    if any(pair.get(s, {}).get("process_success") is not True for s in SIDES):
        return "process-failure"
    b, c = (pair[s]["quality"]["correct"] for s in SIDES)
    if b and c:
        return ("both-correct-equal" if pair["B"]["output"]["sha256"] == pair["C"]["output"]["sha256"]
                else "both-correct-bytes-differ")
    return "baseline-incorrect-current-correct" if c else "baseline-correct-current-incorrect" if b else "neither-correct"


class Journal:
    """Flushed events; no fsync/power-loss guarantee and no resumption."""
    def __init__(self, path):
        self.stream, self.bytes, self.events = path.open("xb"), 0, 0

    def append(self, kind, **fields):
        event = {"event": self.events, "type": kind, "monotonic_ns": time.monotonic_ns(), **fields}
        raw = (json.dumps(event, separators=(",", ":"), allow_nan=False) + "\n").encode()
        require(len(raw) <= MAX_EVENT and self.bytes + len(raw) <= MAX_JOURNAL, "journal budget exceeded")
        self.stream.write(raw)
        self.stream.flush()
        self.events += 1
        self.bytes += len(raw)

    def close(self):
        self.stream.close()
