# ADR 0021: `pagura-crypto` exposes hashes and verification; ciphers stay internal

Status: accepted, 2026-10-18. Refines ADR 0015's use of the crate.

## Context

`pagura-crypto` described itself as "not a general-purpose cryptography
library", yet exported `aes::Aes`, `aes::unpad_pkcs7`, `Rc4`, every SHA type
and free `md5`/`sha*` functions. Outside the crate only `Md5` (ICC profile IDs
in `pagura-render`) and `standard::*` (the document layer) were used. ADR 0015
then added SHA-1, SHA-224, RSA and ECDSA verification, which the signature
layer consumes through `HashAlgorithm`, the hash types and `Verification`.

A review found three further problems:

- **Duplicated hashing.** Six hashers each repeated buffering, length
  counting and padding; SHA-224/256 and SHA-384/512 differed only in initial
  values and output length. There was no trait, so every caller that chose a
  hash at run time wrote its own dispatch macro.
- **Validate-then-index.** `SecurityParams` was checked by a function that
  returned `()`, and the algorithms then sliced its `Vec`s on the strength of
  that check; any new entry point that skipped it would panic on hostile
  input.
- **Inconsistent key hygiene.** `FileKey` derived `PartialEq`, the hashers'
  `Debug` printed buffers holding password bytes, and nothing was cleared.

## Decision

1. **The public surface is hashing, signature verification and the standard
   security handler.** `Digest` with `Md5`, `Sha1`, `Sha224`, `Sha256`,
   `Sha384` and `Sha512`; `HashAlgorithm`, `RsaPublicKey`, `PssParameters`,
   `RsaLimits`, `ecdsa::{Curve, PublicKey, Signature}` and `Verification`;
   `standard::*`; `features`. AES and RC4 are `pub(crate)`: only the security
   handler needs them, and a reader has no use for encryption or for a cipher
   API without a key-management story. Private modules use `pub(crate)`,
   enforced with `unreachable_pub`.
2. **One `Digest` trait over one engine.** `update`, `finalize` and a
   one-shot `digest` replace the free functions. A single generic
   Merkle–Damgård engine owns the block buffer, message length and padding;
   each algorithm supplies its chaining state, compression function, length
   field and output encoding. The trait has no block-size constant until a
   consumer (for example HMAC) needs one.
3. **Parse, don't validate.** `SecurityParams` stays the plain record of
   Tables 20 and 21 that the document layer fills. `authenticate` parses it
   into a private typed handler whose strings are fixed-size arrays and whose
   key length is a 5–16 byte type; the algorithms read only those. Error
   paths for states the types exclude are removed rather than given a
   `Syntax` kind.
4. **The authenticated key carries its prepared form.** `FileKey` holds the
   legacy key, or for revisions 5 and 6 the expanded AES-256 key that AESV3
   uses directly, so the file key is not re-expanded for every string and
   stream. It is not comparable and its `Debug` prints no key material.
5. **Key material is cleared on a best-effort basis.** The AES key schedule,
   the RC4 state, hash state and buffers, the legacy file key, derived object
   keys, the Algorithm 2.B workspace and SASLprep's password buffers are
   overwritten when no longer needed, through one `wipe` helper and a
   `SecretBytes` buffer that never reallocates. Safe Rust cannot prevent
   copies made by moves, reallocation, registers or the optimizer, so this
   narrows how long key bytes stay in freed memory; it is not a guarantee, in
   line with the crate's stated lack of constant-time behaviour.

## Consequences

- Callers that hash import `pagura_crypto::Digest` (`pagura-render` does for
  ICC profile IDs) and write `Sha256::digest(data)` instead of
  `sha256(data)`. Code that chose a hash at run time can write one generic
  function over `D: Digest`.
- `HashAlgorithm`, `Curve` and `RsaLimits` are `#[non_exhaustive]`: RFC 8017
  and SP 800-186 define more hashes and curves than are implemented. Matches
  outside the crate need a wildcard arm, and `RsaLimits` is built from
  `&Limits` or `Default`. Enums whose set a standard closes (`Verification`,
  `Authority`, `CryptMethod`, `PermissionsCheck`, `PermissionsValidation`)
  stay exhaustive and say why.
- Exposing a cipher later is an API decision for its own ADR, not a
  visibility change.
- Clearing adds a few stores per key or hash; none is on a per-pixel or
  per-glyph path.
