"""Independent encryption-side oracles, from PDF algorithms and published vectors.

Only Python stdlib is used. This module never imports or executes Pagura.
Deterministic salts, keys and IVs are FOR TEST FIXTURES ONLY, never production
writing. AES is a word-oriented implementation of FIPS 197, independent of the
byte-oriented Rust implementation. R5 is the documented Adobe extension.
"""
from __future__ import annotations

from functools import lru_cache
from hashlib import md5, sha256, sha384, sha512

from .model import Case, Vector, INVALID, RENDER
from .pdf import document, stream, opaque_rectangles

AREA = "crypto"
PAD = bytes.fromhex("28bf4e5e4e758a4164004e56fffa01082e2e00b6d0683e802f0ca9fe6453697a")
ID = bytes.fromhex("00112233445566778899aabbccddeeff")
PERMISSIONS = -4
CONTENT = b"1 0 0 rg 8 8 48 32 re f\n0 0 1 rg 0 0 8 8 re f\n"
PLAIN = b"independent security handler oracle\x00\xff"
SBOX = bytes.fromhex(
    "637c777bf26b6fc53001672bfed7ab76ca82c97dfa5947f0add4a2af9ca472c0"
    "b7fd9326363ff7cc34a5e5f171d8311504c723c31896059a071280e2eb27b275"
    "09832c1a1b6e5aa0523bd6b329e32f8453d100ed20fcb15b6acbbe394a4c58cf"
    "d0efaafb434d338545f9027f503c9fa851a3408f929d38f5bcb6da2110fff3d2"
    "cd0c13ec5f974417c4a77e3d645d197360814fdc222a908846eeb814de5e0bdb"
    "e0323a0a4906245cc2d3ac629195e479e7c8376d8dd54ea96c56f4ea657aae08"
    "ba78252e1ca6b4c6e8dd741f4bbd8b8a703eb5664803f60e613557b986c11d9e"
    "e1f8981169d98e949b1e87e9ce5528df8ca1890dbfe6426841992d0fb054bb16"
)


def xtime(x):
    return ((x << 1) ^ (0x11b if x & 0x80 else 0)) & 255


# Word-oriented round tables: SubBytes plus MixColumns; ShiftRows selects bytes.
TABLE = tuple((xtime(s) << 24) | (s << 16) | (s << 8) | (xtime(s) ^ s) for s in SBOX)


def rotate(word, bits):
    return ((word >> bits) | (word << (32 - bits))) & 0xffffffff


def subword(word):
    return int.from_bytes(bytes(SBOX[x] for x in word.to_bytes(4, "big")), "big")


