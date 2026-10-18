"""Short, routine coverage-guided fuzzing of every cargo-fuzz target.

Runs each target in fuzz/ with libFuzzer for a fixed, short time (30 seconds
by default), one target at a time, starting from the seed corpus that
fuzz/seed.py builds, and prints executions and coverage per target. Exits
with status 1 when any target reports a failure (a panic, the time limit or
the memory limit) and with status 2 when the nightly toolchain or cargo-fuzz
is missing. It needs nightly Rust, so it is a separate command and not part
of tools/check.py. Builds and runs are offline.

    python3 tools/fuzz_smoke.py                        # every target, 30 s each
    python3 tools/fuzz_smoke.py --target font,image --time 120
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FUZZ = ROOT / "fuzz"
# Largest input per target: whole documents need room for their
# cross-reference data; single streams and codec inputs stay small, which
# keeps executions fast.
MAX_LEN = {
    "syntax": 32768,
    "filters": 65536,
    "document": 131072,
    "render": 131072,
    "font": 65536,
    "image": 65536,
    "text": 32768,
    "content": 16384,
    "host": 65536,
    "navigation": 131072,
    "extraction": 16384,
}
LIBFUZZER = ["-rss_limit_mb=2048", "-timeout=10", "-print_final_stats=1"]
BUILD = ["-O", "-a"]  # release with debug assertions and overflow checks
STATUS = re.compile(r"^#(\d+)\s+(\w+)\s+cov: (\d+) ft: (\d+) corp: (\d+)/")
STAT = re.compile(r"^stat::(\w+):\s+(\d+)")


def targets() -> list[str]:
    manifest = tomllib.loads((FUZZ / "Cargo.toml").read_text())
    return [binary["name"] for binary in manifest["bin"]]


def missing_tools() -> str | None:
    """A plain explanation when the toolchain cannot run cargo-fuzz."""
    if shutil.which("cargo") is None:
        return "cargo was not found on PATH; install Rust with rustup first."
    for command, advice in [
        (["cargo", "+nightly", "--version"],
         "The nightly Rust toolchain is not installed; run `rustup toolchain install nightly`."),
        (["cargo", "+nightly", "fuzz", "--version"],
         "cargo-fuzz is not installed; run `cargo install --locked cargo-fuzz`."),
    ]:
        try:
            result = subprocess.run(command, cwd=FUZZ, capture_output=True, text=True)
        except OSError:
            return advice
        if result.returncode != 0:
            return advice
    return None


def classify(log: str) -> str:
    if "libFuzzer: timeout" in log:
        return "time limit"
    if "out-of-memory" in log:
        return "memory limit"
    if "panicked" in log or "deadly signal" in log or "ERROR: AddressSanitizer" in log:
        return "panic"
    return "failure"


def run_target(name: str, seconds: int, seed: int, keep: bool, artifacts: Path, env: dict) -> dict:
    corpus = FUZZ / "corpus" / name
    artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"fuzz-{name}-") as scratch:
        # New units go to the first directory: a scratch one keeps the seed
        # corpus fixed between routine runs unless --keep asks to grow it.
        directories = [str(corpus)] if keep else [scratch, str(corpus)]
        command = ["cargo", "+nightly", "fuzz", "run", *BUILD, name, *directories, "--",
                   f"-max_total_time={seconds}", f"-max_len={MAX_LEN.get(name, 65536)}", f"-seed={seed}",
                   f"-artifact_prefix={artifacts}/", *LIBFUZZER]
        started = time.monotonic()
        result = subprocess.run(command, cwd=FUZZ, env=env, capture_output=True, text=True, errors="replace")
        elapsed = time.monotonic() - started
    log = result.stdout + result.stderr
    status = None
    stats = {}
    for line in log.splitlines():
        if match := STATUS.match(line):
            status = match
        elif match := STAT.match(line):
            stats[match.group(1)] = int(match.group(2))
    outcome = {
        "target": name,
        "ok": result.returncode == 0,
        "executions": stats.get("number_of_executed_units", int(status.group(1)) if status else 0),
        "per_second": stats.get("average_exec_per_sec", 0),
        "coverage": int(status.group(3)) if status else 0,
        "features": int(status.group(4)) if status else 0,
        "corpus": int(status.group(5)) if status else 0,
        "rss_mb": stats.get("peak_rss_mb", 0),
        "seconds": elapsed,
    }
    if not outcome["ok"]:
        outcome["failure"] = classify(log)
        outcome["inputs"] = sorted(str(p) for p in artifacts.iterdir() if p.is_file())
        outcome["log"] = "\n".join(log.splitlines()[-40:])
    return outcome


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--target", action="append", default=[],
                        help="run only these targets (repeatable or comma-separated)")
    parser.add_argument("--time", type=int, default=30, help="seconds per target (default 30)")
    parser.add_argument("--seed", type=int, default=1, help="libFuzzer random seed (default 1)")
    parser.add_argument("--reseed", action="store_true", help="rebuild fuzz/corpus with fuzz/seed.py first")
    parser.add_argument("--keep", action="store_true", help="add new inputs to fuzz/corpus/<target>")
    arguments = parser.parse_args()
    available = targets()
    selected = [t for group in arguments.target for t in group.split(",") if t] or available
    unknown = sorted(set(selected) - set(available))
    if unknown:
        print(f"Unknown target(s): {', '.join(unknown)}. Available: {', '.join(available)}.")
        return 2
    if arguments.time <= 0:
        print("--time must be a positive number of seconds.")
        return 2
    problem = missing_tools()
    if problem:
        print(f"Cannot fuzz: {problem}")
        return 2
    if arguments.reseed or not all((FUZZ / "corpus" / t).is_dir() for t in selected):
        print("Building seed corpora with fuzz/seed.py", flush=True)
        subprocess.run([sys.executable, str(FUZZ / "seed.py")], cwd=ROOT, check=True)
    env = {**os.environ, "CARGO_NET_OFFLINE": "true"}
    print("Building fuzz targets (nightly, release with debug assertions)", flush=True)
    build = subprocess.run(["cargo", "+nightly", "fuzz", "build", *BUILD], cwd=FUZZ, env=env,
                           capture_output=True, text=True, errors="replace")
    if build.returncode != 0:
        print(build.stderr[-4000:])
        print("Building the fuzz targets failed.")
        return 1
    stamp = time.strftime("%Y%m%d-%H%M%S")
    results = []
    for name in selected:
        print(f"Fuzzing {name} for {arguments.time} s", flush=True)
        outcome = run_target(name, arguments.time, arguments.seed, arguments.keep,
                             FUZZ / "artifacts" / "smoke" / stamp / name, env)
        results.append(outcome)
        if not outcome["ok"]:
            print(outcome["log"])
            print(f"{name}: {outcome['failure']}; inputs: {', '.join(outcome['inputs']) or 'none saved'}")
            print(f"Minimize with: cd fuzz && cargo +nightly fuzz tmin {' '.join(BUILD)} {name} INPUT", flush=True)
    print()
    print(f"{'target':<10} {'result':<13} {'executions':>11} {'exec/s':>8} {'cov':>7} {'ft':>8} {'corpus':>7} "
          f"{'rss MB':>7} {'seconds':>8}")
    for r in results:
        result = "ok" if r["ok"] else r["failure"]
        print(f"{r['target']:<10} {result:<13} {r['executions']:>11} {r['per_second']:>8} {r['coverage']:>7} "
              f"{r['features']:>8} {r['corpus']:>7} {r['rss_mb']:>7} {r['seconds']:>8.1f}")
    failed = [r["target"] for r in results if not r["ok"]]
    total = sum(r["seconds"] for r in results)
    print(f"Fuzzed {len(results)} target(s) for {total:.0f} s in total; "
          + (f"failures in: {', '.join(failed)}." if failed else "no failures."))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
