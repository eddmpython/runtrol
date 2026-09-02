//! The input boundary: what a viewer typed reaches the CLI exactly as typed, with one exception.
//!
//! Keys, paste, IME text, a lone Escape, an interrupt, and a mouse report a viewer's terminal sends are
//! forwarded byte for byte, once, in order. Nothing is rewritten, translated, or held back
//! (`terminalTransportIntegrity`, input and geometry). A mouse report is the CLI's own input vocabulary: the
//! CLI switched reporting on, and that viewer chose to honour it. Whether a viewer honours it is the viewer's
//! own business at its own edge (Studio keeps its terminal's selection and wheel by taking the CLI's mouse
//! switches out in `mouseModeFilter.ts`); the host never switches reporting on toward any viewer and turns
//! no gesture into keys.
//!
//! The one exception: the answers a viewer's own terminal sends to the CLI's questions (device attributes,
//! cursor reports and the like) are dropped. The host answered already (`xterm`), and a second answer from
//! each attached viewer would reach the CLI as stray input. Only a sequence already in progress is carried
//! to the next write, bounded, so that an answer or a report split across two writes is still recognised as
//! one.

/// How much of an unfinished sequence is carried to the next write. A stray ESC never grows the carry past
/// this.
const CARRY_BYTES: usize = 24;

/// The scan state between input writes.
#[derive(Debug, Default)]
pub struct InputCarry {
    tail: Vec<u8>,
}

impl InputCarry {
    /// The bytes to forward to the CLI for this write: everything as typed, terminal answers dropped.
    ///
    /// This path never consults the hosted screen, so a keystroke never waits behind output rendering.
    pub fn forward(&mut self, input: &[u8]) -> Vec<u8> {
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(input);
        let mut out = Vec::with_capacity(window.len());
        let mut at = 0usize;
        while at < window.len() {
            let Some(&byte) = window.get(at) else { break };
            if byte != 0x1b {
                out.push(byte);
                at += 1;
                continue;
            }
            match sequence_at(&window, at) {
                Scan::Mouse(end) => {
                    out.extend_from_slice(window.get(at..end).unwrap_or(&[]));
                    at = end;
                }
                Scan::Answer(end) => at = end,
                Scan::Incomplete => {
                    let rest = window.get(at..).unwrap_or(&[]);
                    if rest.len() <= CARRY_BYTES {
                        self.tail = rest.to_vec();
                    } else {
                        out.extend_from_slice(rest);
                    }
                    return out;
                }
                // A lone ESC ending a write is the Escape key. Holding it for the next write would delay the
                // key and then deliver it glued to the next one, which a CLI reads as an Alt chord (measured
                // 2026-09-02 in the carry's own fixture). Only a sequence already in progress is carried.
                Scan::Escape | Scan::Plain => {
                    out.push(byte);
                    at += 1;
                }
            }
        }
        out
    }
}

enum Scan {
    /// One whole SGR mouse report (`ESC [ < button ; col ; row M|m`), and where it ends.
    Mouse(usize),
    /// One whole terminal answer, and where it ends.
    Answer(usize),
    /// A lone ESC with nothing after it in this write: the Escape key, not the start of a sequence.
    Escape,
    Incomplete,
    Plain,
}

