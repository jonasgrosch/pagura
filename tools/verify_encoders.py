"""Check pagura-codec's zlib and PNG encoders against independent decoders.

Runs the `encoded_vectors` example, which encodes every codec fixture, and
decodes its output with tools/fixturegen/codec.py: Python's zlib for the
DEFLATE streams and a PNG reader written from the specification. Cargo
tests never start an interpreter (CONTRIBUTING.md, Conventions 2), so this
check runs from tools/check.py instead.
"""
import subprocess
import sys
from pathlib import Path

from fixturegen.codec import verify_encoded_batch

ROOT = Path(__file__).resolve().parents[1]
# The test profile reuses what tools/check.py's `cargo test --all-targets`
# already built, so the example costs no extra compilation there.
EXAMPLE = ["cargo", "run", "--locked", "--offline", "--quiet", "--profile", "test",
           "-p", "pagura-codec", "--example", "encoded_vectors"]
# Every encode-png fixture is encoded at two levels; fewer fixtures than this
# means the corpus or the example lost PNG coverage.
MIN_PNG_VECTORS = 20


def main() -> None:
    batch = subprocess.run(EXAMPLE, cwd=ROOT, check=True, stdout=subprocess.PIPE).stdout
    streams, images = verify_encoded_batch(batch)
    if streams == 0 or images < 2 * MIN_PNG_VECTORS:
        sys.exit(f"too few encoded vectors: {streams} zlib streams, {images} PNG images")
    print(f"Independent decoders verified {streams} zlib streams and {images} PNG images")


if __name__ == "__main__":
    main()
