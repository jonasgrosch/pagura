# Retained execution sources

These files preserve the integration helpers and patches byte-for-byte. They
refer to their original local result paths and are an execution archive, not
an installable tool. Failed and canceled attempts are intentionally included.

`full_check_v2.py` validated `final-inputs.json`. Later comment-only corrections
are bound by `post-doc-delta.json` and `post-doc-inputs.json`; `post_docs.py`
validated their formatting and public documentation. The original helper reports
are not rewritten to repair stale running-receipt pins.

The retrospective collector `build_manifest.py` preserves final outer receipts
and local raw evidence. It was run before the comment changes. Helpers outside
the runner source digest were not independently pinned before their historical
execution; the trusted local coordinator remains part of this evidence boundary.
