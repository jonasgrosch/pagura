//! Host protocol framing (ADR 0005) as plain bytes, for tests that drive a
//! session or the host binary: a frame is a little-endian `u32` body length,
//! then the body, which starts with a kind byte and a little-endian `u32`
//! request identifier. Payload layouts stay in each test, independent of
//! the host's own encoder.

/// Bytes of the kind and identifier at the start of every body.
const HEADER: usize = 5;

/// Builds one frame from its kind, identifier and payload.
///
/// # Panics
///
/// Panics if the body does not fit the `u32` length prefix.
pub fn frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let length = payload
        .len()
        .checked_add(HEADER)
        .and_then(|length| u32::try_from(length).ok())
        .unwrap_or_else(|| panic!("frame payload of {} bytes is too long", payload.len()));
    let mut bytes = Vec::with_capacity(payload.len() + HEADER + 4);
    bytes.extend(length.to_le_bytes());
    bytes.push(kind);
    bytes.extend(id.to_le_bytes());
    bytes.extend(payload);
    bytes
}

/// One frame of a byte stream, from [`split_frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    body: &'a [u8],
}

impl<'a> Frame<'a> {
    /// The whole body: kind, identifier and payload.
    pub fn body(&self) -> &'a [u8] {
        self.body
    }

    /// The kind byte.
    pub fn kind(&self) -> u8 {
        self.body[0]
    }

    /// The request identifier.
    pub fn id(&self) -> u32 {
        let mut id = [0; 4];
        id.copy_from_slice(&self.body[1..HEADER]);
        u32::from_le_bytes(id)
    }

    /// The payload after the kind and identifier.
    pub fn payload(&self) -> &'a [u8] {
        &self.body[HEADER..]
    }
}

/// Splits a byte stream into its frames.
///
/// # Panics
///
/// Panics if the stream ends inside a frame or a body is shorter than the
/// kind and identifier.
#[track_caller]
pub fn split_frames(mut bytes: &[u8]) -> Vec<Frame<'_>> {
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        let Some((length, rest)) = bytes.split_first_chunk::<4>() else {
            panic!("stream ends inside a frame length");
        };
        let length = usize::try_from(u32::from_le_bytes(*length))
            .unwrap_or_else(|_| panic!("frame length exceeds the address space"));
        let Some((body, rest)) = rest.split_at_checked(length) else {
            panic!("stream ends inside a {length}-byte frame body");
        };
        assert!(
            body.len() >= HEADER,
            "frame body of {length} bytes has no header"
        );
        frames.push(Frame { body });
        bytes = rest;
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::{frame, split_frames};

    #[test]
    fn frames_round_trip_through_the_splitter() {
        let hello = frame(1, 1, &1_u32.to_le_bytes());
        // The handshake from the host's literal protocol oracle.
        assert_eq!(hello, [9, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0]);
        let mut stream = hello.clone();
        stream.extend(frame(130, 0x0102_0304, b"payload"));
        stream.extend(frame(255, 7, &[]));
        let frames = split_frames(&stream);
        let fields: Vec<_> = frames
            .iter()
            .map(|frame| (frame.kind(), frame.id(), frame.payload()))
            .collect();
        let expected: [(u8, u32, &[u8]); 3] = [
            (1, 1, &[1, 0, 0, 0]),
            (130, 0x0102_0304, b"payload"),
            (255, 7, b""),
        ];
        assert_eq!(fields, expected);
        assert_eq!(frames[0].body(), &hello[4..]);
        assert_eq!(split_frames(&[]), []);
    }

    #[test]
    #[should_panic(expected = "stream ends inside a 9-byte frame body")]
    fn truncated_streams_fail_loudly() {
        let hello = frame(1, 1, &[0; 4]);
        split_frames(&hello[..hello.len() - 1]);
    }

    #[test]
    #[should_panic(expected = "has no header")]
    fn bodies_without_a_header_fail_loudly() {
        split_frames(&[1, 0, 0, 0, 9]);
    }
}