/// What begins at `start` (an ESC).
fn sequence_at(window: &[u8], start: usize) -> Scan {
    let rest = window.get(start + 1..).unwrap_or(&[]);
    if rest.is_empty() {
        return Scan::Escape;
    }
    if let Some(after) = rest.strip_prefix(b"[<") {
        return match mouse_report_len(after) {
            Some(used) => Scan::Mouse(start + 3 + used),
            None if after.len() < 12 && after.iter().all(|b| b.is_ascii_digit() || *b == b';') => {
                Scan::Incomplete
            }
            None => Scan::Plain,
        };
    }
    // Answers a viewer's terminal sends on its own: CSI with a private or parameter body ending in `c`
    // (device attributes), `R` (cursor position), `$y` (mode report), `n` (status), `u` (keyboard flags);
    // DCS `P>|...ST` (version) and `P0+r`/`P1+r...ST` (capabilities); OSC 10/11 colour replies.
    if let Some(after) = rest.strip_prefix(b"[") {
        let body = after
            .iter()
            .take_while(|b| b.is_ascii_digit() || matches!(b, b';' | b'?' | b'>' | b'$'))
            .count();
        return match after.get(body) {
            Some(b'c' | b'R' | b'n' | b'u' | b'y') if body > 0 => {
                Scan::Answer(start + 2 + body + 1)
            }
            None if body < 16 => Scan::Incomplete,
            Some(_) | None => Scan::Plain,
        };
    }
    if rest.starts_with(b"P>|") || rest.starts_with(b"P0+r") || rest.starts_with(b"P1+r") {
        return match rest.windows(2).position(|pair| pair == b"\x1b\\") {
            Some(terminator) => Scan::Answer(start + 1 + terminator + 2),
            None if rest.len() < 64 => Scan::Incomplete,
            None => Scan::Plain,
        };
    }
    if rest.starts_with(b"]10;rgb:") || rest.starts_with(b"]11;rgb:") {
        return match rest.iter().position(|b| *b == 0x07) {
            Some(bell) => Scan::Answer(start + 1 + bell + 1),
            None => match rest.windows(2).position(|pair| pair == b"\x1b\\") {
                Some(terminator) => Scan::Answer(start + 1 + terminator + 2),
                None if rest.len() < 48 => Scan::Incomplete,
                None => Scan::Plain,
            },
        };
    }
    Scan::Plain
}

/// How many bytes `button ; col ; row M|m` takes, when `after` begins with a whole report.
fn mouse_report_len(after: &[u8]) -> Option<usize> {
    let mut at = 0usize;
    for index in 0..3 {
        let digits = after
            .get(at..)?
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 || digits > 5 {
            return None;
        }
        at += digits;
        if index < 2 {
            if after.get(at) != Some(&b';') {
                return None;
            }
            at += 1;
        }
    }
    matches!(after.get(at)?, b'M' | b'm').then_some(at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_paste_ime_text_and_a_lone_escape_reach_the_cli_exactly_as_typed() {
        let mut carry = InputCarry::default();
        let corpus = "안녕하세요 hello\r\npasted line one\nline two\r\x03".as_bytes();
        assert_eq!(carry.forward(corpus), corpus.to_vec());
        // A lone Escape is the key, delivered now; the next write is not glued to it.
        assert_eq!(carry.forward(b"\x1b"), b"\x1b".to_vec());
        assert_eq!(carry.forward(b"x"), b"x".to_vec());
        // Arrow keys and function keys are sequences too, and they pass whole.
        assert_eq!(
            carry.forward(b"\x1b[A\x1b[3~\x1bOP"),
            b"\x1b[A\x1b[3~\x1bOP".to_vec()
        );
    }

    #[test]
    fn a_viewers_own_terminal_answers_are_dropped_whole_or_split() {
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.forward(b"a\x1b[?62;22c\x1b[12;40R\x1b[?2026;2$y\x1b[0n\x1b[?0ub"),
            b"ab".to_vec()
        );
        assert_eq!(
            carry.forward(b"\x1bP>|xterm(378)\x1b\\\x1b]11;rgb:0000/0000/0000\x1b\\c"),
            b"c".to_vec()
        );
        let mut split = InputCarry::default();
        assert_eq!(split.forward(b"\x1b[?6"), Vec::<u8>::new());
        assert_eq!(split.forward(b"2;22cd"), b"d".to_vec());
    }

    #[test]
    fn a_terminal_viewers_mouse_report_is_forwarded_exactly_once() {
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.forward(b"\x1b[<0;10;5M\x1b[<0;10;5m"),
            b"\x1b[<0;10;5M\x1b[<0;10;5m".to_vec()
        );
        let mut split = InputCarry::default();
        assert_eq!(split.forward(b"\x1b[<64;3"), Vec::<u8>::new());
        assert_eq!(split.forward(b";7Mz"), b"\x1b[<64;3;7Mz".to_vec());
        assert_eq!(
            split.forward(b"q"),
            b"q".to_vec(),
            "nothing of the report is carried again"
        );
    }

    #[test]
    fn a_stray_escape_longer_than_the_carry_passes_through() {
        let mut carry = InputCarry::default();
        let long = [&b"\x1b["[..], &[b'9'; 40][..]].concat();
        assert_eq!(carry.forward(&long), long);
        assert!(carry.tail.is_empty());
    }
}
