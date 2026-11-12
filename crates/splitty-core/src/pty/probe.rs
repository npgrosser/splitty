//! The program status probe, `ESC ] 7501 ; ?` ended by BEL or `ESC \`
//! (Program Status Protocol, revision 0.4): the server answers it itself and
//! removes it from a session's output before anything records or forwards the
//! output, so neither a client's emulator nor the terminal under
//! `splitty attach` answers it a second time.
//!
//! [`ProbeMatcher`] runs first in the session's record path. It holds back a
//! beginning of the probe at the end of a chunk, at most the 9 bytes
//! `ESC ] 7501 ; ? ESC`, and passes it on with the next chunk, or at the end
//! of the output, when it turns out to be no probe. It does not know whether
//! it is inside another string, so a probe inside a DCS or APC string is
//! removed too.

use std::borrow::Cow;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The probe up to its terminator.
const PROBE: &[u8] = b"\x1b]7501;?";

/// A probe's terminator, which its answer repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminator {
    Bel,
    St,
}

impl Terminator {
    /// The answer to a probe ended by this terminator: the probe itself, which
    /// lists no states or kinds and so means all of them.
    pub fn answer(self) -> &'static [u8] {
        match self {
            Self::Bel => b"\x1b]7501;?\x07",
            Self::St => b"\x1b]7501;?\x1b\\",
        }
    }
}

/// How the bytes from an ESC on relate to the probe.
enum Match {
    /// A whole probe of this length.
    Probe(usize, Terminator),
    /// A beginning of the probe that the bytes end in.
    Beginning,
    None,
}

fn match_at(bytes: &[u8]) -> Match {
    let known = bytes.len().min(PROBE.len());
    if bytes[..known] != PROBE[..known] {
        return Match::None;
    }
    match bytes.get(PROBE.len()) {
        None => Match::Beginning,
        Some(&BEL) => Match::Probe(PROBE.len() + 1, Terminator::Bel),
        Some(&ESC) => match bytes.get(PROBE.len() + 1) {
            None => Match::Beginning,
            Some(b'\\') => Match::Probe(PROBE.len() + 2, Terminator::St),
            Some(_) => Match::None,
        },
        Some(_) => Match::None,
    }
}

/// Removes probes from a session's output; see the module documentation.
#[derive(Debug, Default)]
pub struct ProbeMatcher {
    /// A beginning of the probe at the end of the last chunk.
    held: Vec<u8>,
}

