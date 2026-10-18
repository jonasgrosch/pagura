//! Legacy PDF RC4, corroborated by RFC 6229 section 2 test vectors.
//! RFC 6229 is informational; it does not specify the cipher algorithm.

use crate::secret::wipe;

/// Stateful legacy RC4 cipher. Debug output never includes its state, which
/// is cleared when dropped.
#[derive(Clone)]
pub(crate) struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}

impl std::fmt::Debug for Rc4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rc4 { .. }")
    }
}

impl Drop for Rc4 {
    fn drop(&mut self) {
        wipe(&mut self.state);
    }
}

impl Rc4 {
    /// Runs the RC4 key scheduling algorithm. The security handler passes
    /// keys of 5–16 bytes; RC4 accepts 1–256.
    pub(crate) fn new(key: &[u8]) -> Self {
        debug_assert!(
            (1..=256).contains(&key.len()),
            "RC4 keys have 1 to 256 bytes"
        );
        let mut state = [0; 256];
        for (slot, value) in state.iter_mut().zip(0..=u8::MAX) {
            *slot = value;
        }
        let mut j = 0u8;
        for (i, &byte) in (0..256).zip(key.iter().cycle()) {
            j = j.wrapping_add(state[i]).wrapping_add(byte);
            state.swap(i, usize::from(j));
        }
        Self { state, i: 0, j: 0 }
    }

    /// XORs bytes with the next keystream bytes, preserving streaming state.
    pub(crate) fn apply_keystream(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.state[usize::from(self.i)]);
            self.state.swap(usize::from(self.i), usize::from(self.j));
            let index =
                self.state[usize::from(self.i)].wrapping_add(self.state[usize::from(self.j)]);
            *byte ^= self.state[usize::from(index)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_testkit::{assert_bytes_eq, fixture, fixture_text, hex, unhex};

    #[test]
    fn rfc6229_key1_offsets_and_streaming() {
        let mut cipher = Rc4::new(&[1, 2, 3, 4, 5]);
        let mut data = [0; 272];
        for chunk in data.chunks_mut(7) {
            cipher.apply_keystream(chunk);
        }
        assert_eq!(hex(&data[..16]), "b2396305f03dc027ccc3524a0a1118a8");
        assert_eq!(hex(&data[16..32]), "6982944f18fc82d589c403a47a0d0919");
        assert_eq!(hex(&data[256..272]), "1cfcf62b03eddb641d77dfcf7f8d8c93");
    }

    #[test]
    fn rfc6229_key_sizes() {
        for (size, expected) in [
            (7, "293f02d47f37c9b633f2af5285feb46b"),
            (8, "97ab8a1bf0afb96132f2f67258da15a8"),
            (10, "ede3b04643e586cc907dc21851709902"),
            (16, "9ac7cc9a609d1ef7b2932899cde41b97"),
            (24, "0595e57fe5f0bb3c706edac8a4b2db11"),
            (32, "eaa6bd25880bf93d3f5d1e4ca2611d91"),
        ] {
            let key: Vec<u8> = (1..=size).collect();
            let mut output = [0; 16];
            Rc4::new(&key).apply_keystream(&mut output);
            assert_eq!(hex(&output), expected, "{size}-byte key");
        }
    }

    #[test]
    fn rfc6229_both_key_families_at_every_listed_offset() {
        for bits in [40, 128, 256] {
            for family in [1, 2] {
                let name = format!("rc4-rfc6229-{bits}-key{family}");
                let key = fixture("crypto", &format!("{name}.key"));
                let mut contiguous = vec![0; 4112];
                Rc4::new(&key).apply_keystream(&mut contiguous);
                let mut streamed = vec![0; 4112];
                let mut rc4 = Rc4::new(&key);
                for chunk in streamed.chunks_mut(13) {
                    rc4.apply_keystream(chunk);
                    rc4.apply_keystream(&mut []);
                }
                assert_bytes_eq(&streamed, &contiguous, &name);
                let mut count = 0;
                for line in fixture_text("crypto", &format!("{name}.samples")).lines() {
                    let (offset, expected) = line.split_once(' ').unwrap();
                    let offset: usize = offset.parse().unwrap();
                    let expected = unhex(expected);
                    assert_bytes_eq(
                        &contiguous[offset..offset + expected.len()],
                        &expected,
                        &format!("{name} offset {offset}"),
                    );
                    count += 1;
                }
                assert_eq!(count, 18);
            }
        }
    }

    #[test]
    #[should_panic(expected = "RC4 keys have 1 to 256 bytes")]
    fn empty_keys_violate_the_key_contract() {
        let _ = Rc4::new(&[]);
    }

    #[test]
    fn debug_is_redacted() {
        assert_eq!(format!("{:?}", Rc4::new(b"secret")), "Rc4 { .. }");
    }
}
