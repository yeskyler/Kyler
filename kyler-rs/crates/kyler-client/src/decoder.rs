use crate::error::{Error, Result};

#[derive(Debug)]
pub(crate) struct Frame {
    pub id: Option<String>,
    pub name: String,
    pub data: String,
}
/// Bounded line/frame parser. Byte buffering tolerates split UTF-8 and CR/LF boundaries.
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: String,
    name: String,
    id: Option<String>,
    size: usize,
    limit: usize,
    skip_lf: bool,
    first_line: bool,
}
impl Decoder {
    pub fn new(limit: usize) -> Self {
        Self {
            line: Vec::new(),
            data: String::new(),
            name: String::new(),
            id: None,
            size: 0,
            limit,
            skip_lf: false,
            first_line: true,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.size += 1;
            if self.size > self.limit {
                return Err(Error::protocol(
                    "SSE event exceeds the configured byte limit.",
                ));
            }
            if byte == b'\r' || byte == b'\n' {
                self.line(&mut frames)?;
                self.skip_lf = byte == b'\r';
            } else {
                self.line.push(byte);
            }
        }
        Ok(frames)
    }
    fn line(&mut self, frames: &mut Vec<Frame>) -> Result<()> {
        let bytes = std::mem::take(&mut self.line);
        let mut line = std::str::from_utf8(&bytes)
            .map_err(|_| Error::protocol("Invalid UTF-8 in SSE response."))?;
        if self.first_line {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            self.first_line = false;
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                frames.push(Frame {
                    id: self.id.take(),
                    name: if self.name.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut self.name)
                    },
                    data: std::mem::take(&mut self.data),
                });
            }
            self.data.clear();
            self.id = None;
            self.name.clear();
            self.size = 0;
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => self.name = value.into(),
            "id" if !value.contains('\0') => self.id = Some(value.into()),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Decoder;

    #[test]
    fn decodes_arbitrary_utf8_crlf_chunks_and_multiple_data_lines() {
        let source = "\u{feff}: heartbeat\r\n\r\nid: 42\r\nevent: text_delta\r\ndata: \u{4f60}\u{597d}\r\ndata: world\r\n\r\n";
        for chunk_size in 1..=source.len() {
            let mut decoder = Decoder::new(4096);
            let mut frames = Vec::new();
            for chunk in source.as_bytes().chunks(chunk_size) {
                frames.extend(decoder.push(chunk).unwrap());
            }
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].id.as_deref(), Some("42"));
            assert_eq!(frames[0].name, "text_delta");
            assert_eq!(frames[0].data, "\u{4f60}\u{597d}\nworld");
        }
    }
    #[test]
    fn rejects_unbounded_frames_and_invalid_utf8() {
        assert!(Decoder::new(8).push(b"data: unbounded").is_err());
        assert!(Decoder::new(100).push(b"data: \xff\n\n").is_err());
        let mut decoder = Decoder::new(16);
        for _ in 0..100 {
            assert!(decoder.push(b": ping\n\n").unwrap().is_empty());
        }
        assert!(decoder.push(b"data: partial").unwrap().is_empty());
    }
}
