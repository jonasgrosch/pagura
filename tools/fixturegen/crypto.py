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
    """Fixed attributed vectors; source availability is recorded in crypto.json.

    FIPS 197-upd1 Appendix C links to a separate vector document, whereas
    Appendix A/B examples are present locally and checked in primitive_examples.
    The CBC/RC4 source texts remain unavailable; these are corroborating tests.
    """
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


def encrypted_objects(p, key, method, identity_strings=False):
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
    return objects


def encrypted_pdf(p, key, method, identity_strings=False):
    objects = encrypted_objects(p, key, method, identity_strings)
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


def integer_root(value, degree):
    """Exact floor root; no floating-point rounding of SHA-2 constants."""
    low, high = 0, 1 << ((value.bit_length() + degree - 1) // degree)
    while high - low > 1:
        middle = (low + high) // 2
        if middle**degree <= value:
            low = middle
        else:
            high = middle
    return low


def primitive_examples():
    """Primary-text numeric examples and independent equation-derived oracles."""
    # RFC 1321 section 3.4; guard against values near a floating-point boundary.
    from math import sin
    scaled = [abs(sin(i)) * 2**32 for i in range(1, 65)]
    assert all(abs(value - round(value)) > 0.00001 for value in scaled)
    constants = b"".join(int(value).to_bytes(4, "little") for value in scaled)
    shifts = bytes([7, 12, 17, 22]*4 + [5, 9, 14, 20]*4 + [4, 11, 16, 23]*4 + [6, 10, 15, 21]*4)
    yield Vector("md5-round-constants", {".constants": constants, ".shifts": shifts},
                 ("crypto-hash-vectors",), "RFC 1321 section 3.4 sine-derived constants and all rotation counts")
    primes = []
    candidate = 2
    while len(primes) < 80:
        if all(candidate % prime for prime in primes if prime*prime <= candidate):
            primes.append(candidate)
        candidate += 1

    def roots(values, degree, width):
        mask = (1 << width) - 1
        return b"".join((integer_root(prime << (width*degree), degree) & mask).to_bytes(width//8, "big")
                        for prime in values)

    yield Vector("sha2-round-constants", {
        ".sha256": roots(primes[:64], 3, 32), ".sha512": roots(primes, 3, 64),
        ".iv256": roots(primes[:8], 2, 32), ".iv384": roots(primes[8:16], 2, 64),
        ".iv512": roots(primes[:8], 2, 64)}, ("crypto-hash-vectors",),
        "FIPS 180-4 sections 4.2.2-4.2.3 and 5.3.3-5.3.5 exact fractional-root constants")
    data = b"abc"
    yield Vector("hash-padding-abc", {
        ".input": data, ".md5-padded": data + b"\x80" + bytes(52) + (24).to_bytes(8, "little"),
        ".sha256-padded": data + b"\x80" + bytes(52) + (24).to_bytes(8, "big"),
        ".sha512-padded": data + b"\x80" + bytes(108) + (24).to_bytes(16, "big"),
        ".digests": (" ".join(hash(data).hexdigest() for hash in (md5, sha256, sha384, sha512))+"\n").encode()},
        ("crypto-hash-vectors",), "FIPS 180-4 sections 5.1.1-5.1.2 printed abc padding examples; RFC 1321 sections 3.1-3.2 padding; hashlib digest oracles")
    inverse = bytes(SBOX.index(value) for value in range(256))
    yield Vector("aes-substitution-tables", {".forward": SBOX, ".inverse": inverse},
                 ("crypto-cipher-vectors",), "FIPS 197-upd1 Tables 4 and 6 complete forward and inverse substitution values")
    # FIPS 197-upd1 Appendix A.1: every word, including the input key.
    key = bytes.fromhex("2b7e151628aed2a6abf7158809cf4f3c")
    expanded = bytes.fromhex(
        "2b7e151628aed2a6abf7158809cf4f3ca0fafe1788542cb123a339392a6c7605"
        "f2c295f27a96b9435935807a7359f67f3d80477d4716fe3e1e237e446d7a883b"
        "ef44a541a8525b7fb671253bdb0bad00d4d1c6f87c839d87caf2b8bc11f915bc"
        "6d88a37a110b3efddbf98641ca0093fd4e54f70e5f5fc9f384a64fb24ea6dc4f"
        "ead27321b58dbad2312bf5607f8d292fac7766f319fadc2128d12941575c006e"
        "d014f9a8c9ee2589e13f0cc8b6630ca6"
    )
    assert expanded == b"".join(word.to_bytes(4, "big") for word in AES(key).words)
    yield Vector("aes128-key-expansion", {".key": key, ".expanded": expanded},
                 ("crypto-cipher-vectors",), "FIPS 197-upd1 Appendix A.1 complete key schedule")
    # FIPS 197-upd1 Appendix A.3: every word, including the input key.
    key = bytes.fromhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
    expanded = bytes.fromhex(
        "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4"
        "9ba354118e6925afa51a8b5f2067fcdea8b09c1a93d194cdbe49846eb75d5b9a"
        "d59aecb85bf3c917fee94248de8ebe96b5a9328a2678a647983122292f6c79b3"
        "812c81addadf48ba24360af2fab8b46498c5bfc9bebd198e268c3ba709e04214"
        "68007bacb2df331696e939e46c518d80c814e20476a9fb8a5025c02d59c58239"
        "de1369676ccc5a71fa2563959674ee155886ca5d2e2f31d77e0af1fa27cf73c3"
        "749c47ab18501ddae2757e4f7401905acafaaae3e4d59b349adf6acebd10190d"
        "fe4890d1e6188d0b046df344706c631e"
    )
    assert expanded == b"".join(word.to_bytes(4, "big") for word in AES(key).words)
    yield Vector("aes256-key-expansion", {".key": key, ".expanded": expanded},
                 ("crypto-cipher-vectors",), "FIPS 197-upd1 Appendix A.3 complete key schedule")
    # Numeric states printed in Appendix B, serialized by columns (section 3.4).
    states = (
        "1 start 193de3bea0f4e22b9ac68d2ae9f84808\n"
        "1 sub d42711aee0bf98f1b8b45de51e415230\n"
        "1 shift d4bf5d30e0b452aeb84111f11e2798e5\n"
        "1 mix 046681e5e0cb199a48f8d37a2806264c\n"
        "2 start a49c7ff2689f352b6b5bea43026a5049\n"
        "2 sub 49ded28945db96f17f39871a7702533b\n"
        "2 shift 49db873b453953897f02d2f177de961a\n"
        "2 mix 584dcaf11b4b5aacdbe7caa81b6bb0e5\n"
        "3 start aa8f5f0361dde3ef82d24ad26832469a\n"
        "3 sub ac73cf7befc111df13b5d6b545235ab8\n"
        "3 shift acc1d6b8efb55a7b1323cfdf457311b5\n"
        "3 mix 75ec0993200b633353c0cf7cbb25d0dc\n"
        "4 start 486c4eee671d9d0d4de3b138d65f58e7\n"
        "4 sub 52502f2885a45ed7e311c807f6cf6a94\n"
        "4 shift 52a4c89485116a28e3cf2fd7f6505e07\n"
        "4 mix 0fd6daa9603138bf6fc0106b5eb31301\n"
        "5 start e0927fe8c86363c0d9b1355085b8be01\n"
        "5 sub e14fd29be8fbfbba35c89653976cae7c\n"
        "5 shift e1fb967ce8c8ae9b356cd2ba974ffb53\n"
        "5 mix 25d1a9adbd11d168b63a338e4c4cc0b0\n"
        "6 start f1006f55c1924cef7cc88b325db5d50c\n"
        "6 sub a163a8fc784f29df10e83d234cd503fe\n"
        "6 shift a14f3dfe78e803fc10d5a8df4c632923\n"
        "6 mix 4b868d6d2c4a8980339df4e837d218d8\n"
        "7 start 260e2e173d41b77de86472a9fdd28b25\n"
        "7 sub f7ab31f02783a9ff9b4340d354b53d3f\n"
        "7 shift f783403f27433df09bb531ff54aba9d3\n"
        "7 mix 1415b5bf461615ec274656d7342ad843\n"
        "8 start 5a4142b11949dc1fa3e019657a8c040c\n"
        "8 sub be832cc8d43b86c00ae1d44dda64f2fe\n"
        "8 shift be3bd4fed4e1f2c80a642cc0da83864d\n"
        "8 mix 00512fd1b1c889ff54766dcdfa1b99ea\n"
        "9 start ea835cf00445332d655d98ad8596b0c5\n"
        "9 sub 87ec4a8cf26ec3d84d4c46959790e7a6\n"
        "9 shift 876e46a6f24ce78c4d904ad897ecc395\n"
        "9 mix 473794ed40d4e4a5a3703aa64c9f42bc\n"
        "10 start eb40f21e592e38848ba113e71bc342d2\n"
        "10 sub e9098972cb31075f3d327d94af2e2cb5\n"
        "10 shift e9317db5cb322c723d2e895faf090794\n"
    ).encode()
    key = bytes.fromhex("2b7e151628aed2a6abf7158809cf4f3c")
    plain = bytes.fromhex("3243f6a8885a308d313198a2e0370734")
    cipher = bytes.fromhex("3925841d02dc09fbdc118597196a0b32")
    assert AES(key).block(plain) == cipher
    yield Vector("aes128-cipher-example", {".key": key, ".plain": plain, ".cipher": cipher, ".states": states},
                 ("crypto-cipher-vectors",), "FIPS 197-upd1 Appendix B cipher output and all 39 intermediate states")

    # Algorithm 2.B retains the full selected digest between iterations. Only
    # the final result is cut to 32 bytes (FIPS 180-4 section 7, not SHA-512/256).
    remaining = {32, 48, 64}
    salt = bytes(range(8))
    for index in range(64):
        password = f"primitive-example-{index}".encode()
        user = bytes(range(48)) if index % 2 else b""
        digest = sha256(password + salt + user).digest()
        rounds, last = 0, 0
        selectors = []
        while rounds < 64 or last > rounds - 32:
            encrypted = AES(digest[:16]).cbc((password + digest + user)*64, digest[16:32])
            selector = int.from_bytes(encrypted[:16], "big") % 3
            selectors.append(selector)
            digest = (sha256, sha384, sha512)[selector](encrypted).digest()
            rounds, last = rounds + 1, encrypted[-1]
        if len(digest) not in remaining:
            continue
        remaining.remove(len(digest))
        assert set(selectors) == {0, 1, 2} and rounds <= 287
        assert modern_hash(6, password, salt, user) == digest[:32]
        yield Vector(f"r6-hash-truncation-{len(digest)}", {
            ".password": password, ".salt": salt, ".user": user, ".full-hash": digest,
            ".hash": digest[:32], ".selectors": bytes(selectors)}, ("security-r6",),
            "ISO 32000-2 Algorithm 2.B full intermediate digest and final leftmost-256-bit truncation",
            extra={"final_digest_bytes": len(digest), "rounds": rounds})
        if not remaining:
            break
    assert not remaining


def cases():
    self_check()
    yield from primitive_examples()
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
        if revision == 4:
            yield invalid_parameter_vector(f"r{revision}-key{length}", p, b"short-key-user", "syntax")
        else:
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

    yield from parameter_edge_cases()
    yield from integration_examples()


def params_bytes(p):
    return "".join(f"{field}={value.hex() if isinstance(value, bytes) else int(value)}\n"
                   for field, value in p.items()).encode()


def invalid_parameter_vector(name, p, password, error):
    return Vector(name, {".params": params_bytes(p), ".password": password, ".error": error.encode()},
                  (f"security-r{p['revision']}",), "Parameter rejection independent of cipher output",
                  extra={"authentication_error": error})


def parameter_edge_cases():
    # Table 21 selects R2/R3 for V=1 using the later-revision access bits.
    p, _ = parameters(2, b"", b"owner-secret", permissions=-260)
    yield invalid_parameter_vector("r2-revision-permissions", p, b"", "syntax")
    p, _ = parameters(3, b"", b"owner-secret", length=5)
    p["version"] = 1
    yield invalid_parameter_vector("r3-version1-permissions", p, b"", "syntax")
    # Table 20 fixes V=4 to 128 bits even when its crypt method is RC4.
    p, _ = parameters(4, b"", b"owner-secret")
    for length in [5, 15]:
        yield invalid_parameter_vector(f"r4-fixed-key-{length}", {**p, "key_length": length}, b"", "syntax")
    # Algorithm 2.A(c) checks O first: equal passwords grant owner authority.
    p, key = parameters(6, b"same-password", b"same-password")
    case = vector("r6-owner-precedence", p, key, b"same-password", b"same-password", "AESV3")
    yield Vector(case.name, {**case.files, ".authority": b"owner"}, case.requirements,
                 "Algorithm 2.A(c): shared user/owner password has owner authority")
    p, _ = parameters(6, b"", b"owner-secret")
    # The unimplemented preparation path must run before byte truncation.
    for name, password in [("ascii-control", b"\x7f"), ("control-after-truncation", b"a"*127+b"\x00"),
                           ("unicode", "café".encode())]:
        yield invalid_parameter_vector("r6-saslprep-"+name, p, password, "unsupported:security.saslprep")
    # Fixed PDFDocEncoding bytes, not UTF-8: U+20AC is byte 0xa0 (Table D.3).
    p, key = parameters(3, b"\xa0", b"owner-secret")
    case = vector("r3-pdfdocencoding-euro", p, key, b"\xa0", b"owner-secret", "V2")
    yield Vector(case.name, {**case.files, ".pdf": encrypted_pdf(p, key, "V2")}, case.requirements,
                 "U+20AC uses PDFDocEncoding byte 0xa0; Unicode text interfaces must convert before hashing",
                 extra={"text_password_codepoint": "U+20AC", "prepared_encoding": "PDFDocEncoding"})
    # Readers preserve raw permission bits in the digest; reserved bits do not
    # become access rights (7.6.4.2, Table 22). These are reader inputs, not
    # examples of conforming permission generation.
    for name, permissions in [("reserved-low", -3), ("reserved-high", 0x0cfc), ("accessibility-clear", -516)]:
        p, key = parameters(3, b"", b"owner-secret", permissions=permissions)
        yield vector("r3-permissions-"+name, p, key, b"", b"owner-secret", "V2")
    # Algorithm 5(f)'s trailing U bytes are arbitrary, not part of validation.
    p, key = parameters(3, b"", b"owner-secret")
    p["user_hash"] = p["user_hash"][:16]+bytes(range(16))
    yield vector("r3-user-hash-tail", p, key, b"", b"owner-secret", "V2")


def integration_examples():
    """Small dictionary/stream inputs with explicit document-layer expectations.

    They are vectors, not conformance successes: the expectation describes the
    affected API operation and remains separate from observed engine behaviour.
    """
    requirement = ("security-dictionary-integration",)
    p, key = parameters(4, b"", b"owner-secret")
    base = encrypted_objects(p, key, "AESV2")
    changes = [
        ("identity-shadow", lambda d: d.replace(b"/CF <<", b"/CF << /Identity 7 ", 1),
         "open and render without processing the shadow Identity entry"),
        ("auth-event-default", lambda d: d.replace(b"/AuthEvent /DocOpen", b"/AuthEvent /EFOpen"),
         "StmF/StrF use DocOpen despite EFOpen; open and render"),
        ("v4-no-length", lambda d: d.replace(b"/Length 128 ", b"", 1),
         "use the fixed 128-bit V=4 file key; open and render"),
        ("crypt-filter-type", lambda d: d.replace(b"/CFM /AESV2", b"/Type /Wrong /CFM /AESV2"),
         "reject the crypt filter Type in strict mode"),
        ("crypt-filter-no-length", lambda d: d.replace(b"/Length 16 ", b"", 1),
         "reject the missing required crypt-filter Length in strict mode"),
        ("standard-filter-name", lambda d: d.replace(b"StdCF", b"OtherCF"),
         "report or reject a filter name outside standard-handler support"),
    ]
    def packed(objects, trailer=None, modern=False):
        trailer = (b"/Encrypt 5 0 R /ID ["+hex_string(ID)+b" "+hex_string(ID)+b"] ") if trailer is None else trailer
        data = document(objects, trailer)
        return data.replace(b"%PDF-1.7", b"%PDF-2.0", 1) if modern else data
    for name, change, expected in changes:
        objects = list(base); objects[4] = change(objects[4])
        yield Vector("dictionary-"+name, {".pdf": packed(objects)}, requirement, expected,
                     extra={"operation": "Document::open with strict recovery", "expectation": expected})
    # None delegates to the security handler; it is not the Identity name.
    objects = encrypted_objects(p, key, "Identity")
    objects[4] = encryption_dictionary(p, "V2", False).replace(b"/CFM /V2", b"/CFM /None")
    yield Vector("dictionary-cfm-none", {".pdf": packed(objects)}, requirement,
                 "None requires handler dispatch; Standard must not silently map it to Identity",
                 extra={"operation": "Document::open with strict recovery", "expectation": "unsupported or rejected Standard CFM None"})
    # Only the catalog's document-level metadata is excluded, not every object
    # whose Type happens to be Metadata (Table 21, Algorithm 2(f)).
    p, key = parameters(4, b"", b"owner-secret", metadata=False)
    objects = encrypted_objects(p, key, "AESV2")
    objects[0] = objects[0].replace(b" >>", b" /OtherMetadata 7 0 R >>", 1)
    plain = b"private metadata"
    objects.append(stream(encrypt(key, "AESV2", 7, plain), b"/Type /Metadata /Subtype /XML "))
    yield Vector("dictionary-other-metadata", {".pdf": packed(objects), ".expected": plain}, requirement,
                 "Only document-level metadata is exempt; object 7 still decrypts",
                 extra={"operation": "stream_data(object 7 0)", "expectation": "bytes equal .expected"})
    p, key = parameters(2, b"", b"owner-secret")
    objects = encrypted_objects(p, key, "V2")
    objects[4] = objects[4][:-2]+b" /EncryptMetadata false >>"
    yield Vector("dictionary-legacy-metadata", {".pdf": packed(objects), ".expected": b"<metadata>fixture</metadata>"}, requirement,
                 "EncryptMetadata is meaningful only for V=4/5; object 6 still decrypts for V=1",
                 extra={"operation": "stream_data(object 6 0)", "expectation": "bytes equal .expected"})
    p, key = parameters(6, b"", b"owner-secret")
    objects = encrypted_objects({**p, "permissions": p["permissions"] ^ 4}, key, "AESV3")
    yield Vector("dictionary-perms-mismatch", {".pdf": packed(objects, modern=True)}, requirement,
                 "An authenticated permissions disagreement must not disappear at the document boundary",
                 extra={"operation": "strict and repair Document::open", "expectation": "strict rejection or explicit reported recovery, never silent success"})
    objects = encrypted_objects(p, key, "AESV3")
    for name, trailer in [("absent", b"/Encrypt 5 0 R "), ("one", b"/Encrypt 5 0 R /ID ["+hex_string(ID)+b"] "),
                          ("short", b"/Encrypt 5 0 R /ID [<00> <00>] ")]:
        yield Vector("dictionary-id-"+name, {".pdf": packed(objects, trailer, modern=True)}, requirement,
                     "Table 15 requires two direct identifiers of at least 16 bytes for encrypted files",
                     extra={"operation": "Document::open with strict recovery", "expectation": "syntax error for malformed required ID"})
