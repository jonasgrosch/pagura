//! Batched Adler arithmetic for stream decoding (RFC 1950, 2.2).

// RFC 1950's modulus and a maximum of 5552 unreduced bytes keep both u32
// sums in range, even when each byte is 255. A call boundary need not reduce.
pub(super) fn update(mut bytes: &[u8], a: &mut u32, b: &mut u32, pending: &mut usize) {
    while !bytes.is_empty() {
        let count = bytes.len().min(5552 - *pending);
        let (head, tail) = bytes.split_at(count);
        for &byte in head {
            *a += u32::from(byte);
            *b += *a;
        }
        *pending += count;
        if *pending == 5552 {
            *a %= 65521;
            *b %= 65521;
            *pending = 0;
        }
        bytes = tail;
    }
}

#[cfg(test)]
mod tests {
    use super::update;
    #[test]
    fn arbitrary_emission_boundaries_preserve_the_checksum() {
        for size in [0, 1, 32, 5551, 5552, 5553, 16384] {
            let bytes = vec![255; size];
            let mut expected = (1_u32, 0_u32);
            for &v in &bytes {
                expected.0 = (expected.0 + u32::from(v)) % 65521;
                expected.1 = (expected.1 + expected.0) % 65521;
            }
            for chunk in [1, 7, 258, 5552, 8192] {
                let (mut a, mut b, mut pending) = (1, 0, 0);
                for bytes in bytes.chunks(chunk) {
                    update(bytes, &mut a, &mut b, &mut pending);
                }
                assert_eq!((a % 65521, b % 65521), expected);
            }
        }
    }
}
