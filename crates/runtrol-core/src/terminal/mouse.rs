//! One mouse for every CLI, for the viewer that has none of its own: a touch screen's taps and swipes,
//! turned into keys on the screen the CLI drew.
//!
//! The decision (`docs/terminalSurface.md`, operator-fixed 2026-08-25, narrowed 2026-08-29): the surface
//! does not depend on a CLI's own mouse support. Claude Code can report mouse in one of its renderers,
//! Codex and Grok report none, and a person must not learn three feels. So for a touch viewer the host
//! switches mouse reporting on toward the *viewer* only ([`VIEWER_MOUSE_ON`], never sent to the CLI),
//! receives the viewer's SGR mouse reports on the input path, and translates each one here into the keys
//! that reach the same place. The CLI sees keys, as it does from a keyboard.
//!
//! A real terminal emulator ([`ViewerKind::Terminal`]) gets none of this. **The mouse is a touch-screen
//! concept in this product and nothing else** (operator, 2026-08-29, fixed in the ledger): on a computer the
//! terminal keeps its own mouse, selecting on drag and scrolling on wheel, and no click ever reaches the
//! CLI. That takes two things. The host never switches reporting on toward a terminal, and the CLI's own
//! attempts to switch it on (`ESC [ ? 1000 h` and its relatives, which one renderer of Claude Code sends)
//! are taken out of the stream before it reaches any viewer or the screen model ([`OutputCarry`]), so the
//! viewer's terminal never enters mouse mode and a later snapshot cannot replay it. Any mouse report a
//! terminal viewer sends anyway is dropped here instead of forwarded. Before this, every click became arrow
//! keys, which in Claude Code's prompt recalled earlier input, and drag selection was gone.
//!
//! Translation is geometry on the screen model, not reading: a click on a row above the cursor is that
//! many Up keys, a wheel notch is a few arrow keys. Nothing here knows what the rows say.
//!
//! The same path also drops, for every viewer, the answers a viewer's own terminal sends to the CLI's
//! questions (device attributes, cursor reports and the like): the host answered already, and a second
//! answer from each attached viewer would reach the CLI as stray input.

use vt100::Screen;

