//! What the user types into an attached terminal: the detach key, found in
//! its plain form and as the keyboard protocols that full-screen programs turn
//! on report it, and the rest as text for `input`, which carries UTF-8.

const ESC: u8 = 0x1b;

/// The key that detaches: a control character the user may choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetachKey {
    /// What the terminal sends for the key without a keyboard protocol.
    byte: u8,
    /// The key's character, as the kitty keyboard protocol and xterm's
    /// modifyOtherKeys report it together with Ctrl.
    key: u32,
}

impl Default for DetachKey {
    fn default() -> Self {
        Self {
            byte: 0x1c,
            key: u32::from(b'\\'),
        }
    }
}

impl std::str::FromStr for DetachKey {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("`{value}` is not ctrl-<key> with a letter or one of \\ ] ^ _");
        let lower = value.to_ascii_lowercase();
        let key = lower.strip_prefix("ctrl-").ok_or_else(invalid)?;
        let [key] = key.as_bytes() else {
            return Err(invalid());
        };
        let byte = match key {
            b'a'..=b'z' => key - b'a' + 1,
            b'\\' => 0x1c,
            b']' => 0x1d,
            b'^' => 0x1e,
            b'_' => 0x1f,
            _ => return Err(invalid()),
        };
        Ok(Self {
            byte,
            key: u32::from(*key),
        })
    }
}

impl DetachKey {
    /// Where the first press of the key starts in `data`. A key's report
    /// arrives within one read from the terminal, so a sequence split between
    /// two reads is not looked for; holding back an escape would delay every
    /// plain Escape until the next key.
    pub fn find(&self, data: &[u8]) -> Option<usize> {
        let mut index = 0;
        while index < data.len() {
            if data[index] == self.byte {
                return Some(index);
            }
            if data[index] == ESC && data.get(index + 1) == Some(&b'[') {
                let params_start = index + 2;
                let Some(length) = data[params_start..]
                    .iter()
                    .position(|b| (0x40..=0x7e).contains(b))
                else {
                    // An unfinished sequence; the plain key may still follow.
                    return data[params_start..]
                        .iter()
                        .position(|&b| b == self.byte)
                        .map(|offset| params_start + offset);
                };
                let end = params_start + length;
                if self.is_reported(&data[params_start..end], data[end]) {
                    return Some(index);
                }
                index = end + 1;
                continue;
            }
            index += 1;
        }
        None
    }

    /// `CSI <key>[:…] ; <mods>[:<event>] u` (kitty) or `CSI 27 ; <mods> ; <key> ~`
    /// (modifyOtherKeys) for a press of the key with Ctrl and no other modifier.
    fn is_reported(&self, params: &[u8], last: u8) -> bool {
        let Ok(params) = std::str::from_utf8(params) else {
            return false;
        };
        let mut fields = params.split(';');
        let (key, modifiers) = match last {
            b'u' => (
                fields.next().and_then(|f| f.split(':').next()),
                fields.next().unwrap_or("1"),
            ),
            b'~' if fields.next() == Some("27") => {
                let modifiers = fields.next().unwrap_or("1");
                (fields.next(), modifiers)
            }
            _ => return false,
        };
        let mut modifiers = modifiers.split(':');
        let mods: Option<u32> = modifiers.next().and_then(|m| m.parse().ok());
        let released = modifiers.next() == Some("3");
        key.and_then(|k| k.parse::<u32>().ok()) == Some(self.key)
            && mods.is_some_and(ctrl_only)
            && !released
    }
}

/// Ctrl alone among Shift, Alt, Ctrl, Super, Hyper and Meta; Caps Lock and
/// Num Lock do not count.
fn ctrl_only(mods: u32) -> bool {
    mods.checked_sub(1)
        .is_some_and(|bits| bits & !(64 | 128) == 4)
}

/// Terminal input as text: a UTF-8 sequence split between two reads waits for
/// its rest, and bytes that are not UTF-8 become U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8Input {
    pending: Vec<u8>,
}

impl Utf8Input {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut text = String::new();
        let mut rest = data.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    text.push_str(valid);
                    return text;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    text.push_str(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(length) => {
                            text.push(char::REPLACEMENT_CHARACTER);
                            rest = &after[length..];
                        }
                        None => {
                            self.pending = after.to_vec();
                            return text;
                        }
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
    fn detach_keys_are_ctrl_with_a_letter_or_one_of_four_signs() {
        assert_eq!("ctrl-\\".parse::<DetachKey>(), Ok(DetachKey::default()));
        assert_eq!("ctrl-]".parse::<DetachKey>().map(|k| k.byte), Ok(0x1d));
        assert_eq!("CTRL-A".parse::<DetachKey>().map(|k| k.byte), Ok(0x01));
        for invalid in ["alt-x", "ctrl-", "ctrl-ab", "ctrl-1", "ctrl-[", "x"] {
            assert!(invalid.parse::<DetachKey>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_key_is_found_plain_and_as_keyboard_protocols_report_it() {
        let key = DetachKey::default();
        assert_eq!(key.find(b"ls\x1c rest"), Some(2));
        assert_eq!(key.find(b"x\x1b[92;5u"), Some(1));
        assert_eq!(key.find(b"\x1b[92;69:1u"), Some(0), "with Caps Lock");
        assert_eq!(key.find(b"\x1b[27;5;92~"), Some(0));
        assert_eq!(key.find(b"\x1b[92;5:3u"), None, "release");
        assert_eq!(key.find(b"\x1b[92;7u"), None, "Ctrl+Alt");
        assert_eq!(key.find(b"\x1b[93;5u"), None, "another key");
        assert_eq!(key.find(b"\x1b[1;5A\x1c"), Some(6));
        assert_eq!(key.find(b"plain \x1d text"), None);
        let other: DetachKey = "ctrl-]".parse().expect("key");
        assert_eq!(other.find(b"\x1c\x1d"), Some(1));
        assert_eq!(other.find(b"\x1b[93;5u"), Some(0));
    }

    #[test]
    fn input_is_utf8_also_when_a_character_is_split_between_reads() {
        let mut input = Utf8Input::default();
        let bytes = "Grüße 🙂".as_bytes();
        let mut text = String::new();
        for byte in bytes {
            text.push_str(&input.push(std::slice::from_ref(byte)));
        }
        assert_eq!(text, "Grüße 🙂");
        assert_eq!(input.push(b"a\xffb"), "a\u{fffd}b");
    }
}
