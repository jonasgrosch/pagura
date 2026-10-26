# Document-service CLI

`pagura --help` lists all commands. These commands expose the library's metadata
and catalog attachment views without altering the PDF:

```sh
pagura metadata input.pdf
pagura attachments input.pdf
pagura extract-metadata input.pdf packet.xmp
pagura extract-attachment input.pdf 0 UF payload.bin
```

All four accept `--strict` and `--password TEXT`. Reported repairs are enabled by
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
extraction. MIME names and sizes are declarations, not verified properties.

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
checksums, editing and host/viewer integration remain in D02.

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
