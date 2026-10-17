# Security posture

Research bootstrap. Do not deploy against hostile production documents yet.

All product Rust targets forbid unsafe code and currently use no external Cargo
dependencies. This reduces one class of bugs; it does not prove correctness or
prevent resource exhaustion. Parsing, traversal and rendering have explicit limits.
Host filesystem/network/script actions are not available during interpretation.

The CLI limits file size before allocation and propagates errors. Passwords,
JavaScript, XFA, external resources and unsupported graphics are not executed.
Output is written only after complete successful processing; callers choose paths.

Before deployment: implement process sandboxing and cancellation; complete
coverage-guided fuzzing and malicious corpus testing; review all codecs and
signature paths; validate platform resource limits. Report vulnerabilities
privately through GitHub's "Report a vulnerability" (private security advisories)
on the repository, not in public issues.
