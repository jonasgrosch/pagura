//! FIPS 197-upd1 printed tables and Appendix A/B numeric examples.
use super::*;
use pagura_testkit::fixture;

#[test]
fn every_substitution_and_round_constant_matches_printed_tables() {
    assert_eq!(
        TABLES.0.as_slice(),
        fixture("crypto", "aes-substitution-tables.forward")
    );
    assert_eq!(
        TABLES.1.as_slice(),
        fixture("crypto", "aes-substitution-tables.inverse")
    );
    // Table 5 supplies the nonzero leading byte of each round constant.
    let mut rcon = 1;
    for expected in [1, 2, 4, 8, 16, 32, 64, 128, 0x1b, 0x36] {
        assert_eq!(rcon, expected);
        rcon = multiply(rcon, 2);
    }
}

#[test]
fn appendix_a_complete_128_and_256_bit_key_expansions() {
    for (name, rounds) in [("aes128-key-expansion", 10), ("aes256-key-expansion", 14)] {
        let key = fixture("crypto", &format!("{name}.key"));
        let aes = if rounds == 10 {
            Aes::new128(key.as_slice().try_into().unwrap())
        } else {
            Aes::new256(key.as_slice().try_into().unwrap())
        };
        let expected = fixture("crypto", &format!("{name}.expanded"));
        assert_eq!(aes.rounds, rounds);
        assert_eq!(aes.keys[..=rounds].as_flattened(), expected, "{name}");
    }
}

#[test]
fn appendix_b_all_intermediate_states_and_inverse_transformations() {
    let plain: [u8; 16] = fixture("crypto", "aes128-cipher-example.plain")
        .try_into()
        .unwrap();
    let cipher: [u8; 16] = fixture("crypto", "aes128-cipher-example.cipher")
        .try_into()
        .unwrap();
    let aes = Aes::new128(
        &fixture("crypto", "aes128-cipher-example.key")
            .try_into()
            .unwrap(),
    );
    let states = fixture("crypto", "aes128-cipher-example.states");
    let mut block = plain;
    let mut count = 0;
    for line in std::str::from_utf8(&states).unwrap().lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let round: usize = fields[0].parse().unwrap();
        let before = block;
        match fields[1] {
            "start" => add_key(&mut block, &aes.keys[round - 1]),
            "sub" => {
                for byte in &mut block {
                    *byte = TABLES.0[usize::from(*byte)];
                }
            }
            "shift" => shift_rows(&mut block, false),
            "mix" => mix_columns(&mut block, false),
            _ => unreachable!(),
        }
        assert_eq!(block, tests::bytes::<16>(fields[2]), "state {count}");
        let mut inverse = block;
        match fields[1] {
            "start" => add_key(&mut inverse, &aes.keys[round - 1]),
            "sub" => {
                for byte in &mut inverse {
                    *byte = TABLES.1[usize::from(*byte)];
                }
            }
            "shift" => shift_rows(&mut inverse, true),
            "mix" => mix_columns(&mut inverse, true),
            _ => unreachable!(),
        }
        assert_eq!(inverse, before, "inverse state {count}");
        count += 1;
    }
    assert_eq!(count, 39);
    add_key(&mut block, &aes.keys[10]);
    assert_eq!(block, cipher);
    // Exercise the public round order against the fixed known answers in both
    // directions, independently of the intermediate-state walk above.
    let mut encrypted = plain;
    aes.encrypt_block(&mut encrypted);
    assert_eq!(encrypted, cipher);
    let mut decrypted = cipher;
    aes.decrypt_block(&mut decrypted);
    assert_eq!(decrypted, plain);
}

#[test]
fn sp800_38a_appendix_f2_cbc_encrypt_decrypt_and_intermediate_blocks() {
    for bits in [128, 256] {
        let name = format!("cbc-aes{bits}-f2");
        let key = fixture("crypto", &format!("{name}.key"));
        let iv: [u8; 16] = fixture("crypto", &format!("{name}.iv")).try_into().unwrap();
        let aes = if bits == 128 {
            Aes::new128(key.as_slice().try_into().unwrap())
        } else {
            Aes::new256(key.as_slice().try_into().unwrap())
        };
        let plain = fixture("crypto", &format!("{name}.plain"));
        let cipher = fixture("crypto", &format!("{name}.cipher"));
        let chained = fixture("crypto", &format!("{name}.chained"));
        let mut encrypted = plain.clone();
        aes.encrypt_cbc(&iv, &mut encrypted).unwrap();
        assert_eq!(encrypted, cipher, "{name}");
        // Fixed published ciphertext, independent of the encryption result.
        let mut decrypted = cipher.clone();
        aes.decrypt_cbc(&iv, &mut decrypted).unwrap();
        assert_eq!(decrypted, plain, "{name}");
        let mut previous = iv;
        for ((input, output), chained) in plain
            .as_chunks::<16>()
            .0
            .iter()
            .zip(cipher.as_chunks::<16>().0)
            .zip(chained.as_chunks::<16>().0)
        {
            let mut block = *chained;
            aes.encrypt_block(&mut block);
            assert_eq!(&block, output, "{name}");
            let mut block = *output;
            aes.decrypt_block(&mut block);
            assert_eq!(&block, chained, "{name}");
            // Section 6.2: input block is plaintext XOR IV/previous ciphertext.
            let mut mixed = *input;
            add_key(&mut mixed, &previous);
            assert_eq!(&mixed, chained, "{name}");
            previous = *output;
        }
        for split in (0..=plain.len()).step_by(16) {
            let mut encrypted = plain.clone();
            aes.encrypt_cbc(&iv, &mut encrypted[..split]).unwrap();
            let chain = if split == 0 {
                &iv
            } else {
                encrypted[split - 16..split].try_into().unwrap()
            };
            let chain = *chain;
            aes.encrypt_cbc(&chain, &mut encrypted[split..]).unwrap();
            assert_eq!(encrypted, cipher, "{name} split {split}");
            let mut decrypted = cipher.clone();
            let chain = if split == 0 {
                &iv
            } else {
                cipher[split - 16..split].try_into().unwrap()
            };
            aes.decrypt_cbc(&iv, &mut decrypted[..split]).unwrap();
            aes.decrypt_cbc(chain, &mut decrypted[split..]).unwrap();
            assert_eq!(decrypted, plain, "{name} split {split}");
        }
    }
}
