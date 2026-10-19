//! Reading request frames from a byte stream.

use crate::protocol::layout;
use pagura::{Error, Position};
use std::io::{self, Read};

/// What the stream delivered.
#[derive(Debug)]
pub(super) enum Incoming {
    /// A complete body of a well-sized frame.
    Body(Vec<u8>),
    /// EOF exactly between frames.
    Eof,
    /// A framing error, which ends the session.
    Malformed(Error),
}

/// Reads one frame, refusing a declared length over `max_body` before
/// reserving any memory for it.
pub(super) fn read(input: &mut impl Read, max_body: usize) -> io::Result<Incoming> {
    let framing = |error: Error| {
        Ok(Incoming::Malformed(
            error.at(Position::Buffer(layout::FRAME_START)),
        ))
    };
    let mut length = [0; layout::LENGTH_BYTES];
    let [first, rest @ ..] = &mut length;
    if !read_first(input, first)? {
        return Ok(Incoming::Eof);
    }
    if !fill(input, rest)? {
        return framing(Error::syntax("truncated frame length"));
    }
    let length = usize::try_from(u32::from_le_bytes(length)).unwrap_or(usize::MAX);
    if length < layout::HEADER_BYTES {
        return framing(Error::syntax("frame body is shorter than header"));
    }
    if length > max_body {
        return framing(Error::limit("request frame budget exceeded"));
    }
    let mut body = Vec::new();
    if body.try_reserve_exact(length).is_err() {
        return Ok(Incoming::Malformed(Error::limit(
            "request allocation failed",
        )));
    }
    body.resize(length, 0);
    if !fill(input, &mut body)? {
        return framing(Error::syntax("truncated frame body"));
    }
    Ok(Incoming::Body(body))
}

/// The identifier of a body, or zero if it is too short to have one.
pub(super) fn request_id(body: &[u8]) -> u32 {
    body.get(layout::ID_OFFSET..)
        .and_then(<[u8]>::first_chunk)
        .map_or(0, |id| u32::from_le_bytes(*id))
}

/// Reads the first byte of a frame, retrying interrupted reads; `false` at EOF.
fn read_first(input: &mut impl Read, byte: &mut u8) -> io::Result<bool> {
    loop {
        match input.read(std::slice::from_mut(byte)) {
            Ok(count) => return Ok(count > 0),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// Fills `bytes`; `false` at EOF.
fn fill(input: &mut impl Read, bytes: &mut [u8]) -> io::Result<bool> {
    match input.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Delivers one byte per read and interrupts every other call.
    struct Trickle<'a> {
        bytes: &'a [u8],
        interrupt: bool,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let Some((&byte, rest)) = self.bytes.split_first() else {
                return Ok(0);
            };
            let Some(slot) = buffer.first_mut() else {
                return Ok(0);
            };
            *slot = byte;
            self.bytes = rest;
            Ok(1)
        }
    }

    #[test]
    fn interrupted_reads_retry_until_a_frame_or_eof() {
        let frame = pagura_testkit::frame(1, 7, &[1, 0, 0, 0]);
        let mut input = Trickle {
            bytes: &frame,
            interrupt: false,
        };
        let Incoming::Body(body) = read(&mut input, 1024).unwrap() else {
            panic!("a complete frame");
        };
        assert_eq!(body, &frame[layout::LENGTH_BYTES..]);
        assert!(matches!(read(&mut input, 1024).unwrap(), Incoming::Eof));
        let mut cut: &[u8] = &frame[..frame.len() - 1];
        let Incoming::Malformed(error) = read(&mut cut, 1024).unwrap() else {
            panic!("a truncated frame");
        };
        assert_eq!(error.position, Position::Buffer(layout::FRAME_START));
    }

    #[test]
    fn identifiers_need_a_full_header() {
        assert_eq!(request_id(&[1, 2, 0, 0, 0]), 2);
        assert_eq!(request_id(&[1, 2, 0]), 0);
    }
}
