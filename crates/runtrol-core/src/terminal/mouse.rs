//! One mouse for every CLI: a viewer's clicks and wheel, turned into keys on the screen the CLI drew.
//!
//! The decision (`docs/terminalSurface.md`, operator-fixed 2026-08-25): the surface does not depend on a
//! CLI's own mouse support. Claude Code can report mouse in one of its renderers, Codex and Grok report
//! none, and a person must not learn three feels. So the host switches mouse reporting on toward the
//! *viewer* only ([`VIEWER_MOUSE_ON`], never sent to the CLI), receives the viewer's SGR mouse reports on
//! the input path, and translates each one here into the keys that reach the same place. The CLI sees
//! keys, as it does from a keyboard.
//!
//! Translation is geometry on the screen model, not reading: a click on a row above the cursor is that
//! many Up keys, a wheel notch is a few arrow keys. Nothing here knows what the rows say.
//!
//! The same path also drops the answers a viewer's own terminal sends to the CLI's questions (device
//! attributes, cursor reports and the like): the host answered already, and a second answer from each
//! attached viewer would reach the CLI as stray input.

use vt100::Screen;

/// Sent to a viewer with its snapshot: report clicks (1000) and wheel in SGR form (1006). Drag reporting
/// (1002) is left off, so the viewer's own selection stays a plain drag where its terminal allows.
pub const VIEWER_MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1006h";

/// How many arrow keys one wheel notch becomes. The value the shipped CLIs' own scroll settings use.
const WHEEL_STEP: usize = 3;

/// A report may be split across two input writes. The tail is kept so a sequence that straddles the
/// boundary is still recognised as one.
const CARRY_BYTES: usize = 24;

/// The scan state between input writes.
#[derive(Debug, Default)]
pub struct InputCarry {
    tail: Vec<u8>,
}

impl InputCarry {
    /// The bytes to forward to the CLI for this input: keys as typed, mouse reports translated, terminal
    /// answers dropped.
    pub fn translate(&mut self, input: &[u8], screen: &Screen) -> Vec<u8> {
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
                Scan::Mouse(report, end) => {
                    translate_mouse(report, screen, &mut out);
                    at = end;
                }
                Scan::Answer(end) => at = end,
                Scan::Incomplete => {
                    // Keep the unfinished sequence for the next write, bounded so a stray ESC never
                    // grows the carry without limit.
                    let rest = window.get(at..).unwrap_or(&[]);
                    if rest.len() <= CARRY_BYTES {
                        self.tail = rest.to_vec();
                    } else {
                        out.extend_from_slice(rest);
                    }
                    return out;
                }
                Scan::Plain => {
                    out.push(byte);
                    at += 1;
                }
            }
        }
        out
    }
}

/// One SGR mouse report: `ESC [ < button ; col ; row M|m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseReport {
    button: u16,
    /// One-based column as the viewer reports it.
    col: u16,
    /// One-based row as the viewer reports it.
    row: u16,
    pressed: bool,
}

enum Scan {
    Mouse(MouseReport, usize),
    Answer(usize),
    Incomplete,
    Plain,
}

