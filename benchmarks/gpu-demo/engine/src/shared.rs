//! Explicit native host capability. Safe file writes; no mapping or Rust FFI.
use super::{Error, MAX_FRAME, Result, Session, word};
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, Write};
use std::path::Path;

pub struct Output {
    frame: File,
    packet: File,
}

struct Sink<'a> {
    file: &'a mut File,
    count: usize,
}

impl Write for Sink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME - self.count {
            return Err(io::Error::other("shared output exceeds 64 MiB"));
        }
        let written = self.file.write(bytes)?;
        self.count += written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Output {
    pub fn from_args() -> io::Result<Option<Self>> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.is_empty() {
            return Ok(None);
        }
        if args.len() != 3 || args[0] != "--shared-output" {
            return Err(io::Error::other("expected --shared-output FRAME PACKET"));
        }
        Ok(Some(Self::open(Path::new(&args[1]), Path::new(&args[2]))?))
    }

    fn open(frame: &Path, packet: &Path) -> io::Result<Self> {
        fn file(path: &Path) -> io::Result<File> {
            let file = OpenOptions::new().write(true).open(path)?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.len() != MAX_FRAME as u64 {
                return Err(io::Error::other(
                    "shared output must be a preallocated 64 MiB file",
                ));
            }
            Ok(file)
        }
        if frame == packet {
            return Err(io::Error::other("shared frame and packet must be distinct"));
        }
        Ok(Self {
            frame: file(frame)?,
            packet: file(packet)?,
        })
    }

    pub fn request(&mut self, session: &mut Session, request: &[u8]) -> Result<String> {
        let rgba = word(request, 0)? == 7;
        let file = if rgba {
            &mut self.frame
        } else {
            &mut self.packet
        };
        file.rewind()
            .map_err(|_| Error::invalid_argument("shared output seek failed"))?;
        let mut sink = Sink { file, count: 0 };
        let (mut meta, data) =
            session.request_with_sink(request, if rgba { Some(&mut sink) } else { None })?;
        sink.write_all(&data)
            .map_err(|_| Error::invalid_argument("shared output write failed"))?;
        // File writes are complete before the acknowledgement. The host must
        // wait for GPU completion before sending the next mutating request.
        meta.pop();
        meta.push_str(&format!(
            ",\"shared_bytes\":{},\"shared_kind\":\"{}\",\"pipe_payload_bytes\":0}}",
            sink.count,
            if rgba { "rgba" } else { "packet" }
        ));
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{number, open, request};
    use std::io::Read;

    #[test]
    fn shared_bands_match_collected_pixels_and_use_only_band_storage() {
        let folder = std::env::temp_dir().join(format!("pagura-shared-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let paths = [folder.join("frame"), folder.join("packet")];
        for path in &paths {
            File::create_new(path)
                .unwrap()
                .set_len(MAX_FRAME as u64)
                .unwrap();
        }
        let mut output = Output::open(&paths[0], &paths[1]).unwrap();
        let mut reference = Session::new();
        let mut streamed = Session::new();
        let content = [b"0.2 0.5 0.8 rg 1.25 2.75 12.3 20.1 re f".as_slice()];
        open(&mut reference, &content);
        open(&mut streamed, &content);
        for region in [[0, 0, 0, 0], [1, 2, 20, 30]] {
            let args = [7, 0, 150, 7, region[0], region[1], region[2], region[3]];
            let (_, expected) = request(&mut reference, args);
            let wire: Vec<_> = args.into_iter().flat_map(u32::to_le_bytes).collect();
            for _ in 0..2 {
                let meta = output.request(&mut streamed, &wire).unwrap();
                assert_eq!(number(&meta, "shared_bytes") as usize, expected.len());
                let mut actual = vec![0; expected.len()];
                File::open(&paths[0])
                    .unwrap()
                    .read_exact(&mut actual)
                    .unwrap();
                assert_eq!(actual, expected);
                assert!(streamed.output.is_empty());
                assert!(streamed.output.capacity() <= 20 * 4 * 7 * 3);
            }
        }
        let mut file = File::create(folder.join("bounded")).unwrap();
        let mut sink = Sink {
            file: &mut file,
            count: MAX_FRAME - 1,
        };
        assert!(sink.write_all(&[0, 0]).is_err());
        drop(output);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
