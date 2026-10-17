"""Install isolated benchmark tools. These dependencies never enter the product."""
import argparse
import os
import tomllib
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def competitor_cache(root=ROOT):
    """Resolve the isolated cache, rejecting both repository descendants and ancestors."""
    cache = Path(os.environ.get("PAGURA_COMPETITORS_DIR", "~/.cache/pagura-competitors")).expanduser().resolve()
    root = root.resolve()
    # A linked worktree has a separate directory but the same clean-room boundary.
    git = root / ".git"
    roots = [root]
    if git.is_file():
        marker = git.read_text().strip()
        if marker.startswith("gitdir: "):
            metadata = (root / marker.removeprefix("gitdir: ")).resolve()
            if metadata.parent.name == "worktrees":
                roots.append(metadata.parent.parent.parent)
    if any(cache.is_relative_to(base) or base.is_relative_to(cache) for base in roots):
        raise ValueError("PAGURA_COMPETITORS_DIR must be outside the repository and worktree")
    return cache


CACHE = competitor_cache()


def run(args, cwd=ROOT, env=None):
    subprocess.run([str(a) for a in args], cwd=cwd, check=True, timeout=900, env=env)


def setup():
    CACHE.mkdir(parents=True, exist_ok=True)
    venv = CACHE / "venv"
    python = venv / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
    run([sys.executable, "-m", "venv", venv])
    run([python, "-m", "pip", "install", "-r", ROOT / "benchmarks/requirements.txt"])
    node = CACHE / "node"
    node.mkdir(exist_ok=True)
    lock = ROOT / "benchmarks/node-package-lock.json"
    package = ROOT / "benchmarks/node-package.json"
    if package.exists() and lock.exists():
        shutil.copyfile(package, node / "package.json")
        shutil.copyfile(lock, node / "package-lock.json")
        run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"], node)
    else:
        raise RuntimeError("Missing reviewed Node dependency lock")
    # Copy only our adapter and lock: Cargo never writes registry/build data in-tree.
    adapter = CACHE / "hayro-adapter"
    (adapter / "src").mkdir(parents=True, exist_ok=True)
    for relative in ("Cargo.toml", "Cargo.lock", "src/main.rs"):
        shutil.copyfile(ROOT / "benchmarks/adapters/hayro" / relative, adapter / relative)
    shutil.copyfile(ROOT / "rust-toolchain.toml", adapter / "rust-toolchain.toml")
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    run(["cargo", "+" + channel, "build", "--release", "--locked", "--manifest-path",
         adapter / "Cargo.toml", "--target-dir", CACHE / "hayro-target"], cwd=adapter,
        env={**os.environ, "CARGO_HOME": str(CACHE / "cargo-home")})
    if platform.system() == "Darwin":
        run(["swiftc", "-O", ROOT / "benchmarks/adapters/quartz.swift", "-o", CACHE / "quartz"])
    print("Competitors installed locally. Poppler must be supplied by your OS; Adobe needs a licensed adapter.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup"])
    parser.parse_args()
    setup()