/// What begins at `start` (an ESC).
fn sequence_at(window: &[u8], start: usize) -> Scan {
    let Some(rest) = window.get(start + 1..) else {
        return Scan::Incomplete;
    };
    if rest.is_empty() {
        return Scan::Incomplete;
    }
    if let Some(after) = rest.strip_prefix(b"[<") {
        return match parse_mouse(after) {
            Some((report, used)) => Scan::Mouse(report, start + 3 + used),
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

/// `button ; col ; row M|m` and how many bytes it took.
fn parse_mouse(after: &[u8]) -> Option<(MouseReport, usize)> {
    let mut numbers = [0u16; 3];
    let mut at = 0usize;
    for (index, slot) in numbers.iter_mut().enumerate() {
        let digits = after
            .get(at..)?
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        *slot = decimal(after.get(at..at + digits)?)?;
        at += digits;
        if index < 2 {
            if after.get(at) != Some(&b';') {
                return None;
            }
            at += 1;
        }
    }
    let pressed = match after.get(at)? {
        b'M' => true,
        b'm' => false,
        _ => return None,
    };
    Some((
        MouseReport {
            button: numbers[0],
            col: numbers[1],
            row: numbers[2],
            pressed,
        },
        at + 1,
    ))
}

/// A run of ASCII digits as a number, or nothing when it does not fit.
fn decimal(digits: &[u8]) -> Option<u16> {
    digits.iter().try_fold(0u16, |value, byte| {
        value
            .checked_mul(10)?
            .checked_add(u16::from(byte.wrapping_sub(b'0')))
    })
}

fn arrow(up: bool, screen: &Screen) -> &'static [u8] {
    match (up, screen.application_cursor()) {
        (true, false) => b"\x1b[A",
        (true, true) => b"\x1bOA",
        (false, false) => b"\x1b[B",
        (false, true) => b"\x1bOB",
    }
}

/// The keys one mouse report becomes on this screen.
fn translate_mouse(report: MouseReport, screen: &Screen, out: &mut Vec<u8>) {
    // Bits 0..2 are the button, 64 marks wheel; modifier and motion bits are ignored on purpose.
    let wheel = report.button & 64 != 0;
    let button = report.button & 0b11;
    if wheel {
        let up = button == 0;
        for _ in 0..WHEEL_STEP {
            out.extend_from_slice(arrow(up, screen));
        }
        return;
    }
    if !report.pressed || button != 0 {
        return;
    }
    // A click on a row: move the cursor's row there with arrows. On a list prompt that selects the row;
    // on a plain input it does nothing harmful. Column is deliberately ignored: no shipped CLI moves
    // horizontally by mouse, and typing at a column is not a thing a person expects from a click.
    //
    // The row is clamped to the screen first: a report names any number up to 65535, and without the clamp
    // one click became 196 KB of arrows into the CLI (measured 2026-08-25). The most a click can send is the
    // screen's height in arrows.
    let (rows, _) = screen.size();
    let (cursor_row, _) = screen.cursor_position();
    let target = report.row.saturating_sub(1).min(rows.saturating_sub(1));
    let (steps, up) = if target < cursor_row {
        (usize::from(cursor_row - target), true)
    } else {
        (usize::from(target - cursor_row), false)
    };
    for _ in 0..steps {
        out.extend_from_slice(arrow(up, screen));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with_cursor_at(row: u16) -> vt100::Parser {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(format!("\x1b[{};1H", row + 1).as_bytes());
        parser
    }

    #[test]
    fn keys_pass_through_untouched() {
        let parser = screen_with_cursor_at(0);
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"hello\r\x1b[A\x1b[3~", parser.screen()),
            b"hello\r\x1b[A\x1b[3~".to_vec()
        );
    }

    #[test]
    fn a_wheel_notch_is_three_arrows() {
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<64;10;3M", parser.screen()),
            b"\x1b[A\x1b[A\x1b[A".to_vec()
        );
        assert_eq!(
            carry.translate(b"\x1b[<65;10;3M", parser.screen()),
            b"\x1b[B\x1b[B\x1b[B".to_vec()
        );
    }

    #[test]
    fn a_click_moves_the_cursor_row_by_arrows_and_a_release_does_nothing() {
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<0;4;9M", parser.screen()),
            b"\x1b[B\x1b[B\x1b[B".to_vec()
        );
        assert_eq!(
            carry.translate(b"\x1b[<0;4;4M", parser.screen()),
            b"\x1b[A\x1b[A".to_vec()
        );
        assert!(carry.translate(b"\x1b[<0;4;4m", parser.screen()).is_empty());
    }

    #[test]
    fn a_click_past_the_screen_is_clamped_to_its_last_row() {
        let parser = screen_with_cursor_at(0);
        let mut carry = InputCarry::default();
        let out = carry.translate(b"\x1b[<0;1;65535M", parser.screen());
        assert_eq!(
            out.len(),
            23 * 3,
            "at most the screen's height in arrows: {}",
            out.len()
        );
    }

    #[test]
    fn a_report_split_across_writes_is_one_report() {
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert!(carry.translate(b"\x1b[<64;1", parser.screen()).is_empty());
        assert_eq!(
            carry.translate(b"0;3M", parser.screen()),
            b"\x1b[A\x1b[A\x1b[A".to_vec()
        );
    }

    #[test]
    fn a_viewers_own_terminal_answers_are_dropped() {
        let parser = screen_with_cursor_at(0);
        let mut carry = InputCarry::default();
        let input = b"\x1b[?62;22c\x1b[>41;378;0c\x1b[5;10R\x1b[?2026;2$y\x1bP>|xterm(378)\x1b\\x";
        assert_eq!(carry.translate(input, parser.screen()), b"x".to_vec());
    }

    #[test]
    fn application_cursor_mode_changes_the_arrow_spelling() {
        let mut parser = screen_with_cursor_at(2);
        parser.process(b"\x1b[?1h");
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<0;1;1M", parser.screen()),
            b"\x1bOA\x1bOA".to_vec()
        );
    }
}
