# Untimed native output preflight

`run.py` executes the fixed 52 native cells once on each frozen binary: 104
outputs. It uses the existing fixture generator, independent image oracle,
source identity and owned-child process helper. Each raw output must have the
expected dimensions, exact independent RGB, alpha 255 and same-mode byte parity.
Failures remain in the report; there are no retries or timing statistics.

Freeze the ordinary experiment plan and bindings first. Include this helper
and every imported benchmark helper in `bindings.bound_files_sha256`. Use a new
output directory and the existing serial runner and strict observer:

```sh
ABS_PYTHON -B ABS_SCRIPT --root ABS_CANDIDATE \
  --bindings ABS_BINDINGS --bindings-sha256 SHA --output ABS_NEW_DIRECTORY
```

The coordinator must separately authenticate completed build receipts and
the final preflight receipt/guard. Require `verified-native-preflight`, all 104
records verified, unchanged identities and every retained artifact hash.
Raw process metrics are incidental diagnostics and establish no speed result.
The helper was independently source-reviewed; its actual matrix run is pending.
