"""Stage the public viewer without exposing corpus, personal files or build caches."""
import argparse
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def package(destination: Path, wasm: Path):
    """Copy public assets to a new directory, refusing an existing deployment."""
    with wasm.open("rb") as source:
        if source.read(8) != b"\0asm\x01\0\0\0":
            raise ValueError("Expected a WebAssembly module")
    destination.mkdir(parents=True, exist_ok=False)
    for path in (ROOT / "web").iterdir():
        if path.is_file() and path.suffix in (".mjs", ".html", ".css"):
            shutil.copyfile(path, destination / path.name)
    shutil.copyfile(wasm, destination / "pagura-host.wasm")
    (destination / ".nojekyll").touch()
    notices = [ROOT / "LICENSE-APACHE", ROOT / "LICENSE-MIT", ROOT / "data/README.md", ROOT / "data/afm/MustRead.html"]
    notices += [p for p in (ROOT / "data").rglob("*") if p.is_file() and (p.name.startswith("LICENSE") or p.name == "NOTICE")]
    for path in notices:
        target = destination / "licenses" / path.relative_to(ROOT)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--wasm", type=Path, default=ROOT / "target/wasm32-wasip1/release/pagura-host.wasm")
    args = parser.parse_args()
    package(args.destination, args.wasm)
