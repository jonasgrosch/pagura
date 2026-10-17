# Official references

`lock.json` records actual acquisition, hashes and missing access.
`normative-index.json` inventories every normative paragraph with links from the
PDF Association's current official index. `requirements.json` maps implemented
tests to requirements; it must not be confused with comprehensive conformance.

Core: ISO 32000-2:2020 and applicable errata; ISO 32000-1:2008/Adobe PDF 1.7 for
legacy behavior. Profiles: PDF/A (ISO 19005), PDF/X (ISO 15930), PDF/UA (ISO 14289).
Extensions: ISO/TS 32001-32005 and other applicable extensions listed by ISO.
Referenced standards include ICC, fonts/CMaps, JPEG/JPEG2000/JBIG2/CCITT,
cryptography, metadata and signatures. PAdES supplements PDF signature support.

Authority order: applicable normative text and ISO-approved corrections;
explicitly labeled industry resolutions; published test descriptions; observed
implementation behavior. Open questions stay unresolved. Do not treat new
versions of dated normative references as automatically applicable.

Full documents are private local research material in `$PAGURA_REFERENCES_DIR`
(default `~/.cache/pagura-references`, shared by all checkouts) or
`references/private/`, not redistributed in Git. Public availability is not a
redistribution license. ISO sponsored documents require the publisher's normal
acquisition process. No account creation, purchase or terms acceptance is automated.

Refresh: `python3 tools/references.py sync`. This changes evidence and must be
reviewed; normal builds/tests never use the network. Missing full text remains
explicit even if the public catalogue page was successfully cached.