class AES:
    """Encryption-only FIPS 197 AES, with 32-bit column words."""
    def __init__(self, key):
        assert len(key) in (16, 32)
        count = len(key) // 4
        self.rounds = count + 6
        self.words = [int.from_bytes(key[i:i+4], "big") for i in range(0, len(key), 4)]
        rcon = 1
        while len(self.words) < 4 * (self.rounds + 1):
            i = len(self.words)
            value = self.words[-1]
            if i % count == 0:
                value = subword(rotate(value, 24)) ^ (rcon << 24)
                rcon = xtime(rcon)
            elif count == 8 and i % count == 4:
                value = subword(value)
            self.words.append(self.words[-count] ^ value)

    def block(self, data):
        assert len(data) == 16
        state = [int.from_bytes(data[i:i+4], "big") ^ self.words[i//4] for i in range(0, 16, 4)]
        for rnd in range(1, self.rounds):
            state = [TABLE[state[c] >> 24]
                     ^ rotate(TABLE[(state[(c+1) % 4] >> 16) & 255], 8)
                     ^ rotate(TABLE[(state[(c+2) % 4] >> 8) & 255], 16)
                     ^ rotate(TABLE[state[(c+3) % 4] & 255], 24)
                     ^ self.words[rnd*4+c] for c in range(4)]
        final = [(SBOX[state[c] >> 24] << 24)
                 | (SBOX[(state[(c+1) % 4] >> 16) & 255] << 16)
                 | (SBOX[(state[(c+2) % 4] >> 8) & 255] << 8)
                 | SBOX[state[(c+3) % 4] & 255] for c in range(4)]
        return b"".join((word ^ key).to_bytes(4, "big") for word, key in zip(final, self.words[-4:]))

    def cbc(self, data, iv=bytes(16)):
        assert len(data) % 16 == 0 and len(iv) == 16
        result = bytearray()
        for i in range(0, len(data), 16):
            iv = self.block(bytes(x ^ y for x, y in zip(data[i:i+16], iv)))
            result.extend(iv)
        return bytes(result)


def rc4(key, data):
    assert 1 <= len(key) <= 256
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    i = j = 0
    output = bytearray()
    for value in data:
        i = (i+1) % 256
        j = (j+state[i]) % 256
        state[i], state[j] = state[j], state[i]
        output.append(value ^ state[(state[i]+state[j]) % 256])
    return bytes(output)


def self_check():
    """Published FIPS 197 Appendix C, SP 800-38A F.2 and RFC 6229 vectors."""
    plain = bytes.fromhex("00112233445566778899aabbccddeeff")
    for key, expected in [
        (bytes(range(16)), "69c4e0d86a7b0430d8cdb78070b4c55a"),
        (bytes(range(32)), "8ea2b7ca516745bfeafc49904b496089"),
    ]:
        assert AES(key).block(plain).hex() == expected
    plain = bytes.fromhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51"
                          "30c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710")
    iv = bytes(range(16))
    for key, expected in [
        ("2b7e151628aed2a6abf7158809cf4f3c", "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
                                             "73bed6b8e3c1743b7116e69e222295163ff1caa1681fac09120eca307586e1a7"),
        ("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4", "f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d"
                                                                             "39f23369a9d9bacfa530e26304231461b2eb05e2c39be9fcda6c19078c6a9d1b"),
    ]:
        assert AES(bytes.fromhex(key)).cbc(plain, iv).hex() == expected
    for size, expected in [(7, "293f02d47f37c9b633f2af5285feb46b"),
                           (8, "97ab8a1bf0afb96132f2f67258da15a8"),
                           (10, "ede3b04643e586cc907dc21851709902"),
                           (16, "9ac7cc9a609d1ef7b2932899cde41b97"),
                           (24, "0595e57fe5f0bb3c706edac8a4b2db11"),
                           (32, "eaa6bd25880bf93d3f5d1e4ca2611d91")]:
        assert rc4(bytes(range(1, size+1)), bytes(16)).hex() == expected
    cipher = rc4(bytes(range(1, 6)), bytes(272))
    assert cipher[:16].hex() == "b2396305f03dc027ccc3524a0a1118a8"
    assert cipher[16:32].hex() == "6982944f18fc82d589c403a47a0d0919"
    assert cipher[256:272].hex() == "1cfcf62b03eddb641d77dfcf7f8d8c93"
    assert md5(b"abc").hexdigest() == "900150983cd24fb0d6963f7d28e17f72"
    assert sha256(b"abc").hexdigest() == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    assert sha384(b"abc").hexdigest() == "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    assert sha512(b"abc").hexdigest() == "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"


def padded(password):
    return (password[:32] + PAD)[:32]


@lru_cache(maxsize=None)
def modern_hash(revision, password, salt, user=b""):
    """Algorithms 2.A/2.B, password is an already prepared byte string."""
    password = password[:127]
    k = sha256(password + salt + user).digest()
    if revision == 5:
        return k
    iteration = 0
    last = 0
    while iteration < 64 or last > iteration - 32:
        e = AES(k[:16]).cbc((password + k + user) * 64, k[16:32])
        selector = int.from_bytes(e[:16], "big") % 3
        k = (sha256, sha384, sha512)[selector](e).digest()
        last = e[-1]
        iteration += 1
    assert iteration <= 287
    return k[:32]


def parameters(revision, user, owner, metadata=True, length=None, permissions=PERMISSIONS):
    length = length or (5 if revision == 2 else 32 if revision >= 5 else 16)
    p = dict(version={2: 1, 3: 2, 4: 4, 5: 5, 6: 5}[revision], revision=revision,
             key_length=length, permissions=permissions, encrypt_metadata=metadata, document_id=ID)
    if revision < 5:
        digest = md5(padded(owner or user)).digest()
        if revision >= 3:
            for _ in range(50):
                digest = md5(digest).digest()
        o = padded(user)
        for i in range(1 if revision == 2 else 20):
            o = rc4(bytes(x ^ i for x in digest[:length]), o)
        p["owner_hash"] = o
        digest = md5(padded(user) + o + permissions.to_bytes(4, "little", signed=True) + ID
                     + (b"\xff" * 4 if revision >= 4 and not metadata else b"")).digest()
        if revision >= 3:
            for _ in range(50):
                digest = md5(digest[:length]).digest()
        key = digest[:length]
        u = PAD if revision == 2 else md5(PAD + ID).digest()
        for i in range(1 if revision == 2 else 20):
            u = rc4(bytes(x ^ i for x in key), u)
        p["user_hash"] = u if revision == 2 else u + bytes(16)
    else:
        key = sha256(b"fixture file key" + bytes([revision, metadata]) + user + owner).digest()
        # Distinct deterministic validation/key salts, not random in fixture data.
        uv, uk, ov, ok = [sha256(bytes([revision, i]) + user + owner).digest()[:8] for i in range(4)]
        u = modern_hash(revision, user, uv) + uv + uk
        p.update(user_hash=u, owner_hash=modern_hash(revision, owner, ov, u) + ov + ok,
                 user_encrypted_key=AES(modern_hash(revision, user, uk)).cbc(key),
                 owner_encrypted_key=AES(modern_hash(revision, owner, ok, u)).cbc(key),
                 encrypted_permissions=AES(key).block(permissions.to_bytes(4, "little", signed=True)
                     + b"\xff"*4 + (b"T" if metadata else b"F") + b"adb" + bytes.fromhex("12345678")))
    return p, key


def encrypt(key, method, object_number, data, generation=0):
    if method == "Identity":
        return data
    if method != "AESV3":
        key = md5(key + object_number.to_bytes(4, "little")[:3] + generation.to_bytes(2, "little")
                  + (b"sAlT" if method == "AESV2" else b"")).digest()[:min(len(key)+5, 16)]
    if method == "V2":
        return rc4(key, data)
    iv = sha256(b"fixture IV" + data).digest()[:16]
    padding = 16 - len(data) % 16
    return iv + AES(key).cbc(data + bytes([padding])*padding, iv)


def hex_string(data):
    return b"<" + data.hex().encode() + b">"


def encryption_dictionary(p, method, identity_strings):
    result = b"<< /Filter /Standard /V %d /R %d /Length %d /P %d " % (p["version"], p["revision"], p["key_length"]*8, p["permissions"])
    for field, name in [("owner_hash", "O"), ("user_hash", "U"), ("owner_encrypted_key", "OE"),
                        ("user_encrypted_key", "UE"), ("encrypted_permissions", "Perms")]:
        if field in p:
            result += b"/" + name.encode() + b" " + hex_string(p[field]) + b" "
    if p["revision"] >= 4:
        result += b"/EncryptMetadata " + (b"true " if p["encrypt_metadata"] else b"false ")
        result += b"/CF << /StdCF << /CFM /" + method.encode() + b" /AuthEvent /DocOpen /Length %d >> >> " % p["key_length"]
        result += b"/StmF /StdCF /StrF /" + (b"Identity " if identity_strings else b"StdCF ")
    return result + b">>"


def encrypted_pdf(p, key, method, identity_strings=False):
    # Object 3 exercises string decryption; object 4 exercises stream decryption.
    title = encrypt(key, "Identity" if identity_strings else method, 3, b"fixture title")
    extension = b" /Extensions << /ADBE << /BaseVersion /1.7 /ExtensionLevel 3 >> >>" if p["revision"] == 5 else b""
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R" + extension + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents 4 0 R /Title " + hex_string(title) + b" >>",
        stream(encrypt(key, method, 4, CONTENT)),
        encryption_dictionary(p, method, identity_strings),
        stream(encrypt(key, method, 6, b"<metadata>fixture</metadata>") if p["encrypt_metadata"] else b"<metadata>fixture</metadata>", b"/Type /Metadata /Subtype /XML "),
    ]
    pdf = document(objects, b"/Encrypt 5 0 R /ID [" + hex_string(ID) + b" " + hex_string(ID) + b"] ")
    # Same-width header replacement preserves every independently computed xref offset.
    return pdf.replace(b"%PDF-1.7", b"%PDF-2.0", 1) if p["revision"] == 6 else pdf


