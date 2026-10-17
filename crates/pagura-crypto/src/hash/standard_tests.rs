//! Numeric examples and equations in RFC 1321 and FIPS 180-4.
use super::*;
use pagura_testkit::fixture;

fn words32(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_be_bytes(*bytes))
        .collect()
}
fn words64(data: &[u8]) -> Vec<u64> {
    data.as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| u64::from_be_bytes(*bytes))
        .collect()
}

#[test]
fn every_md5_constant_and_shift_matches_section_3_4() {
    let constants = fixture("crypto", "md5-round-constants.constants");
    let words: Vec<_> = constants
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    assert_eq!(MD5_K.as_slice(), words);
    let shifts = fixture("crypto", "md5-round-constants.shifts");
    assert_eq!(
        MD5_S.as_slice(),
        shifts.into_iter().map(u32::from).collect::<Vec<_>>()
    );
}

#[test]
fn every_sha2_constant_and_initial_word_matches_fractional_roots() {
    // The independent generator uses exact integer roots of prime numbers.
    assert_eq!(
        K32.as_slice(),
        words32(&fixture("crypto", "sha2-round-constants.sha256"))
    );
    assert_eq!(
        K64.as_slice(),
        words64(&fixture("crypto", "sha2-round-constants.sha512"))
    );
    assert_eq!(
        Sha256::new().state.as_slice(),
        words32(&fixture("crypto", "sha2-round-constants.iv256"))
    );
    assert_eq!(
        Sha384::new().state.as_slice(),
        words64(&fixture("crypto", "sha2-round-constants.iv384"))
    );
    assert_eq!(
        Sha512::new().state.as_slice(),
        words64(&fixture("crypto", "sha2-round-constants.iv512"))
    );
}

#[test]
fn abc_padding_examples_and_hashlib_digest_oracles() {
    // FIPS 180-4 sections 5.1.1/5.1.2 print the padded abc messages.
    // Their digest values come from hashlib, not the separately linked examples.
    let input = fixture("crypto", "hash-padding-abc.input");
    let expected = fixture("crypto", "hash-padding-abc.digests");
    let expected: Vec<_> = std::str::from_utf8(&expected)
        .unwrap()
        .split_whitespace()
        .collect();
    let mut md5_state = Md5::new().state;
    md5_compress(
        &mut md5_state,
        &fixture("crypto", "hash-padding-abc.md5-padded")
            .try_into()
            .unwrap(),
    );
    let md5_bytes: Vec<_> = md5_state.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut sha256_state = Sha256::new().state;
    sha32_compress(
        &mut sha256_state,
        &fixture("crypto", "hash-padding-abc.sha256-padded")
            .try_into()
            .unwrap(),
    );
    let sha256_bytes: Vec<_> = sha256_state
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
    let block = fixture("crypto", "hash-padding-abc.sha512-padded")
        .try_into()
        .unwrap();
    let mut sha384_state = Sha384::new().state;
    sha64_compress(&mut sha384_state, &block);
    let sha384_bytes: Vec<_> = sha384_state[..6]
        .iter()
        .flat_map(|word| word.to_be_bytes())
        .collect();
    let mut sha512_state = Sha512::new().state;
    sha64_compress(&mut sha512_state, &block);
    let sha512_bytes: Vec<_> = sha512_state
        .into_iter()
        .flat_map(u64::to_be_bytes)
        .collect();
    for (compressed, finalized, expected) in [
        (md5_bytes, md5(&input).to_vec(), expected[0]),
        (sha256_bytes, sha256(&input).to_vec(), expected[1]),
        (sha384_bytes, sha384(&input).to_vec(), expected[2]),
        (sha512_bytes, sha512(&input).to_vec(), expected[3]),
    ] {
        assert_eq!(tests::hex(&compressed), expected);
        assert_eq!(tests::hex(&finalized), expected);
    }
}
