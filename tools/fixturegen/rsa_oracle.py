"""Read private reference vectors and produce independent arithmetic oracles.

Only Python's standard library is used; this never imports or runs Pagura.
NIST cases are public domain. Full Wycheproof data stays in the external cache
(Apache-2.0); no Wycheproof vectors are bundled here. Cache files are read
only after their MANIFEST.tsv size and SHA-256 match. Protocol records contain
precomputed digests so RSA and message hashing are checked independently.
"""
import collections
import hashlib
import io
import math
import random
import re
import sys
import zipfile
if __package__:
    from .ecdsa_vectors import ROOT, reference_bytes, reference_json
else:
    from ecdsa_vectors import ROOT, reference_bytes, reference_json


SUPPORTED = {"SHA1", "SHA224", "SHA256", "SHA384", "SHA512"}


def name(algorithm):
    return algorithm.replace("SHA2-", "SHA").replace("-", "")


def digest(algorithm, message):
    return hashlib.new(name(algorithm).lower(), bytes.fromhex(message)).hexdigest()


def integer(value):
    number = int(value, 16)
    return number.to_bytes(max(1, (number.bit_length() + 7) // 8), "big").hex()


def emit(source, ident, kind, algorithm, mgf, salt, n, e, message, signature, outcome):
    if len(message) % 2:
        # Preserve non-byte CAVP negative messages for explicit format rejection.
        assert outcome == "invalid"
        kind = "malformed-hex"
        message_digest = message
    else:
        message_digest = digest(algorithm, message)
    print("|".join(map(str, [source, ident, kind, name(algorithm), name(mgf), salt,
          integer(n), integer(e), message_digest, signature or "-", outcome])))


def hashes():
    path = ROOT / "cavp/shabytetestvectors.zip"
    if not path.exists():
        print("# CAVP SHA cache unavailable; bundled cases still run")
        return
    with zipfile.ZipFile(io.BytesIO(reference_bytes(path))) as archive:
        for algorithm in ["SHA1", "SHA224"]:
            for kind in ["ShortMsg", "LongMsg", "Monte"]:
                fields = {}
                for line in archive.read(f"shabytetestvectors/{algorithm}{kind}.rsp").decode().splitlines():
                    if "=" not in line or line.startswith(("#", "[")):
                        continue
                    key, value = map(str.strip, line.split("=", 1))
                    fields[key] = value
                    if key != "MD":
                        continue
                    if kind == "Monte":
                        print("|".join(["monte", algorithm, fields["Seed"], value]))
                        fields["Seed"] = value
                    else:
                        length = int(fields["Len"])
                        assert length % 8 == 0
                        message = fields["Msg"][:length // 4]
                        print("|".join([kind, algorithm, message or "-", value]))


def wycheproof():
    directory = ROOT / "wycheproof/testvectors_v1"
    if not directory.exists():
        print("# Wycheproof RSA cache unavailable; bundled cases still run")
        return
    for path in sorted(directory.glob("rsa*.json")):
        data = reference_json(path)
        assert data["algorithm"] in ("RSASSA-PKCS1-v1_5", "RSASSA-PSS")
        kind = "v15" if data["algorithm"] == "RSASSA-PKCS1-v1_5" else "pss"
        count = 0
        for group in data["testGroups"]:
            key = group["publicKey"]
            for test in group["tests"]:
                outcome = test["result"]
                if outcome == "acceptable":
                    # RFC 8017 9.2 requires DER and an explicit NULL. These
                    # compatibility encodings are deliberately rejected.
                    assert set(test["flags"]) <= {"MissingNull", "BerEncodedPadding"}, test
                emit(path.stem, test["tcId"], kind, group["sha"], group.get("mgfSha", group["sha"]),
                     group.get("sLen", 0), key["modulus"], key["publicExponent"],
                     test["msg"], test["sig"], outcome)
                count += 1
        assert count == data["numberOfTests"]
        print(f"# {path.name}: all {count} vectors loaded")


def acvp():
    directory = ROOT / "acvp/RSA-SigVer-FIPS186-5"
    if not directory.exists():
        print("# ACVP RSA cache unavailable; bundled cases still run")
        return
    data = reference_json(directory / "prompt.json")
    results = reference_json(directory / "expectedResults.json")
    expected = {(g["tgId"], t["tcId"]): t["testPassed"] for g in results["testGroups"] for t in g["tests"]}
    unsupported = collections.Counter()
    for group in data["testGroups"]:
        algorithm = group["hashAlg"]
        if name(algorithm) not in SUPPORTED:
            unsupported[(group["sigType"], algorithm)] += len(group["tests"])
            continue
        kind = "v15" if group["sigType"] == "pkcs1v1.5" else "pss"
        assert group["sigType"] in ("pkcs1v1.5", "pss")
        if kind == "pss":
            assert group["maskFunction"] == "mgf1"
        for test in group["tests"]:
            emit("ACVP-" + kind, test["tcId"], kind, algorithm, algorithm, group.get("saltLen", 0),
                 group["n"], group["e"], test["message"], test["signature"],
                 "valid" if expected[(group["tgId"], test["tcId"])] else "invalid")
    for (kind, algorithm), count in sorted(unsupported.items()):
        print(f"# ACVP {kind}: {count} {algorithm} cases require an unimplemented hash")


def cavp():
    path = ROOT / "cavp/186-3rsatestvectors.zip"
    if not path.exists():
        print("# CAVP RSA cache unavailable; bundled cases still run")
        return
    with zipfile.ZipFile(io.BytesIO(reference_bytes(path))) as archive:
        for filename, kind in [("SigVer15_186-3.rsp", "v15"), ("SigVerPSS_186-3.rsp", "pss")]:
            text = archive.read(filename).decode()
            salt_lengths = {}
            if kind == "pss":
                profiles = re.split(r"Mod Size (\d+) with ", text.splitlines()[2])
                for bits, profile in zip(profiles[1::2], profiles[2::2]):
                    for algorithm, size in re.findall(r"SHA-(1|224|256|384|512)\(Salt len: (\d+)\)", profile):
                        salt_lengths[(int(bits), "SHA" + algorithm)] = int(size)
            fields = {}
            index = 0
            for line in text.splitlines():
                if "=" not in line or line.startswith(("#", "[")):
                    continue
                key, value = map(str.strip, line.split("=", 1))
                fields[key] = value
                if key != "Result":
                    continue
                index += 1
                # The header supplies the profile independently of the encoded
                # message. CAVP represents a zero-byte SaltVal with the text 00.
                bits = int(fields["n"], 16).bit_length()
                salt = 0 if kind == "v15" else salt_lengths[(bits, fields["SHAAlg"])]
                if kind == "pss":
                    assert (salt == 0 and fields["SaltVal"] == "00") or len(fields["SaltVal"]) == salt * 2
                # CAVP writes S as an integer (some valid values omit a leading
                # zero nibble). RFC 8017 I2OSP gives its signature octets.
                signature = int(fields["S"], 16)
                width = max((bits + 7) // 8, (signature.bit_length() + 7) // 8)
                emit("CAVP-" + kind, index, kind, fields["SHAAlg"], fields["SHAAlg"], salt,
                     fields["n"], fields["e"], fields["Msg"], signature.to_bytes(width, "big").hex(),
                     "valid" if value.startswith("P") else "invalid")


def powers():
    rng = random.Random(0x8017)
    cases = []
    for _ in range(1000):
        bits = rng.randrange(2, 1025)
        modulus = rng.getrandbits(bits) | (1 << (bits - 1)) | 1
        cases.append((modulus, rng.randrange(modulus), rng.getrandbits(rng.randrange(257))))
    for bits in [2, 3, 7, 8, 9, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257,
                 511, 512, 513, 1023, 1024, 1025, 2047, 2048, 2049, 3072, 4096, 8192, 16384]:
        for modulus in [(1 << bits) - 1, (1 << (bits - 1)) + 1]:
            for base in [0, 1, modulus // 2, modulus - 1]:
                for exponent in [0, 1, 2, 3, 65537]:
                    cases.append((modulus, base, exponent))
    for modulus, base, exponent in cases:
        width = (modulus.bit_length() + 7) // 8
        def encode(value):
            return value.to_bytes(max(1, (value.bit_length() + 7) // 8), "big").hex()
        print("|".join([encode(modulus), base.to_bytes(width, "big").hex(),
                        encode(exponent) if exponent else "-", pow(base, exponent, modulus).to_bytes(width, "big").hex()]))


def timing():
    directory = ROOT / "wycheproof/testvectors_v1"
    for bits in [2048, 3072, 4096]:
        data = reference_json(directory / f"rsa_signature_{bits}_sha256_test.json")
        group = data["testGroups"][0]
        test = next(t for t in group["tests"] if t["result"] == "valid")
        key = group["publicKey"]
        print("|".join([str(bits), integer(key["modulus"]), integer(key["publicExponent"]),
                        digest("SHA256", test["msg"]), test["sig"]]))


def test_key():
    """Public deterministic test material from known Mersenne primes."""
    p, q, e = (1 << 521) - 1, (1 << 607) - 1, 65537
    return p * q, e, pow(e, -1, (p - 1) * (q - 1))


def sign_representative(encoded, key):
    """Independent RSA arithmetic; private operations exist only in tooling."""
    n, e, d = key
    representative = int.from_bytes(encoded, "big")
    assert representative < n
    signature = pow(representative, d, n)
    assert pow(signature, e, n) == representative
    return signature.to_bytes((n.bit_length() + 7) // 8, "big")


def encodings():
    """RFC 8017 9.1/9.2: independent encodings with targeted mutations.

    Test material uses known Mersenne primes and the small prime 65521.
    Python integer inversion/exponentiation creates signatures; each recovered
    representative is checked here before it is emitted. No product code or
    Pagura output supplies an expectation.
    """
    message = b"RFC 8017 encoding boundary checks"
    p = (1 << 521) - 1
    assert all(65521 % d for d in range(2, math.isqrt(65521) + 1))
    for q in [(1 << 607) - 1, 65521]:
        n = p * q
        e = 65537
        d = pow(e, -1, (p - 1) * (q - 1))
        bits = n.bit_length()
        width = (bits + 7) // 8

        def signed(ident, kind, algorithm, mgf, salt, encoded, valid):
            representative = int.from_bytes(encoded, "big")
            assert representative < n
            signature = pow(representative, d, n)
            assert pow(signature, e, n) == representative
            emit("RFC8017", str(bits) + "-" + ident, kind, algorithm, mgf, salt,
                 hex(n), hex(e), message.hex(), signature.to_bytes(width, "big").hex(),
                 "valid" if valid else "invalid")

        algorithms = ["SHA1", "SHA224", "SHA256", "SHA384", "SHA512"] if q > 65521 else ["SHA256"]
        for algorithm in algorithms:
            hashed = bytes.fromhex(digest(algorithm, message.hex()))
            if q > 65521:
                oid = (b"\x2b\x0e\x03\x02\x1a" if algorithm == "SHA1" else
                       b"\x60\x86\x48\x01\x65\x03\x04\x02" + bytes([{"SHA224": 4, "SHA256": 1, "SHA384": 2, "SHA512": 3}[algorithm]]))
                def der(tag, value):
                    assert len(value) < 128
                    return bytes([tag, len(value)]) + value
                def info(parameters):
                    return der(0x30, der(0x30, der(6, oid) + parameters) + der(4, hashed))
                def v15(tail):
                    return b"\0\1" + b"\xff" * (width - len(tail) - 3) + b"\0" + tail
                tail = info(b"\x05\0")
                signed(algorithm + "-der", "v15", algorithm, algorithm, 0, v15(tail), True)
                for label, altered in [("missing-null", info(b"")),
                    ("ber-length", b"\x30\x81" + tail[1:]),
                    ("trailing-data", tail + b"\0"),
                    ("wrong-digest", tail[:-1] + bytes([tail[-1] ^ 1]))]:
                    signed(algorithm + "-" + label, "v15", algorithm, algorithm, 0, v15(altered), False)
                encoded = bytearray(v15(tail)); encoded[3] = 0xfe
                signed(algorithm + "-padding", "v15", algorithm, algorithm, 0, encoded, False)
            em_bits = bits - 1
            em_width = (em_bits + 7) // 8
            for salt_len in sorted({0, min(len(hashed), em_width - len(hashed) - 2), em_width - len(hashed) - 2}):
                salt = bytes(range(salt_len))
                for mgf in sorted({algorithm, "SHA1"}):
                    h = hashlib.new(name(algorithm).lower(), b"\0" * 8 + hashed + salt).digest()
                    db = b"\0" * (em_width - len(h) - salt_len - 2) + b"\1" + salt
                    mask = b"".join(hashlib.new(name(mgf).lower(), h + i.to_bytes(4, "big")).digest()
                                    for i in range((len(db) + hashlib.new(name(mgf).lower()).digest_size - 1) // hashlib.new(name(mgf).lower()).digest_size))
                    encoded = bytearray(a ^ b for a, b in zip(db, mask))
                    encoded[0] &= 0xff >> (8 * em_width - em_bits)
                    encoded += h + b"\xbc"
                    ident = algorithm + "-" + mgf + "-" + str(salt_len)
                    signed(ident, "pss", algorithm, mgf, salt_len, encoded, True)
                    wrong = bytearray(encoded); wrong[-1] ^= 1
                    signed(ident + "-trailer", "pss", algorithm, mgf, salt_len, wrong, False)
                    if salt_len:
                        signed(ident + "-salt", "pss", algorithm, mgf, salt_len - 1, encoded, False)
                    if bits % 8 == 1:
                        signed(ident + "-extra-octet", "pss", algorithm, mgf, salt_len, b"\1" + encoded, False)
                    else:
                        wrong = bytearray(encoded); wrong[0] |= 0x80
                        signed(ident + "-high-bit", "pss", algorithm, mgf, salt_len, wrong, False)


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "hash":
        hashes()
    elif mode == "rsa":
        wycheproof()
        acvp()
        cavp()
    elif mode == "pow":
        powers()
    elif mode == "timing":
        timing()
    elif mode == "encodings":
        encodings()
    else:
        raise ValueError(f"unknown suite {mode}")
