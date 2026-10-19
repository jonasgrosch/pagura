//! FIPS 197-upd1 printed tables and Appendix A/B/C numeric examples, and
//! SP 800-38A Appendix F.2 CBC examples.
use super::*;
use pagura_testkit::{fixture, fixture_text, unhex, unhex_array};

fn cipher(key: &[u8]) -> Aes {
    match key.len() {
        16 => Aes::new128(&key.try_into().unwrap()),
        32 => Aes::new256(&key.try_into().unwrap()),
        length => panic!("no AES key has {length} bytes"),
    }
}

fn blocks(bytes: &mut [u8]) -> &mut [[u8; 16]] {
    let (blocks, rest) = bytes.as_chunks_mut::<16>();
    assert_eq!(rest.len(), 0, "partial block");
    blocks
}

#[test]
fn every_substitution_and_product_matches_printed_tables() {
    assert_eq!(
        SBOX.as_slice(),
        fixture("crypto", "aes-substitution-tables.forward")
    );
    assert_eq!(
        INV_SBOX.as_slice(),
        fixture("crypto", "aes-substitution-tables.inverse")
    );
    // FIPS 197 Table 5 supplies the nonzero leading byte of each round constant.
    let mut rcon = 1;
    for expected in [1, 2, 4, 8, 16, 32, 64, 128, 0x1b, 0x36] {
        assert_eq!(rcon, expected);
        rcon = MUL2[usize::from(rcon)];
    }
    // FIPS 197 4.2 works {57} • {13} = {fe} through xtime.
    assert_eq!(multiply(0x57, 0x13), 0xfe);
    for (table, factor) in [
        (&MUL2, 2),
        (&MUL3, 3),
        (&MUL9, 9),
        (&MUL11, 11),
        (&MUL13, 13),
        (&MUL14, 14),
    ] {
        for value in 0..=u8::MAX {
            assert_eq!(table[usize::from(value)], multiply(value, factor));
        }
    }
}

#[test]
fn appendix_a_complete_128_and_256_bit_key_expansions() {
    for (name, rounds) in [("aes128-key-expansion", 10), ("aes256-key-expansion", 14)] {
        let key = fixture("crypto", &format!("{name}.key"));
        let aes = cipher(&key);
        let expected = fixture("crypto", &format!("{name}.expanded"));
        assert_eq!(aes.rounds, rounds);
        assert_eq!(aes.keys[..=rounds].as_flattened(), expected, "{name}");
        assert_eq!(aes.cipher_key(), key, "{name}");
    }
}

#[test]
fn appendix_b_all_intermediate_states_and_inverse_transformations() {
    let plain: [u8; 16] = fixture("crypto", "aes128-cipher-example.plain")
        .try_into()
        .unwrap();
    let ciphertext: [u8; 16] = fixture("crypto", "aes128-cipher-example.cipher")
        .try_into()
        .unwrap();
    let aes = cipher(&fixture("crypto", "aes128-cipher-example.key"));
    let mut block = plain;
    let mut count = 0;
    for line in fixture_text("crypto", "aes128-cipher-example.states").lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let round: usize = fields[0].parse().unwrap();
        let before = block;
        let mut inverse = match fields[1] {
            "start" => {
                xor(&mut block, &aes.keys[round - 1]);
                block
            }
            "sub" => {
                sub_bytes(&mut block);
                block
            }
            "shift" => {
                shift_rows(&mut block);
                block
            }
            "mix" => {
                mix_columns(&mut block);
                block
            }
            step => panic!("unknown step {step}"),
        };
        assert_eq!(block, unhex_array::<16>(fields[2]), "state {count}");
        match fields[1] {
            "start" => xor(&mut inverse, &aes.keys[round - 1]),
            "sub" => inv_sub_bytes(&mut inverse),
            "shift" => inv_shift_rows(&mut inverse),
            _ => inv_mix_columns(&mut inverse),
        }
        assert_eq!(inverse, before, "inverse state {count}");
        count += 1;
    }
    assert_eq!(count, 39);
    xor(&mut block, &aes.keys[10]);
    assert_eq!(block, ciphertext);
    // Exercise the public round order against the fixed known answers in both
    // directions, independently of the intermediate-state walk above.
    let mut encrypted = plain;
    aes.encrypt_block(&mut encrypted);
    assert_eq!(encrypted, ciphertext);
    let mut decrypted = ciphertext;
    aes.decrypt_block(&mut decrypted);
    assert_eq!(decrypted, plain);
}

