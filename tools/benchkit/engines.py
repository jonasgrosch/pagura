"""External engine commands, identities and isolated historical builds."""
import json
import os
import shutil
import subprocess
import tempfile
import tomllib
from contextlib import contextmanager
from pathlib import Path
from competitors import CACHE
from .config import ROOT, ADAPTERS, sha


def engines():
    python = CACHE / "venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    exe = ".exe" if os.name == "nt" else ""
    return {
        "pagura": [str(ROOT / ("target/release/pagura" + exe))],
        "poppler": [os.environ.get("PAGURA_POPPLER") or shutil.which("pdftoppm") or "pdftoppm"],
        "pdfium": [str(python), str(ADAPTERS / "python_engine.py"), "pdfium"],
        "mupdf": [str(python), str(ADAPTERS / "python_engine.py"), "mupdf"],
        "pdfjs": [shutil.which("node") or "node", str(ADAPTERS / "pdfjs.mjs")],
        "hayro": [os.environ.get("PAGURA_HAYRO") or str(CACHE / ("hayro-target/release/pagura-hayro-competitor" + exe))],
        "quartz": [os.environ.get("PAGURA_QUARTZ") or str(CACHE / "quartz")],
    }


def locked_group(lock, group="bench"):
    """uv.lock entries one dependency group resolves to, transitively, in canonical JSON.

    Engine identity hashes only these, so bumping a lint tool leaves it unchanged.
    """
    data = tomllib.loads(Path(lock).read_text())
    packages = {}
    for package in data["package"]:
        packages.setdefault(package["name"], []).append(package)
    root = next(p for p in data["package"] if p.get("source", {}).get("virtual") == ".")
    pending = list(root.get("dev-dependencies", {}).get(group, []))
    selected = {}
    while pending:
        entry = pending.pop()
        for package in packages[entry["name"]]:
            key = (package["name"], package.get("version"))
            if key not in selected:
                selected[key] = package
                pending.extend(package.get("dependencies", []))
            for extra in entry.get("extra", []):
                pending.extend(package.get("optional-dependencies", {}).get(extra, []))
    return json.dumps([selected[key] for key in sorted(selected)], sort_keys=True).encode()


def identify(name, command):
    try:
        result = subprocess.run(command + (["-v"] if name == "poppler" else ["--version"]),
                                capture_output=True, text=True, timeout=20, cwd=ROOT)
        if result.returncode:
            raise ValueError((result.stdout + result.stderr)[-2000:])
        files = [Path(p) for p in command if Path(p).is_file()]
        if name in ("pdfium", "mupdf"):
            for pattern in ("*.so", "*.dylib", "*.pyd"):
                files.extend((CACHE / "venv").rglob(pattern))
        if name == "pdfjs":
            files.append(ROOT / "benchmarks/node-package-lock.json")
            files.append(CACHE / "node/node_modules/pdfjs-dist/legacy/build/pdf.mjs")
            for pattern in ("*.node", "*.wasm"):
                files.extend((CACHE / "node/node_modules").rglob(pattern))
        if name == "hayro":
            files.append(ADAPTERS / "hayro/Cargo.lock")
        artifacts = {str(p): sha(p.read_bytes()) for p in sorted(set(files))}
        if name in ("pdfium", "mupdf"):
            artifacts[str(ROOT / "uv.lock") + "#bench"] = sha(locked_group(ROOT / "uv.lock"))
        return {"status": "available", "version": (result.stdout + result.stderr).strip(),
                "command": command, "artifacts": artifacts}
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return {"status": "unavailable", "reason": str(error), "command": command,
                "hint": ("build Pagura offline with cargo build --release --locked --offline -p pagura" if name == "pagura"
                         else "supply pdftoppm via PAGURA_POPPLER or PATH" if name == "poppler"
                         else "explicit network setup required: tools/competitors.py setup; cache: " + str(CACHE))}


def command_for(name, base, case, output):
    source = str(ROOT / case["path"])
    page = str(case.get("page", 0) + 1)
    base = password_command(name, base, case)
    if name in ("pagura", "revision"):
        return base + ["render", source, str(output), str(case["dpi"]), str(case.get("page", 0))]
    if name == "poppler":
        return base + ["-f", page, "-l", page, "-singlefile", "-r", str(case["dpi"]), source, str(output.with_suffix(""))]
    return base + [source, str(output), str(case["dpi"]), str(case.get("page", 0))]


def conversion_command(name, base, case, output):
    command = command_for(name, base, case, output)
    if name in ("pagura", "revision"):
        command.insert(len(password_command(name, base, case)) + 1, "--timings")
    return command


@contextmanager
def revision_engine(revision, repository=ROOT):
    """Build a detached, temporary out-of-tree worktree, offline; always remove it.

    Git resolves an exact commit before any build. The requested revision is trusted
    build input supplied by the operator, never obtained from a PDF or manifest.
    """
    commit = subprocess.check_output(["git", "rev-parse", "--verify", "--end-of-options", revision + "^{commit}"],
                                     cwd=repository, text=True).strip()
    with tempfile.TemporaryDirectory(prefix="pagura-regression-") as directory:
        worktree = Path(directory) / "source"
        target = Path(directory) / "target"
        if worktree.resolve().is_relative_to(repository.resolve()):
            raise ValueError("regression worktree must be outside the repository")
        subprocess.run(["git", "worktree", "add", "--detach", str(worktree), commit], cwd=repository, check=True)
        try:
            channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
            subprocess.run(["cargo", "+" + channel, "build", "--release", "--locked", "--offline", "-p", "pagura", "--target-dir", str(target)],
                           cwd=worktree, check=True)
            binary = target / "release" / ("pagura.exe" if os.name == "nt" else "pagura")
            yield [str(binary)], commit
        finally:
            subprocess.run(["git", "worktree", "remove", "--force", str(worktree)], cwd=repository, check=True)



def password_command(name, base, case):
    """Forward the manifest credential without shell interpolation or guessing its role."""
    if "password" not in case:
        return list(base)
    password = case["password"]
    if not isinstance(password, str):
        raise ValueError("password must be a string")
    if name == "poppler":
        return base + ["-upw", password, "-opw", password]
    return base + ["--password", password]
