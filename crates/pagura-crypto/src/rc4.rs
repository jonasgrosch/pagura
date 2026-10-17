//! RC4 as specified by the legacy PDF algorithms; vectors from RFC 6229.
use pagura_core::{Error, Result};

/// Stateful legacy RC4 cipher. Debug output never includes its state.
#[derive(Clone)]
pub struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}
impl std::fmt::Debug for Rc4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rc4 { .. }")
    }
}
impl Rc4 {
    /// Runs the RC4 key scheduling algorithm for a 1–256 byte key.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an empty or overlong key.
    pub fn new(key: &[u8]) -> Result<Self> {
        if key.is_empty() || key.len() > 256 {
            return Err(Error::invalid_argument(
                "RC4 key must contain 1 to 256 bytes",
            ));
        }
        let mut state = [0; 256];
        for (slot, value) in state.iter_mut().zip(0..=u8::MAX) {
            *slot = value;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
            state.swap(i, usize::from(j));
        }
        Ok(Self { state, i: 0, j: 0 })
    }
    /// XORs bytes with the next keystream bytes, preserving streaming state.
    pub fn apply_keystream(&mut self, data: &mut [u8]) {
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
    #[test]
    fn rfc6229_key1_offsets_and_streaming() {
        let mut cipher = Rc4::new(&[1, 2, 3, 4, 5]).unwrap();
        let mut data = [0; 272];
        for chunk in data.chunks_mut(7) {
            cipher.apply_keystream(chunk);
        }
        assert_eq!(
            &data[..16],
            &[
                0xb2, 0x39, 0x63, 0x05, 0xf0, 0x3d, 0xc0, 0x27, 0xcc, 0xc3, 0x52, 0x4a, 0x0a, 0x11,
                0x18, 0xa8
            ]
        );
        assert_eq!(
            &data[16..32],
            &[
                0x69, 0x82, 0x94, 0x4f, 0x18, 0xfc, 0x82, 0xd5, 0x89, 0xc4, 0x03, 0xa4, 0x7a, 0x0d,
                0x09, 0x19
            ]
        );
        assert_eq!(
            &data[256..272],
            &[
                0x1c, 0xfc, 0xf6, 0x2b, 0x03, 0xed, 0xdb, 0x64, 0x1d, 0x77, 0xdf, 0xcf, 0x7f, 0x8d,
                0x8c, 0x93
            ]
        );
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
            Rc4::new(&key).unwrap().apply_keystream(&mut output);
            for (actual, pair) in output.iter().zip(expected.as_bytes().as_chunks::<2>().0) {
                assert_eq!(
                    *actual,
                    u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                );
            }
        }
    }
    #[test]
    fn invalid_keys_and_debug_redaction() {
        assert!(Rc4::new(&[]).is_err());
        assert!(Rc4::new(&[0; 257]).is_err());
        assert_eq!(format!("{:?}", Rc4::new(b"secret").unwrap()), "Rc4 { .. }");
    }
}
