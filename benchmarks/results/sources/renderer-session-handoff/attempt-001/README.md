# Initial archive export, not published

The first stream verifier rejected a hard-link member. tarfile.gettarinfo
recognized repeated inodes and emitted links; the writer had incorrectly
recorded empty-stream hashes for those entries. No scientific source or
measurement changed. This attempt is not the publication package. Original
archives remain private in /private/tmp/pagura-renderer-session-archive-20261021.
The corrected exporter forces ordinary regular-file payloads and regenerates
all assets, then independently verifies every included payload before upload.
