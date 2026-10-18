//! Known answers and streaming for every hash, and the padded single-block
//! examples of RFC 1321 and FIPS 180-4.
use super::*;
use pagura_testkit::{fixture, fixture_text, hex};

fn hex_digest<D: Digest>(data: &[u8]) -> String {
    hex(D::digest(data).as_ref())
}

/// Every split around both block sizes and both length-field boundaries,
/// with empty updates in between, must match the one-shot digest.
fn assert_streaming<D: Digest>(input: &[u8]) {
    let expected = D::digest(input);
    for split in 0..=input.len() {
        let mut hash = D::default();
        hash.update(&input[..split]);
        hash.update(&[]);
        let copy = hash.clone();
        hash.update(&input[split..]);
        assert_eq!(hash.finalize().as_ref(), expected.as_ref(), "split {split}");
        let mut copy = copy;
        copy.update(&input[split..]);
        assert_eq!(
            copy.finalize().as_ref(),
            expected.as_ref(),
            "clone at {split}"
        );
    }
}

#[test]
fn independent_hashlib_padding_boundaries() {
    let data = fixture("crypto", "hash-padding-boundaries.input");
    for line in fixture_text("crypto", "hash-padding-boundaries.digests").lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let length: usize = fields[0].parse().unwrap();
        let input = &data[..length];
        assert_eq!(hex_digest::<Md5>(input), fields[1], "length {length}");
        assert_eq!(hex_digest::<Sha256>(input), fields[2], "length {length}");
        assert_eq!(hex_digest::<Sha384>(input), fields[3], "length {length}");
        assert_eq!(hex_digest::<Sha512>(input), fields[4], "length {length}");
    }
}

#[test]
fn fips_180_4_examples_and_one_million_a() {
    let two_block = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    let four_block = concat!(
        "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno",
        "ijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
    )
    .as_bytes();
    let million = vec![b'a'; 1_000_000];
    for (input, sha256, sha384, sha512) in [
        (
            b"".as_slice(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        ),
        (
            b"abc".as_slice(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
        (
            two_block.as_slice(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            "3391fdddfc8dc7393707a65b1b4709397cf8b1d162af05abfe8f450de5f36bc6b0455a8520bc4e6f5fe95b1fe3c8452b",
            "204a8fc6dda82f0a0ced7beb8e08a41657c16ef468b228a8279be331a703c33596fd15c13b1b07f9aa1d3bea57789ca031ad85c7a71dd70354ec631238ca3445",
        ),
        (
            four_block,
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039",
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909",
        ),
        (
            million.as_slice(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985",
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b",
        ),
    ] {
        assert_eq!(hex_digest::<Sha256>(input), sha256, "{} bytes", input.len());
        assert_eq!(hex_digest::<Sha384>(input), sha384, "{} bytes", input.len());
        assert_eq!(hex_digest::<Sha512>(input), sha512, "{} bytes", input.len());
    }
    assert_eq!(
        hex_digest::<Md5>(&million),
        "7707d6ae4e027c70eea2a935c2296f21"
    );
}

#[test]
fn every_hash_streams_across_block_and_length_field_boundaries() {
    let input: Vec<u8> = (0..=255).cycle().take(521).collect();
    assert_streaming::<Md5>(&input);
    assert_streaming::<Sha1>(&input);
    assert_streaming::<Sha224>(&input);
    assert_streaming::<Sha256>(&input);
    assert_streaming::<Sha384>(&input);
    assert_streaming::<Sha512>(&input);
}

#[test]
fn abc_padding_examples_and_hashlib_digest_oracles() {
    // FIPS 180-4 5.1.1/5.1.2 print the padded abc messages.
    // Their digest values come from hashlib, not the separately linked examples.
    let input = fixture("crypto", "hash-padding-abc.input");
    let expected = fixture_text("crypto", "hash-padding-abc.digests");
    let expected: Vec<_> = expected.split_whitespace().collect();
    let block = |name: &str| fixture("crypto", &format!("hash-padding-abc.{name}-padded"));
    let mut md5_state = Md5::new().0.state;
    md5::compress(&mut md5_state, &block("md5").try_into().unwrap());
    let md5_bytes: Vec<_> = md5_state.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut sha256_state = Sha256::new().0.state;
    sha2::compress256(&mut sha256_state, &block("sha256").try_into().unwrap());
    let sha512_block = block("sha512").try_into().unwrap();
    let mut sha384_state = Sha384::new().0.state;
    sha2::compress512(&mut sha384_state, &sha512_block);
    let mut sha512_state = Sha512::new().0.state;
    sha2::compress512(&mut sha512_state, &sha512_block);
    for (compressed, finalized, expected) in [
        (md5_bytes, hex_digest::<Md5>(&input), expected[0]),
        (
            be32::<32>(&sha256_state).to_vec(),
            hex_digest::<Sha256>(&input),
            expected[1],
        ),
        (
            be64::<48>(&sha384_state).to_vec(),
            hex_digest::<Sha384>(&input),
            expected[2],
        ),
        (
            be64::<64>(&sha512_state).to_vec(),
            hex_digest::<Sha512>(&input),
            expected[3],
        ),
    ] {
        assert_eq!(hex(&compressed), expected);
        assert_eq!(finalized, expected);
    }
}

#[test]
fn debug_never_prints_buffered_input() {
    let mut hash = Sha256::new();
    hash.update(b"password");
    assert_eq!(format!("{hash:?}"), "Sha256 { .. }");
    assert_eq!(format!("{:?}", Md5::new()), "Md5 { .. }");
}
