"""Independent integer/affine ECDSA oracle; Python standard library only.

Parameters: NIST SP 800-186 sections 3.2.1.3--3.2.1.5.
Test keys and nonces are public, deterministic test data only.
No Pagura code or output is imported or executed.
"""
import hashlib
import random
import sys

CURVES = {
    "P256": (
        2**256 - 2**224 + 2**192 + 2**96 - 1,
        int("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551", 16),
        int("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b", 16),
        int("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296", 16),
        int("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5", 16),
    ),
    "P384": (
        2**384 - 2**128 - 2**96 + 2**32 - 1,
        int("ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973", 16),
        int("b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef", 16),
        int("aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7", 16),
        int("3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f", 16),
    ),
    "P521": (
        2**521 - 1,
        int("1fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa51868783bf2f966b7fcc0148f709a5d03bb5c9b8899c47aebb6fb71e91386409", 16),
        int("51953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00", 16),
        int("c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66", 16),
        int("11839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef42640c550b9013fad0761353c7086a272c24088be94769fd16650", 16),
    ),
}


def add(a, b, p):
    if a is None:
        return b
    if b is None:
        return a
    x, y = a
    u, v = b
    if x == u:
        if (y + v) % p == 0:
            return None
        slope = (3*x*x - 3) * pow(2*y, -1, p) % p
    else:
        slope = (v-y) * pow(u-x, -1, p) % p
    out_x = (slope*slope - x - u) % p
    return out_x, (slope*(x-out_x)-y) % p


def multiply(k, point, p):
    result = None
    while k:
        if k & 1:
            result = add(result, point, p)
        point = add(point, point, p)
        k >>= 1
    return result


def verify(name, digest, point, r, s):
    p, n, b, gx, gy = CURVES[name]
    x, y = point
    if not (0 < r < n and 0 < s < n and 0 <= x < p and 0 <= y < p):
        return False
    if (y*y - x*x*x + 3*x - b) % p:
        return False
    e = int.from_bytes(digest, 'big') >> max(0, 8*len(digest)-n.bit_length())
    inverse = pow(s, -1, n)
    result = add(multiply(e*inverse % n, (gx, gy), p),
                 multiply(r*inverse % n, point, p), p)
    return result is not None and result[0] % n == r


def point_bytes(name, point, compressed=False):
    size = (CURVES[name][0].bit_length() + 7) // 8
    x, y = point
    return (bytes([2 | (y & 1)]) + x.to_bytes(size, "big") if compressed else
            b"\x04" + x.to_bytes(size, "big") + y.to_bytes(size, "big"))


def tlv(tag, data):
    return bytes([tag]) + (bytes([len(data)]) if len(data) < 128 else
                           bytes([0x81, len(data)])) + data


def signature(r, s):
    def integer(value):
        data = value.to_bytes(max(1, (value.bit_length()+7)//8), "big")
        if data[0] & 128:
            data = b"\0" + data
        return tlv(2, data)
    return tlv(0x30, integer(r) + integer(s))


def emit(source, case, name, digest, key, sig, expected):
    """One tab-separated vector line, as pagura-crypto's tests read it."""
    return "\t".join([source, str(case), name, digest.hex(), key.hex(), sig.hex(), str(int(expected))])


# Seeded random cases beside the exhaustive edge pairs; each curve's signature
# cases cover every digest length twice, both point encodings and the all-zero
# and all-one digests.
RANDOM_PAIRS = 16
SIGNATURE_CASES = 14


def arithmetic():
    rng = random.Random(0x186_5)
    for name, (p, n, _, _, _) in CURVES.items():
        for label, m in [("field", p), ("scalar", n)]:
            edges = [0, 1, 2, m-1, m-2, m//2, (1 << (m.bit_length()-1))-1]
            pairs = [(a, b) for a in edges for b in edges]
            pairs += [(rng.randrange(m), rng.randrange(m)) for _ in range(RANDOM_PAIRS)]
            width = 2 * ((m.bit_length() + 7) // 8)
            for a, b in pairs:
                values = [a, b, (a+b) % m, (a-b) % m, a*b % m,
                          pow(a, -1, m) if a else 0]
                yield "\t".join([name, label, *(f"{v:0{width}x}" for v in values)])


def vectors():
    rng = random.Random(0x5EC1)
    for name, (p, n, b, gx, gy) in CURVES.items():
        assert (gy*gy - gx**3 + 3*gx - b) % p == 0
        assert multiply(n, (gx, gy), p) is None
        for case in range(SIGNATURE_CASES):
            d = rng.randrange(1, n)
            k = rng.randrange(1, n)
            point = multiply(d, (gx, gy), p)
            # Includes all-zero, all-one and long prehashes; P-521 must
            # truncate at bit 521 rather than keeping 66 whole bytes.
            digest = (bytes(80) if case == 0 else bytes([255])*80 if case == 1 else
                      hashlib.shake_256(rng.randbytes(32)).digest([20, 28, 32, 48, 64, 66, 80][case % 7]))
            e = int.from_bytes(digest, 'big') >> max(0, 8*len(digest)-n.bit_length())
            r = multiply(k, (gx, gy), p)[0] % n  # pyright: ignore[reportOptionalSubscript]
            s = (e + r*d) * pow(k, -1, n) % n
            assert verify(name, digest, point, r, s)
            key = point_bytes(name, point, case % 2 == 0)
            yield emit('python-affine', f'{case}-valid', name, digest, key, signature(r, s), True)
            yield emit('python-affine', f'{case}-high-low-s', name, digest, key, signature(r, n-s), True)
            changed_r = (r % (n-1)) + 1
            assert not verify(name, digest, point, changed_r, s)
            yield emit('python-affine', f'{case}-changed-r', name, digest, key, signature(changed_r, s), False)
            bad_point = (point[0], (point[1]+1) % p)  # pyright: ignore[reportOptionalSubscript]
            assert not verify(name, digest, bad_point, r, s)
            bad_key = point_bytes(name, bad_point)
            yield emit('python-affine', f'{case}-off-curve', name, digest, bad_key, signature(r, s), False)
        # Q=G, u=1 and v=-1: verification result is infinity and must fail.
        yield emit('python-affine', 'identity-result', name, b'\1', point_bytes(name, (gx, gy)), signature(n-1, 1), False)
        # A prehash entirely reduced to zero is legitimate mathematics.
        r = gx % n
        yield emit('python-affine', 'zero-digest', name, bytes(64), point_bytes(name, (gx, gy)), signature(r, r), True)


if __name__ == '__main__':
    for line in {'arithmetic': arithmetic, 'vectors': vectors}[sys.argv[1]]():
        print(line)
