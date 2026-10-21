# Version 2 launcher correction — unexecuted

This is a new auditor revision for the same retained diagnostic. `audit.py`,
`test_audit.py`, the first failed audit, producer, original pins, and all 5,656
outputs stay unchanged. No rerender, resampling or threshold change is needed.
The coordinator must preserve the failed attempt and use new pins and output
paths for the revised audit. The author has not executed or imported this code.

The first auditor required `sys.orig_argv` to equal the requested command. On
this framework Python installation, the requested `bin/python3.14` launcher
hands execution to `Resources/Python.app/Contents/MacOS/Python`, replacing only
`argv[0]`. The upstream [CPython 3.14 framework launcher source](https://raw.githubusercontent.com/python/cpython/3.14/Mac/Tools/pythonw.c)
sets `__PYVENV_LAUNCHER__`, sets `argv[0] = exec_path`, and uses
`POSIX_SPAWN_SETEXEC` (or `execve`). This source explains the observed mechanism;
it does not establish which app bytes ran historically.

The requested launcher was pinned before execution. The actual app executable
was omitted from that closure. Its new pin is explicitly **post-run**. The
result now includes `auditor_revision: 2`, `runtime_handoff_retrospective: true`,
the external `python_launch` object, and a limitation that current hashes do not
prove the historical app bytes. This is a qualified retained-evidence check, not
a repair of the prelaunch omission. Original performance/resource misses remain.

Keep all original v1 pin fields and add exactly this object, independently
reviewing and hashing the new manifest before the coordinator runs the auditor:

```json
{
  "python_launch": {
    "requested_argv0": "/opt/homebrew/bin/python3",
    "runtime_executable": {
      "path": "/opt/homebrew/Cellar/python@3.14/3.14.7/Frameworks/Python.framework/Versions/3.14/bin/python3.14",
      "sha256": "d8f1d508de5acfd500f20d1949528375ff4a1f470efb267f00d1770941cdeee3"
    },
    "actual_argv0": {
      "path": "/opt/homebrew/Cellar/python@3.14/3.14.7/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python",
      "sha256": "c86f5b1bece373ce5b670f9816630d7002c9e64525f4930fb035ecc8b7bd9345"
    },
    "actual_pin_timing": "post-run"
  }
}
```

The supplied hashes above are the coordinator's separately inspected values;
the auditor rehashes both current files. It requires the requested path to
resolve to the original runtime executable, that executable's hash to agree
with the original plan closure, the exact same framework version prefix for
the app, canonical absolute executable paths, and exact `argv[1:]`. It verifies
that the app was absent from the original closure rather than inventing a
prelaunch pin. The guard command and outer receipt chain still use the exact
requested invocation. Renderer arguments, images, logs, schedule, summaries,
source/build checks and final receipt bindings are unchanged from v1.

Use `audit-v2.py` with the same CLI as v1 and new pin/output paths.
`test_audit_v2.py` contains eight new bounded component tests using inert files
and symlinks: the framework handoff, every remaining argument, requested and
observed identities, framework prefix/version/suffix, both executable byte
pins, launcher resolution, original closure and retrospective scope, and
missing/malformed metadata. They launch no process. These supplement the eight
original tests; they are not a complete synthetic campaign audit.
