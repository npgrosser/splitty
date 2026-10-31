//! Bounded incremental log sanitation, deliberately not a terminal screen emulator.
//! vte runs without its std feature: its OSC storage is a fixed zero-length buffer.

use vte::Perform;

#[derive(Default)]
pub(super) struct LogText {
    parser: vte::Parser<0>,
    utf8: [u8; 4],
    utf8_len: usize,
    after_cr: bool,
}

struct Text<'a> {
    bytes: &'a mut Vec<u8>,
    after_cr: &'a mut bool,
}

impl Perform for Text<'_> {
    fn print(&mut self, character: char) {
        if character.is_control() {
            return;
        }
        *self.after_cr = false;
        let mut encoded = [0; 4];
        self.bytes
            .extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => {
                self.bytes.push(b'\n');
                *self.after_cr = true;
            }
            b'\n' => {
                if !*self.after_cr {
                    self.bytes.push(b'\n');
                }
                *self.after_cr = false;
            }
            b'\t' => {
                self.bytes.push(b'\t');
                *self.after_cr = false;
            }
            _ => {}
        }
    }
}

impl LogText {
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        for &byte in data {
            self.byte(byte, &mut output);
        }
        output
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let mut output = Vec::new();
        if self.utf8_len != 0 {
            self.utf8_len = 0;
            self.emit(&[0xff], &mut output);
        }
        output
    }

    fn emit(&mut self, data: &[u8], output: &mut Vec<u8>) {
        self.parser.advance(
            &mut Text {
                bytes: output,
                after_cr: &mut self.after_cr,
            },
            data,
        );
    }

    /// Normalize both raw and UTF-8-encoded C1 controls to the 7-bit sequences
    /// understood by vte, without confusing Unicode continuation bytes for C1.
    fn c1(&mut self, byte: u8, output: &mut Vec<u8>) {
        let introducer = match byte {
            0x90 => b'P',
            0x98 => b'X',
            0x9b => b'[',
            0x9c => b'\\',
            0x9d => b']',
            0x9e => b'^',
            0x9f => b'_',
            _ => return,
        };
        self.emit(&[0x1b, introducer], output);
    }

    fn byte(&mut self, byte: u8, output: &mut Vec<u8>) {
        if self.utf8_len == 0 {
            if byte < 0x80 {
                self.emit(&[byte], output);
                return;
            }
            if (0x80..=0x9f).contains(&byte) {
                self.c1(byte, output);
                return;
            }
        }
        self.utf8[self.utf8_len] = byte;
        self.utf8_len += 1;
        match std::str::from_utf8(&self.utf8[..self.utf8_len]) {
            Ok(value) => {
                let c1 = value
                    .chars()
                    .next()
                    .filter(|c| ('\u{80}'..='\u{9f}').contains(c));
                let bytes = self.utf8;
                let len = self.utf8_len;
                self.utf8_len = 0;
                if let Some(character) = c1 {
                    self.c1(
                        u8::try_from(u32::from(character)).expect("C1 fits in one byte"),
                        output,
                    );
                } else {
                    self.emit(&bytes[..len], output);
                }
            }
            Err(error) => {
                if let Some(invalid) = error.error_len() {
                    let bytes = self.utf8;
                    let len = self.utf8_len;
                    self.utf8_len = 0;
                    self.emit(&[0xff], output);
                    for &remaining in &bytes[invalid..len] {
                        self.byte(remaining, output);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chunk_boundary_has_the_same_text_and_no_control_payloads() {
        let bytes = b"start\x1b[31mred\x1b[0m\r\n\x1b]52;c;SECRET\x07\x1bPqSECRET\x1b\\\x1b_SECRET\x1b\\\x1b^SECRET\x1b\\\xc3\xb6\xf0\x9f\x99\x82\tend";
        for size in 1..=bytes.len() {
            let mut parser = LogText::default();
            let mut output = Vec::new();
            for chunk in bytes.chunks(size) {
                output.extend(parser.push(chunk));
            }
            output.extend(parser.finish());
            assert_eq!(
                String::from_utf8(output).expect("valid utf8"),
                "startred\nö🙂\tend",
                "chunk size {size}"
            );
        }
    }

    #[test]
    fn c1_controls_do_not_leak_payload_or_corrupt_utf8() {
        let mut parser = LogText::default();
        let bytes = b"\x9d52;c;SECRET\x9c\xc2\x9d52;c;SECRET\xc2\x9c\x9b31mOK\xc3\x9d";
        let output: Vec<_> = bytes
            .iter()
            .flat_map(|byte| parser.push(&[*byte]))
            .collect();
        assert_eq!(String::from_utf8(output).expect("utf8"), "OKÝ");
    }

    #[test]
    fn incomplete_and_invalid_unicode_is_bounded_and_replaced_on_finish() {
        let mut parser = LogText::default();
        assert!(parser.push(&[0xf0, 0x9f]).is_empty());
        assert_eq!(String::from_utf8(parser.finish()).expect("utf8"), "�");
        assert_eq!(
            String::from_utf8(parser.push(b"\xffa")).expect("utf8"),
            "�a"
        );
        assert!(std::mem::size_of::<LogText>() < 2048);
    }

    #[test]
    fn unterminated_huge_strings_keep_no_payload_between_chunks() {
        for introducer in *b"]P_^" {
            let mut parser = LogText::default();
            parser.push(&[0x1b, introducer]);
            for _ in 0..2048 {
                assert!(parser.push(&[b'x'; 1024]).is_empty());
            }
            assert_eq!(parser.push(b"\x1b\\after"), b"after");
        }
    }
}
