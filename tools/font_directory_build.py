"""Build source-owned explicit Repair probes, one fresh compiler cache at a time.

Native and WASI parity only; no timings, competitor or memory claims. Run under
an exclusive qualification lease when used by a local qualification worker.
"""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/font-directory-frozen"
CACHE = ROOT / "target/font-directory-build"
WASM = "wasm32-wasip1"
VARIANTS = {
    "native": None,
    "simd": "-C\x1ftarget-feature=+simd128\x1f-C\x1foverflow-checks=on",
    "scalar": "-C\x1ftarget-feature=-simd128\x1f-C\x1foverflow-checks=on",
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main():
    if (ROOT / "target").is_symlink() or OUTPUT.exists() or CACHE.exists() or OUTPUT.is_symlink() or CACHE.is_symlink():
        raise ValueError("probe output and compiler cache must be fresh")
    if any(key.startswith(("CARGO_PROFILE_", "CARGO_TARGET_")) or key in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN"} for key in os.environ):
        raise ValueError("probe compiler flags and cache are owned by this builder")
    revision = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    if git("status", "--porcelain"):
        raise ValueError("probe requires a clean exact source checkout")
    OUTPUT.mkdir(parents=True)
    compiler = subprocess.check_output(["rustc", "-vV"], text=True)
    sources = {p: sha(ROOT / p) for p in (
        "Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml",
        "crates/pagura/examples/font_directory_probe.rs", "tools/font_directory_build.py",
        "tools/font_directory_parity.mjs", "corpus/manifests/font_directory.json",
    )}
    modules = {}
    for variant, flags in VARIANTS.items():
        if shutil.disk_usage(ROOT).free < 4 * 1024**3:
            raise ValueError("probe build requires four GiB physical headroom")
        if CACHE.exists():
            raise ValueError("preceding probe cache was not reclaimed")
        env = {**os.environ, "CARGO_TARGET_DIR": str(CACHE), "CARGO_BUILD_JOBS": "2",
               "PAGURA_SOURCE_REVISION": revision}
        if flags is not None:
            env["CARGO_ENCODED_RUSTFLAGS"] = flags
        command = ["cargo", "build", "--locked", "--offline", "--release", "--verbose", "-j2", "-p", "pagura",
                   "--example", "font_directory_probe"]
        if variant != "native":
            command += ["--target", WASM]
        (OUTPUT / f"{variant}-pending.json").write_text(json.dumps({"command": command,
            "source_revision": revision, "flags": flags, "features": "pagura defaults",
            "profile": "unchanged source release", "compiler": compiler}, indent=2) + "\n")
        try:
            actual = subprocess.run(command, cwd=ROOT, env=env, capture_output=True)
            # Preserve actual build completion before checking its result or module.
            (OUTPUT / f"{variant}.stdout.log").write_bytes(actual.stdout)
            (OUTPUT / f"{variant}.stderr.log").write_bytes(actual.stderr)
            (OUTPUT / f"{variant}-completed.json").write_text(json.dumps({"command": command,
                "exit": actual.returncode}, indent=2) + "\n")
            if actual.returncode != 0:
                raise ValueError(f"{variant} build failed: {actual.returncode}")
            suffix = ".wasm" if variant != "native" else ".exe" if sys.platform == "win32" else ""
            built = CACHE / (WASM if variant != "native" else "") / "release/examples" / f"font_directory_probe{suffix}"
            frozen = OUTPUT / f"{variant}{suffix}"
            shutil.copy2(built, frozen)
            if sha(built) != sha(frozen):
                raise ValueError("frozen module differs from compiler output")
            modules[variant] = {"path": frozen.relative_to(ROOT).as_posix(), "sha256": sha(frozen),
                                "rustflags": flags, "source_revision": revision}
        finally:
            # Only this helper's known fresh target; child is terminal above.
            if CACHE.is_symlink():
                raise ValueError("compiler cache unexpectedly became a symlink")
            if CACHE.exists():
                shutil.rmtree(CACHE)
    if git("rev-parse", "HEAD") != revision or git("status", "--porcelain"):
        raise ValueError("source changed during probe builds")
    if any(sha(ROOT / p) != digest for p, digest in sources.items()):
        raise ValueError("probe build input changed")
    (OUTPUT / "modules.json").write_text(json.dumps({"schema": 1, "source_revision": revision,
        "source_tree": tree, "compiler": compiler, "sources": sources, "modules": modules,
        "profile": "unchanged source release", "features": "pagura defaults",
        "single_compiler_cache_reclaimed": True}, indent=2) + "\n")
    print("Frozen native/SIMD/scalar explicit Repair modules; compiler cache reclaimed")


if __name__ == "__main__":
    main()