#[test]
fn appendix_c_example_vectors() {
    let plain = unhex_array::<16>("00112233445566778899aabbccddeeff");
    for (key, expected) in [
        (
            "000102030405060708090a0b0c0d0e0f",
            "69c4e0d86a7b0430d8cdb78070b4c55a",
        ),
        (
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "8ea2b7ca516745bfeafc49904b496089",
        ),
    ] {
        let aes = cipher(&unhex(key));
        let mut block = plain;
        aes.encrypt_block(&mut block);
        assert_eq!(block, unhex_array::<16>(expected));
        aes.decrypt_block(&mut block);
        assert_eq!(block, plain);
    }
}

#[test]
fn sp800_38a_appendix_f2_cbc_encrypt_decrypt_and_intermediate_blocks() {
    for bits in [128, 256] {
        let name = format!("cbc-aes{bits}-f2");
        let aes = cipher(&fixture("crypto", &format!("{name}.key")));
        let iv: [u8; 16] = fixture("crypto", &format!("{name}.iv")).try_into().unwrap();
        let plain = fixture("crypto", &format!("{name}.plain"));
        let ciphertext = fixture("crypto", &format!("{name}.cipher"));
        let chained = fixture("crypto", &format!("{name}.chained"));
        let mut encrypted = plain.clone();
        aes.encrypt_cbc(&iv, blocks(&mut encrypted));
        assert_eq!(encrypted, ciphertext, "{name}");
        // Fixed published ciphertext, independent of the encryption result.
        let mut decrypted = ciphertext.clone();
        aes.decrypt_cbc(&iv, blocks(&mut decrypted));
        assert_eq!(decrypted, plain, "{name}");
        let mut previous = iv;
        for ((input, output), chained) in plain
            .as_chunks::<16>()
            .0
            .iter()
            .zip(ciphertext.as_chunks::<16>().0)
            .zip(chained.as_chunks::<16>().0)
        {
            let mut block = *chained;
            aes.encrypt_block(&mut block);
            assert_eq!(&block, output, "{name}");
            let mut block = *output;
            aes.decrypt_block(&mut block);
            assert_eq!(&block, chained, "{name}");
            // SP 800-38A 6.2: the input block is plaintext XOR IV or the
            // previous ciphertext block.
            let mut mixed = *input;
            xor(&mut mixed, &previous);
            assert_eq!(&mixed, chained, "{name}");
            previous = *output;
        }
        for split in (0..=plain.len()).step_by(16) {
            let chain: [u8; 16] = if split == 0 {
                iv
            } else {
                ciphertext[split - 16..split].try_into().unwrap()
            };
            let mut encrypted = plain.clone();
            let (head, tail) = encrypted.split_at_mut(split);
            aes.encrypt_cbc(&iv, blocks(head));
            aes.encrypt_cbc(&chain, blocks(tail));
            assert_eq!(encrypted, ciphertext, "{name} split {split}");
            let mut decrypted = ciphertext.clone();
            let (head, tail) = decrypted.split_at_mut(split);
            aes.decrypt_cbc(&iv, blocks(head));
            aes.decrypt_cbc(&chain, blocks(tail));
            assert_eq!(decrypted, plain, "{name} split {split}");
        }
    }
}

#[test]
fn every_padding_length_and_invalid_padding() {
    for count in 1..=16u8 {
        let mut block = [0; 16];
        block[16 - usize::from(count)..].fill(count);
        assert_eq!(pkcs7_padding(&block).unwrap(), usize::from(count));
    }
    let mut changed = [2; 16];
    changed[14] = 3;
    for block in [[0; 16], [17; 16], [0xff; 16], changed] {
        let error = pkcs7_padding(&block).unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Syntax);
        assert_eq!(error.position, Position::Buffer(15));
    }
}

#[test]
fn debug_never_prints_the_key_schedule() {
    assert_eq!(
        format!("{:?}", Aes::new128(&[0x5a; 16])),
        "Aes { rounds: 10, .. }"
    );
}