impl ProbeMatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes of `data`, after what was held back, without the probes in
    /// them, holding back a beginning of the probe at its end; and the
    /// terminators of the probes removed, in order.
    pub fn filter<'a>(&mut self, data: &'a [u8]) -> (Cow<'a, [u8]>, Vec<Terminator>) {
        let mut probes = Vec::new();
        if self.held.is_empty() && !data.contains(&ESC) {
            return (Cow::Borrowed(data), probes);
        }
        let joined;
        let bytes: &[u8] = if self.held.is_empty() {
            data
        } else {
            let mut held = std::mem::take(&mut self.held);
            held.extend_from_slice(data);
            joined = held;
            &joined
        };
        // Copied up to `kept`; the output is built only once a probe or a
        // beginning is found.
        let mut output: Option<Vec<u8>> = None;
        let mut kept = 0;
        let mut index = 0;
        while let Some(found) = bytes[index..].iter().position(|&b| b == ESC) {
            let at = index + found;
            match match_at(&bytes[at..]) {
                Match::Probe(length, terminator) => {
                    output
                        .get_or_insert_with(|| Vec::with_capacity(bytes.len()))
                        .extend_from_slice(&bytes[kept..at]);
                    probes.push(terminator);
                    index = at + length;
                    kept = index;
                }
                Match::Beginning => {
                    output
                        .get_or_insert_with(|| Vec::with_capacity(bytes.len()))
                        .extend_from_slice(&bytes[kept..at]);
                    self.held = bytes[at..].to_vec();
                    kept = bytes.len();
                    break;
                }
                Match::None => index = at + 1,
            }
        }
        let output = match output {
            Some(mut output) => {
                output.extend_from_slice(&bytes[kept..]);
                Cow::Owned(output)
            }
            None if bytes.len() == data.len() => Cow::Borrowed(data),
            None => Cow::Owned(bytes.to_vec()),
        };
        (output, probes)
    }

    /// The end of the output: the bytes held back, which were no probe.
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `data` in two chunks split at `split`, then the end: the output and
    /// the probes found.
    fn split(data: &[u8], split: usize) -> (Vec<u8>, Vec<Terminator>) {
        let mut matcher = ProbeMatcher::new();
        let (first, mut probes) = matcher.filter(&data[..split]);
        let mut output = first.into_owned();
        let (second, more) = matcher.filter(&data[split..]);
        output.extend_from_slice(&second);
        probes.extend(more);
        output.extend(matcher.finish());
        (output, probes)
    }

    fn every_split(data: &[u8], output: &[u8], probes: &[Terminator]) {
        for at in 0..=data.len() {
            assert_eq!(
                split(data, at),
                (output.to_vec(), probes.to_vec()),
                "{:?} split at {at}",
                String::from_utf8_lossy(data)
            );
        }
    }

    #[test]
    fn a_probe_with_either_terminator_is_removed_at_every_split() {
        every_split(b"a\x1b]7501;?\x07b", b"ab", &[Terminator::Bel]);
        every_split(b"a\x1b]7501;?\x1b\\b", b"ab", &[Terminator::St]);
        assert_eq!(Terminator::Bel.answer(), b"\x1b]7501;?\x07");
        assert_eq!(Terminator::St.answer(), b"\x1b]7501;?\x1b\\");
    }

    #[test]
    fn two_probes_in_one_chunk_are_both_removed() {
        every_split(
            b"\x1b]7501;?\x1b\\\x1b[c\x1b]7501;?\x07",
            b"\x1b[c",
            &[Terminator::St, Terminator::Bel],
        );
    }

    #[test]
    fn beginnings_that_are_no_probe_pass_unchanged() {
        for data in [
            &b"\x1b]7501;state=done\x07"[..],
            b"\x1b]7",
            b"\x1b\x1b]7501;?\x1b\x1b\\",
            b"\x1b]7501;?x\x07",
            b"\x1b]75011;?\x07",
            b"\x1b]7501;??\x07",
            b"\x1b[31mred\x1b[0m",
        ] {
            every_split(data, data, &[]);
        }
    }

    #[test]
    fn a_report_split_after_its_number_reaches_the_next_chunk_whole() {
        let mut matcher = ProbeMatcher::new();
        let (first, probes) = matcher.filter(b"x\x1b]7501;");
        assert_eq!((first.as_ref(), probes.len()), (&b"x"[..], 0), "held back");
        let (second, probes) = matcher.filter(b"state=done\x07");
        assert_eq!(second.as_ref(), b"\x1b]7501;state=done\x07");
        assert!(probes.is_empty());
    }

    #[test]
    fn a_beginning_is_held_back_at_most_until_the_end_of_the_output() {
        let mut matcher = ProbeMatcher::new();
        let (output, _) = matcher.filter(b"prompt\x1b]7501;?\x1b");
        assert_eq!(output.as_ref(), b"prompt");
        assert_eq!(matcher.finish(), b"\x1b]7501;?\x1b", "nine bytes at most");
        assert!(matcher.finish().is_empty());
    }

    #[test]
    fn output_without_a_probe_is_not_copied() {
        let mut matcher = ProbeMatcher::new();
        let data = b"plain \x1b[1mbold\x1b[0m";
        assert!(matches!(matcher.filter(data).0, Cow::Borrowed(_)));
        assert!(matches!(matcher.filter(b"no escape").0, Cow::Borrowed(_)));
    }
}
