"""Paired before/after render timings for exploratory profiling; no competitor or superiority claim.

Build render_profile at the baseline and save that executable, then build it at
HEAD. This script generates only synthetic PDFs in --output; real PDFs remain
read-only in --corpus. Each block has one warmup and five measured renders.
It alternates binary order across three blocks, verifies SHA-256 of every final
RGBA output, and retains raw stage timings and source/binary digests.
"""
import argparse
import hashlib
import json
import platform
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools"))
from fixturegen.pdf import single_page


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(before, after, corpus, output):
    output.mkdir(parents=True, exist_ok=True)
    triangles=[]
    for i in range(50_000):
        x=(i % 250)*3
        y=(i // 250)*3
        triangles.append(f"{x} {y} m {x+2} {y} l {x+1} {y+2} l h f\n".encode())
    (output / "triangles.pdf").write_bytes(single_page([b"".join(triangles)],media_box=b"[0 0 750 600]"))
    (output / "dashed.pdf").write_bytes(single_page([b"1 w [2 2] 0 d 1 16 m 8191 16 l S"],media_box=b"[0 0 8192 32]"))
    cases=[
        ("rectangles",ROOT / "corpus/generated/classic/many-rectangles.pdf",0,300),
        ("triangles",output / "triangles.pdf",0,72),
        ("dashed",output / "dashed.pdf",0,72),
        ("real114",corpus / "0000114.pdf",1,72),
        ("real556",corpus / "0000556.pdf",0,72),
        ("real786",corpus / "0000786.pdf",2,72),
    ]
    report={"method":"one warmup plus five measured renders per process; three blocks with alternating binary order; interpretation outside timing; hashing and output outside timing", "environment":platform.platform(),"machine":platform.machine(),"before_binary_sha256":digest(before),"after_binary_sha256":digest(after),"cases":[]}
    for name,path,page,dpi in cases:
        samples={"before":[],"after":[]}
        hashes={"before":[],"after":[]}
        for block in range(3):
            order=[("before",before),("after",after)]
            if block % 2:
                order.reverse()
            for stage,binary in order:
                rgba=output / f"{name}-{stage}.rgba"
                result=subprocess.run([str(binary),str(path),str(page),str(dpi),"5",str(rgba)],capture_output=True,text=True,check=True)
                raw=output / f"{name}-{stage}-{block}.txt"
                raw.write_text(result.stdout)
                rows=[line.split() for line in result.stdout.splitlines()]
                assert len(rows)==5
                assert len({row[2] for row in rows})==1, "non-deterministic pixels within block"
                samples[stage].extend(float(row[1]) for row in rows)
                hashes[stage].append(digest(rgba))
        for stage, values in hashes.items():
            assert len(set(values)) == 1, f"non-deterministic pixels across {stage} blocks"
        before_pixels=(output / f"{name}-before.rgba").read_bytes()
        after_pixels=(output / f"{name}-after.rgba").read_bytes()
        assert len(before_pixels)==len(after_pixels), "surface size changed"
        different=before_pixels!=after_pixels
        changed=sum(a!=b for a,b in zip(before_pixels,after_pixels)) if different else 0
        maximum=max(abs(a-b) for a,b in zip(before_pixels,after_pixels)) if different else 0
        entry={"name":name,"input_sha256":digest(path),"page_index":page,"dpi":dpi,"rgba_sha256_before":hashes["before"][0],"rgba_sha256_after":hashes["after"][0],"verified_exact":not different,"changed_bytes":changed,"max_absolute_error":maximum,"samples_seconds":samples,"median_seconds":{stage:statistics.median(values) for stage,values in samples.items()}}
        report["cases"].append(entry)
        print(name, *(f"{entry['median_seconds'][stage]*1000:.4f} ms" for stage in ("before","after")), "EXACT" if not different else f"CHANGED ({changed} bytes, max {maximum})",flush=True)
    (output / "profile.json").write_text(json.dumps(report,indent=2)+"\n")
    if any(not case["verified_exact"] for case in report["cases"]):
        print("Output verification FAILED for changed cases; timings are diagnostic only.",file=sys.stderr)
        raise SystemExit(2)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before",type=Path,required=True)
    parser.add_argument("--after",type=Path,required=True)
    parser.add_argument("--corpus",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    run(args.before.resolve(),args.after.resolve(),args.corpus.resolve(),args.output.resolve())