def vector(name, p, key, user, owner, method, validation=None):
    params = "".join(f"{field}={value.hex() if isinstance(value, bytes) else int(value)}\n" for field, value in p.items()).encode()
    # Generation-aware keys, truncation of high object-number byte, and padding boundaries.
    files = {".params": params, ".key": key, ".user": user, ".owner": owner, ".plain": PLAIN,
             ".cipher": encrypt(key, method, 0x01020304, PLAIN, 513), ".method": method.encode()}
    if method.startswith("AES"):
        for size in [0, 1, 15, 16, 17, 31, 32]:
            files[f".size{size}"] = encrypt(key, method, 7, bytes(range(size)), 0)
    extra = {}
    if validation is not None:
        files[".validation"] = validation.encode()
        extra["permissions_validation"] = validation
    return Vector(name, files, ("security-" + ("r5-extension" if p["revision"] == 5 else f'r{p["revision"]}'),),
                  "Independent encryption-side authentication, file-key and object-key oracle", extra=extra)


def cases():
    self_check()
    data = bytes(range(256)) + bytes(range(5))
    lines = [f"{length} " + " ".join(hash(data[:length]).hexdigest() for hash in (md5, sha256, sha384, sha512))
             for length in range(len(data)+1)]
    yield Vector("hash-padding-boundaries", {".input": data, ".digests": ("\n".join(lines)+"\n").encode()},
                 ("crypto-hash-vectors",), "hashlib oracles at every length around 64/128-byte padding boundaries")
    oracle = opaque_rectangles(64, 64, [((8, 8, 48, 32), (255, 0, 0)), ((0, 0, 8, 8), (0, 0, 255))])
    configurations = [
        ("r2-rc4-40", 2, "V2", True, False),
        ("r3-rc4-128", 3, "V2", True, False),
        ("r4-aesv2", 4, "AESV2", True, False),
        ("r4-rc4-filter", 4, "V2", True, False),
        ("r4-identity-strings", 4, "AESV2", True, True),
        ("r4-clear-metadata", 4, "AESV2", False, False),
        ("r5-aesv3", 5, "AESV3", True, False),
        ("r6-aesv3", 6, "AESV3", True, False),
        ("r6-clear-metadata", 6, "AESV3", False, False),
    ]
    for prefix, revision, method, metadata, identity in configurations:
        for user in [b"", b"user-secret"]:
            owner = b"owner-secret"
            p, key = parameters(revision, user, owner, metadata)
            name = prefix + ("-empty" if not user else "-password")
            yield vector(name, p, key, user, owner, method)
            pdf = encrypted_pdf(p, key, method, identity)
            requirement = "security-" + ("r5-extension" if revision == 5 else f"r{revision}")
            for suffix, password in [("user", user), ("owner", owner)]:
                yield Case(name + "-" + suffix, pdf, RENDER, (requirement,),
                           f"{prefix}, {suffix} authentication, {'empty' if not user else 'nonempty'} user password",
                           oracle=oracle, extra={"password": password.decode()})
            yield Case(name + "-wrong-password", pdf, INVALID, (requirement,), "Wrong password must fail opening",
                       pages=None, extra={"password": "wrong-password", "error": "password"})
    # Extra key lengths exercise Algorithm 2's n-byte iteration and Algorithm 1's min(n+5,16).
    for revision, length in [(3, 5), (3, 7), (4, 7)]:
        p, key = parameters(revision, b"short-key-user", b"short-key-owner", length=length)
        yield vector(f"r{revision}-key{length}", p, key, b"short-key-user", b"short-key-owner", "V2")
    # Password truncation and legacy arbitrary byte passwords.
    for revision, user, owner in [(2, bytes(range(128, 168)), b"long-owner"*5),
                                   (3, b"user"*40, b"owner"*40),
                                   (5, b"u"*130, b"o"*130),
                                   (6, b"u"*130, b"o"*130)]:
        p, key = parameters(revision, user, owner)
        yield vector(f"r{revision}-long-password", p, key, user, owner, "AESV3" if revision >= 5 else "V2")

    p, key = parameters(3, b"user", b"owner", length=5, permissions=-260)
    p["version"] = 1
    yield vector("r3-version1", p, key, b"user", b"owner", "V2")

    for revision in [3, 5]:
        p, key = parameters(revision, b"same-password", b"same-password")
        yield vector(f"r{revision}-same-password", p, key, b"same-password", b"same-password", "V2" if revision == 3 else "AESV3")


    # Algorithm 13 outcomes are distinct from successful password authentication.
    # Keys and password entries stay intact; only independently encrypted /Perms
    # plaintext changes. No document recovery outcome is assumed here.
    outcomes = [
        ("validated", (), "validated"),
        ("permissions", (0,), "permissions"),
        ("reserved-bytes", (4,), "reserved-bytes"),
        ("encrypt-metadata", (8,), "encrypt-metadata"),
        ("marker", (9,), "marker"),
        ("multiple", (0, 4, 8, 9), "permissions"),
        ("random-tail", (12, 13, 14, 15), "validated"),
    ]
    for revision in [5, 6]:
        user, owner = b"", b"owner-secret"
        base, key = parameters(revision, user, owner)
        plain = (base["permissions"].to_bytes(4, "little", signed=True)
                 + b"\xff"*4 + b"Tadb" + bytes.fromhex("12345678"))
        for name, offsets, validation in outcomes:
            block = bytearray(plain)
            for offset in offsets:
                block[offset] ^= 1
            p = {**base, "encrypted_permissions": AES(key).block(block)}
            yield vector(f"r{revision}-perms-{name}", p, key, user, owner, "AESV3", validation)
