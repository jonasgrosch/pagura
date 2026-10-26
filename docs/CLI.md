# CLI commands

`pagura --help` lists all commands. These commands expose the library's metadata
and catalog attachment views without altering the PDF:

```sh
pagura metadata input.pdf
pagura attachments input.pdf
pagura extract-metadata input.pdf packet.xmp
pagura extract-attachment input.pdf 0 UF payload.bin
pagura check-attachment input.pdf 0 UF
```

All five accept `--strict` and `--password TEXT`. Reported repairs are enabled by
default; strict mode rejects them. Success exits 0, input/operation/output errors
exit 1, and malformed command arguments exit 2. Extraction produces no stdout.
JSON readers should check `schema` and ignore additional fields within that
schema; removing fields or changing their meaning requires a new schema.

## Metadata JSON (schema 1)

`metadata` prints one JSON object with `info`, `language`, `metadata_object`,
`repairs` and `information`.

- `info` is null when the trailer has no Info dictionary. Otherwise it contains
  `object` and the standard `Title`, `Author`, `Subject`, `Keywords`, `Creator`,
  `Producer`, `CreationDate`, `ModDate` and `Trapped` fields. Text values are
  decoded PDF strings or null when absent; empty strings remain empty.
  `Trapped` is `True`, `False` or `Unknown`, preserving the document's declaration.
- `language` is the catalog language string or null. Dates and language tags are
  preserved without semantic validation.
- `metadata_object` is the catalog metadata stream identity or null. Object
  identities use `{"number": 6, "generation": 0}`; a direct Info dictionary has
  a null `object`. The metadata report does not decode the packet.
- `repairs` and `information` are arrays of diagnostic tags from the operation.

Info fields and XMP remain separate sources. `extract-metadata` decodes the catalog
stream, including its packet wrapper and original encoding, to the requested
path. It returns an error when the stream is absent and an empty file when the
stream exists with zero decoded bytes. Packet contents remain inert; the command
does not parse XML, resolve entities, reconcile Info/XMP or validate schemas.

## Attachment JSON (schema 1)

`attachments` prints `files`, `repairs` and `information`. `files` is the catalog
EmbeddedFiles name tree in byte-key order, with zero-based `index` values for
the extraction command. Each record contains:

| Field | Meaning |
|---|---|
| `index`, `key_hex`, `object` | Listing position, original name-tree key bytes and file-specification object identity. |
| `names` | Decoded `UF` and original `F_hex`, `DOS_hex`, `Mac_hex`, `Unix_hex` bytes. Missing values are null. |
| `description`, `file_system_hex` | Decoded description and original file-system name bytes. |
| `relationship`, `relationship_hex` | AFRelationship name as UTF-8 when representable and as original bytes; absent relationships default to `Unspecified`. |
| `volatile` | File-specification volatility declaration. |
| `payloads` | EF entries, each with `key`, `object`, `media_type_hex` and `declared_size`. |

All `_hex` strings use two lowercase hexadecimal digits per byte. They preserve
non-UTF-8 bytes; decoded Unicode strings are ordinary JSON strings. Tree keys,
filenames and payload identities are distinct. Payloads are decoded only during
extraction or an explicit parameter check. MIME names and sizes in the listing
are unverified declarations.

`extract-attachment INPUT INDEX KEY OUTPUT` selects the listed entry and its
exact EF key (`F`, `UF`, `DOS`, `Mac` or `Unix`). It never guesses between F and UF.
An absent index/key or a file specification without that embedded stream fails.
Run the listing against the same input bytes used for extraction.

The output path always comes from the command line. Embedded paths and URLs are
never followed or used to name output files. Authorization and bounded stream
decoding complete before the destination is created; an existing file is never
overwritten. Output IO failures are reported and may leave a partial new file.

These commands currently cover catalog metadata and EmbeddedFiles only. The
library's object-local metadata, AF arrays and RF related files are not CLI
selection targets yet. Annotation attachment discovery, streaming extraction,
editing and host AF/RF selection remain in D02. Protocol 1.8 and the viewer
already support catalog listing and extraction.

## Attachment parameter checks (schema 1)

`check-attachment INPUT INDEX KEY` uses the same catalog index and exact EF key
as extraction. It completely decodes and authorizes the payload before writing
JSON, even when Size and CheckSum are absent. The report contains `schema: 1`,
`index`, `key`, the payload `object`, actual `decoded_size`, `size`, `checksum`,
`repairs` and `information`. Both check fields are `match`, `mismatch` or
`not-declared`; a missing declaration is never reported as a successful comparison.

Size is compared with the uncompressed byte length. CheckSum is compared with
MD5 over decrypted bytes after all filters, per ISO 32000-2 Table 45. This checks
producer declarations, not authenticity, file content, MIME types or dates.
The library exposes the same observations through `EmbeddedFile::check_parameters`.

A mismatch produces the complete JSON report, an explanatory stderr message and
exit 1. Matching or absent declarations produce exit 0. Authorization, selection,
filter and resource errors produce exit 1 without successful check JSON. No
payload file is created and source bytes remain unchanged. Decoding is bounded
but whole-buffer. Protocol 1.10 and viewer Check controls expose the same
observations; streaming remains open.

## Inspection geometry (schema 2)

`inspect INPUT` retains page/object counts, original byte length and diagnostics.
Its additive `page_geometry` array follows page order. Each record gives
`media_box` and `crop_box` as normalized `[x0,y0,x1,y1]` coordinates in default
user space, `rotation` in clockwise degrees and `user_unit` in units of 1/72 inch.
The crop box is already intersected with the media box by the document view.
These are source geometry values, without pixel rounding. For displayed sizes,
subtract the box endpoints, multiply by UserUnit and exchange width/height for
90/270-degree rotation. Different rendering APIs may select different boxes;
G01's current baseline fixtures use equal media/crop boxes and default transforms.

## Page images

```sh
pagura render input.pdf page.ppm
pagura render --format png input.pdf page.png
pagura render --format png --transparent input.pdf transparent.png 144 0
pagura convert --format png input.pdf existing-output-directory 144 0 3
```

`render` writes one zero-based page (default 0); `convert` writes all pages or an
inclusive zero-based range. DPI defaults to 72. Directory outputs are numbered
from 1 as `page-0001.ppm` or `page-0001.png`; the directory must already exist.
`--format ppm|png` selects the encoding explicitly. PPM is the default regardless
of the filename extension. `--transparent` requires PNG; otherwise both formats
render over opaque white. Password, strict-mode and overprint options also apply.

PNG contains straight RGBA8 pixels; the rendered premultiplied samples are
converted in place, and fully transparent pixels have zero RGB. Output is
non-interlaced with no ancillary color/profile metadata. Page recording, rendering
and bounded PNG encoding finish before a new destination is created. Existing
files are never overwritten; an IO error can leave a partial new file. In a
multi-page conversion, previously completed files remain if a later page fails.

PNG currently retains one full page surface and encoder scratch. PPM continues
to stream renderer bands. `--timings` separates document opening, rendering and
image encoding/writing; it does not establish a performance claim.

Library callers can render a page or tile with the existing APIs and call
`pagura::png::encode_pixmap(surface, pagura::png::Level::Default, &limits)`. The
surface is consumed on success or failure. `pagura::png::encode` also accepts
caller-owned Gray, RGB or straight RGBA buffers and encoding options. Incremental
PNG and resumable jobs remain D02 work. Protocol 1.9 and the viewer expose a
separate complete-page PNG export path, described in the [host protocol](../crates/pagura-host/PROTOCOL.md#version-19-complete-png-page-export).
