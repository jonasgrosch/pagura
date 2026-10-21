"""Offline product checks, identical locally and in CI after toolchain installation."""
import argparse
import os
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARGO = ["cargo"]
OFFLINE = ["--locked", "--offline"]
COMMANDS = [
    ([sys.executable, "tools/policy.py"], {}),
    ([sys.executable, "tools/fixtures.py", "--check"], {}),
    ([sys.executable, "tools/bundled_data.py", "verify"], {}),
    (CARGO + ["fmt", "--all", "--", "--check"], {}),
    (CARGO + ["clippy", *OFFLINE, "--workspace", "--all-targets", "--", "-D", "warnings"], {}),
    (CARGO + ["test", *OFFLINE, "--workspace", "--all-targets"], {}),
    (CARGO + ["test", *OFFLINE, "--workspace", "--doc"], {}),
    (CARGO + ["doc", *OFFLINE, "--workspace", "--no-deps"], {"RUSTDOCFLAGS": "-D warnings"}),
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--target", "wasm32-wasip1"], {}),
    # Matching opt-in font builds exercise informational diagnostics without
    # changing the default WASI host's data footprint.
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target-dir", "target/host-bundled"], {}),
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target", "wasm32-wasip1", "--target-dir", "target/host-bundled"], {}),
    # Native/WASI parity, the browser shim and the viewer harness (Node, no npm packages).
    (["node", "tools/host_test.mjs"], {}),
    ([sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"], {}),
    ([sys.executable, "tools/conformance.py"], {}),
]


def planned_commands(jobs: int | None = None) -> list[tuple[list[str], dict[str, str]]]:
    """Return fresh argv/environment overrides without inspecting or running tools."""
    if jobs is not None and (type(jobs) is not int or jobs < 1):
        raise ValueError("jobs must be a positive integer")
    commands = []
    for original, extra_env in COMMANDS:
        command, environment = list(original), dict(extra_env)
        if jobs is not None:
            # Inherited by nested Cargo invocations, including conformance's build.
            # Rust test harness concurrency is independent of build concurrency.
            environment.update(CARGO_BUILD_JOBS=str(jobs), RUST_TEST_THREADS=str(jobs))
            if command[:2] == [sys.executable, "tools/conformance.py"]:
                command.extend(["--jobs", str(jobs)])
        commands.append((command, environment))
    return commands


def positive_jobs(value: str) -> int:
    try:
        jobs = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("jobs must be a positive integer") from error
    if jobs < 1:
        raise argparse.ArgumentTypeError("jobs must be a positive integer")
    return jobs


def describe_command(command: list[str], environment: dict[str, str]) -> str:
    """Shell-quoted display only; execution always passes argv directly."""
    overrides = [f"{key}={value}" for key, value in sorted(environment.items())]
    return shlex.join((["env", *overrides] if overrides else []) + command)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=("--jobs does not reserve a runner or guarantee process-wide single-thread "
                "execution. Host/browser checks still create worker threads. "
                "Commands inherit the environment plus the displayed overrides."),
    )
    parser.add_argument(
        "--jobs", type=positive_jobs, metavar="N",
        help=("limit Cargo build jobs, Rust test harness threads and conformance workers "
              "to positive N; omit to preserve their existing defaults/environment"),
    )
    parser.add_argument(
        "--dry-run", action="store_true",
        help="print the working directory, commands and environment overrides without executing them",
    )
    arguments = parser.parse_args(argv)
    commands = planned_commands(arguments.jobs)
    if arguments.dry_run:
        print(f"Dry run: no commands executed. Working directory: {ROOT}")
        print("Environment: inherited, with the overrides shown below.")
    for command, extra_env in commands:
        print(("Planned: " if arguments.dry_run else "Checking: ")
              + describe_command(command, extra_env), flush=True)
        if not arguments.dry_run:
            subprocess.run(command, cwd=ROOT, check=True, env={**os.environ, **extra_env})
    if not arguments.dry_run:
        print("All checks passed. This is not a complete PDF conformance or performance certification.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
