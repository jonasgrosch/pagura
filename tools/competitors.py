"""Install isolated benchmark tools. These dependencies never enter the product."""
import argparse
import json
import os
import tomllib
import platform
import shutil
import subprocess
from pathlib import Path

from benchkit.rustflags import repository_rustflags

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
# The hayro adapter's build record in the cache, read back into its engine identity.
HAYRO_BUILD = "hayro-build.json"


def run(args, cwd=ROOT, env=None):
    subprocess.run([str(a) for a in args], cwd=cwd, check=True, timeout=900, env=env)


def native_rustflags(channel, env):
    """The host triple and the repository's `.cargo/config.toml` flags for it, as round 5 builds Pagura."""
    toolchain = ["rustup", "run", channel]
    info = subprocess.check_output(toolchain + ["rustc", "-vV"], cwd=ROOT, env=env, text=True)
    host = next(line.removeprefix("host: ") for line in info.splitlines() if line.startswith("host: "))
    return host, repository_rustflags(toolchain, host, env)


def setup(selected=None):
    defaults = {"pdfium", "mupdf", "pdfjs", "hayro"} | ({"quartz"} if platform.system() == "Darwin" else set())
    selected = set(selected) if selected is not None else defaults
    if not selected or not selected <= {"pdfium", "mupdf", "pdfjs", "hayro", "quartz"}:
        raise ValueError("unknown or empty competitor selection")
    if "quartz" in selected and platform.system() != "Darwin":
        raise ValueError("CoreGraphics requires macOS")
    CACHE.mkdir(parents=True, exist_ok=True)
    if selected & {"pdfium", "mupdf"}:
        # uv.lock pins every transitive wheel by hash and .python-version the interpreter.
        uv = shutil.which("uv")
        if uv is None:
            raise RuntimeError("uv is required: https://docs.astral.sh/uv/getting-started/installation/")
        run([uv, "sync", "--locked", "--only-group", "bench"],
            env={**os.environ, "UV_PROJECT_ENVIRONMENT": str(CACHE / "venv")})
    if "pdfjs" in selected:
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
    if "hayro" in selected:
        # Copy only our adapter and lock: Cargo never writes registry/build data in-tree.
        adapter = CACHE / "hayro-adapter"
        (adapter / "src/bin").mkdir(parents=True, exist_ok=True)
        for relative in ("Cargo.toml", "Cargo.lock", "src/main.rs", "src/bin/capabilities.rs"):
            shutil.copyfile(ROOT / "benchmarks/adapters/hayro" / relative, adapter / relative)
        shutil.copyfile(ROOT / "rust-toolchain.toml", adapter / "rust-toolchain.toml")
        channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        # Pagura's native flags, passed explicitly: they replace inherited RUSTFLAGS, and the
        # isolated CARGO_HOME and out-of-tree adapter keep every other Cargo configuration away.
        env = {key: value for key, value in os.environ.items() if key not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")}
        host, flags = native_rustflags(channel, env)
        run(["cargo", "+" + channel, "build", "--release", "--locked", "--manifest-path",
             adapter / "Cargo.toml", "--target-dir", CACHE / "hayro-target"], cwd=adapter,
            env={**env, "CARGO_HOME": str(CACHE / "cargo-home"), "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)})
        (CACHE / HAYRO_BUILD).write_text(json.dumps({"host": host, "rustflags": flags, "rustflags_source": ".cargo/config.toml"}) + "\n")
    if "quartz" in selected:
        run(["swiftc", "-O", ROOT / "benchmarks/adapters/quartz.swift", "-o", CACHE / "quartz"])
        run(["swiftc", "-O", ROOT / "benchmarks/adapters/capability_quartz.swift", "-o", CACHE / "capability-quartz"])
    print("Competitors installed locally. Poppler must be supplied by your OS; Adobe needs a licensed adapter.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup"])
    parser.add_argument("--engines", help="comma-separated engines to install; Poppler is supplied by the OS")
    args = parser.parse_args()
    setup(args.engines.split(",") if args.engines is not None else None)
