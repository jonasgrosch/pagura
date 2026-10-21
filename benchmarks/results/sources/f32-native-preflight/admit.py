#!/usr/bin/env python3
"""Finalize F32 native admission bindings once; does not launch any measurement."""
import argparse
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--request', required=True, type=Path)
    parser.add_argument('--request-sha256', required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    sys.path.insert(0, str(root / 'benchmarks'))
    from f32_interval_admission import NAME, RESULTS, ROOT, pin, pinned, require, verify_admission
    require(root == ROOT, 'Wrong helper checkout')
    request = pinned(args.request.resolve(), args.request_sha256)
    require(request['experiment'] == NAME and request['status'] == 'frozen-for-admission-check', 'Request not frozen')
    inputs = pin(request['preflight_bindings'])
    require(inputs['stage'] == 'preflight-inputs', 'Input-stage binding required')
    bindings = {**inputs, 'stage': 'native-admission',
                **{key: request[key] for key in ('preflight_bindings', 'native_preflight', 'quality', 'wasi', 'wasi_review')},
                'admission_request': {'path': str(args.request.resolve()), 'sha256': args.request_sha256}}
    verify_admission(bindings)
    output = RESULTS / (NAME + '-bindings.json')
    with output.open('x') as handle:
        json.dump(bindings, handle, indent=2)
        handle.write('\n')
    print(output)


if __name__ == '__main__':
    main()