use super::ViewerKind;

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
    /// The bytes to forward to the CLI for this input: keys as typed, terminal answers dropped, and mouse
    /// reports translated for a touch viewer or forwarded untouched for a terminal.
    pub fn translate(&mut self, input: &[u8], screen: &Screen, viewer: ViewerKind) -> Vec<u8> {
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
                    match viewer {
                        ViewerKind::Touch => translate_mouse(report, screen, &mut out),
                        // A terminal viewer has no mouse toward the CLI at all: the CLI's own request to
                        // report was stripped from the stream, so a report that arrives anyway is the
                        // viewer's terminal acting on its own, and it goes nowhere.
                        ViewerKind::Terminal => {}
                    }
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

/// DEC private modes that switch a terminal's mouse reporting on or off, in every flavour a CLI sends:
/// X10 (9), normal (1000), highlight (1001), button motion (1002), any motion (1003), and the UTF-8, SGR,
/// urxvt and pixel encodings (1005, 1006, 1015, 1016).
const MOUSE_MODES: &[u32] = &[9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016];

/// The CLI's output with every mouse-mode switch taken out.
///
/// Applied before the screen model and before any viewer sees the bytes, so neither a live viewer nor a
/// later snapshot (the model replays the modes it holds) can put a terminal into mouse mode. Other private
/// modes in the same sequence (`ESC [ ? 1049 ; 1000 h`) are kept; only the mouse parameters leave. A
/// sequence split across two writes is carried to the next one, bounded.
#[derive(Debug, Default)]
pub struct OutputCarry {
    tail: Vec<u8>,
}

impl OutputCarry {
    /// `chunk` with the mouse-mode switches removed, plus whatever the previous chunk left unfinished.
    pub fn strip(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(window.len());
        let mut at = 0usize;
        while at < window.len() {
            let Some(&byte) = window.get(at) else { break };
            if byte != 0x1b {
                out.push(byte);
                at += 1;
                continue;
            }
            match private_mode_at(&window, at) {
                ModeScan::Complete {
                    end,
                    params,
                    action,
                } => {
                    let kept: Vec<u32> = params
                        .iter()
                        .copied()
                        .filter(|mode| !MOUSE_MODES.contains(mode))
                        .collect();
                    if kept.len() == params.len() {
                        out.extend_from_slice(window.get(at..end).unwrap_or(&[]));
                    } else if !kept.is_empty() {
                        out.extend_from_slice(b"\x1b[?");
                        for (index, mode) in kept.iter().enumerate() {
                            if index > 0 {
                                out.push(b';');
                            }
                            out.extend_from_slice(mode.to_string().as_bytes());
                        }
                        out.push(action);
                    }
                    at = end;
                }
                ModeScan::Incomplete => {
                    let rest = window.get(at..).unwrap_or(&[]);
                    if rest.len() <= CARRY_BYTES {
                        self.tail = rest.to_vec();
                    } else {
                        out.extend_from_slice(rest);
                    }
                    return out;
                }
                ModeScan::Other => {
                    out.push(byte);
                    at += 1;
                }
            }
        }
        out
    }
}

enum ModeScan {
    /// `ESC [ ? p1 ; p2 ... h|l`, ending at `end` (exclusive).
    Complete {
        end: usize,
        params: Vec<u32>,
        action: u8,
    },
    Incomplete,
    Other,
}

/// What begins at `start` (an ESC), if it is a DEC private mode set or reset.
fn private_mode_at(window: &[u8], start: usize) -> ModeScan {
    let Some(rest) = window.get(start + 1..) else {
        return ModeScan::Incomplete;
    };
    if rest.is_empty() {
        return ModeScan::Incomplete;
    }
    let Some(after) = rest.strip_prefix(b"[?") else {
        // `ESC [` alone may still become `ESC [ ?` on the next write.
        return if rest == b"[" {
            ModeScan::Incomplete
        } else {
            ModeScan::Other
        };
    };
    let body = after
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b';')
        .count();
    match after.get(body) {
        Some(&action @ (b'h' | b'l')) if body > 0 => {
            let mut params = Vec::new();
            for piece in after.get(..body).unwrap_or(&[]).split(|b| *b == b';') {
                match decimal(piece) {
                    Some(value) => params.push(u32::from(value)),
                    // A parameter this does not read as a number is not one it may rewrite.
                    None => return ModeScan::Other,
                }
            }
            ModeScan::Complete {
                end: start + 3 + body + 1,
                params,
                action,
            }
        }
        None if body < 16 => ModeScan::Incomplete,
        Some(_) | None => ModeScan::Other,
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
            carry.translate(b"hello\r\x1b[A\x1b[3~", parser.screen(), ViewerKind::Touch),
            b"hello\r\x1b[A\x1b[3~".to_vec()
        );
    }

    #[test]
    fn a_wheel_notch_is_three_arrows() {
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<64;10;3M", parser.screen(), ViewerKind::Touch),
            b"\x1b[A\x1b[A\x1b[A".to_vec()
        );
        assert_eq!(
            carry.translate(b"\x1b[<65;10;3M", parser.screen(), ViewerKind::Touch),
            b"\x1b[B\x1b[B\x1b[B".to_vec()
        );
    }

    #[test]
    fn a_click_moves_the_cursor_row_by_arrows_and_a_release_does_nothing() {
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<0;4;9M", parser.screen(), ViewerKind::Touch),
            b"\x1b[B\x1b[B\x1b[B".to_vec()
        );
        assert_eq!(
            carry.translate(b"\x1b[<0;4;4M", parser.screen(), ViewerKind::Touch),
            b"\x1b[A\x1b[A".to_vec()
        );
        assert!(
            carry
                .translate(b"\x1b[<0;4;4m", parser.screen(), ViewerKind::Touch)
                .is_empty()
        );
    }

    #[test]
    fn a_click_past_the_screen_is_clamped_to_its_last_row() {
        let parser = screen_with_cursor_at(0);
        let mut carry = InputCarry::default();
        let out = carry.translate(b"\x1b[<0;1;65535M", parser.screen(), ViewerKind::Touch);
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
        assert!(
            carry
                .translate(b"\x1b[<64;1", parser.screen(), ViewerKind::Touch)
                .is_empty()
        );
        assert_eq!(
            carry.translate(b"0;3M", parser.screen(), ViewerKind::Touch),
            b"\x1b[A\x1b[A\x1b[A".to_vec()
        );
    }

    #[test]
    fn a_viewers_own_terminal_answers_are_dropped() {
        let parser = screen_with_cursor_at(0);
        let mut carry = InputCarry::default();
        let input = b"\x1b[?62;22c\x1b[>41;378;0c\x1b[5;10R\x1b[?2026;2$y\x1bP>|xterm(378)\x1b\\x";
        assert_eq!(
            carry.translate(input, parser.screen(), ViewerKind::Touch),
            b"x".to_vec()
        );
    }

    #[test]
    fn a_terminal_viewers_mouse_report_goes_nowhere() {
        // The mouse is a touch-screen concept: on a computer the terminal keeps its own mouse and no click
        // reaches the CLI. Forwarding the report turned a click into arrow keys in Claude Code's prompt,
        // which recalled earlier input (operator, 2026-08-29, three times).
        let parser = screen_with_cursor_at(5);
        let mut carry = InputCarry::default();
        assert!(
            carry
                .translate(b"\x1b[<0;4;9M", parser.screen(), ViewerKind::Terminal)
                .is_empty()
        );
        // Its own terminal answers are still dropped: this host already answered the CLI.
        assert!(
            carry
                .translate(b"\x1b[?62;22c", parser.screen(), ViewerKind::Terminal)
                .is_empty()
        );
    }

    #[test]
    fn application_cursor_mode_changes_the_arrow_spelling() {
        let mut parser = screen_with_cursor_at(2);
        parser.process(b"\x1b[?1h");
        let mut carry = InputCarry::default();
        assert_eq!(
            carry.translate(b"\x1b[<0;1;1M", parser.screen(), ViewerKind::Touch),
            b"\x1bOA\x1bOA".to_vec()
        );
    }

    #[test]
    fn the_clis_mouse_switches_leave_the_stream_and_everything_else_stays() {
        let mut carry = OutputCarry::default();
        assert_eq!(carry.strip(b"a\x1b[?1000h\x1b[?1006hb"), b"ab".to_vec());
        assert_eq!(carry.strip(b"\x1b[?1002l\x1b[?1003l"), Vec::<u8>::new());
        // Other private modes in the same sequence stay; only the mouse parameters leave.
        assert_eq!(
            carry.strip(b"\x1b[?1049;1000;25h"),
            b"\x1b[?1049;25h".to_vec()
        );
        assert_eq!(
            carry.strip(b"\x1b[?25l\x1b[2J\x1b[H"),
            b"\x1b[?25l\x1b[2J\x1b[H".to_vec()
        );
        assert_eq!(carry.strip(b"\x1b[?1h"), b"\x1b[?1h".to_vec());
    }

    #[test]
    fn a_mouse_switch_split_across_two_writes_is_still_taken_out() {
        let mut carry = OutputCarry::default();
        let mut out = carry.strip(b"x\x1b[?10");
        out.extend(carry.strip(b"00hy"));
        assert_eq!(out, b"xy".to_vec());
        let mut split_at_bracket = OutputCarry::default();
        let mut out = split_at_bracket.strip(b"\x1b");
        out.extend(split_at_bracket.strip(b"[?1006h!"));
        assert_eq!(out, b"!".to_vec());
    }

    #[test]
    fn a_stripped_stream_leaves_the_screen_model_with_no_mouse_mode_to_replay() {
        let mut carry = OutputCarry::default();
        let mut parser = vt100::Parser::new(5, 20, 0);
        parser.process(&carry.strip(b"\x1b[?1000h\x1b[?1006hhello"));
        let replay = parser.screen().contents_formatted();
        assert!(!replay.windows(6).any(|w| w == b"?1000h" || w == b"?1006h"));
    }
}
