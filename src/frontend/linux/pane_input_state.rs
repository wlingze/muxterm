//! GTK pane input-mode tracking.
//!
//! Core owns the authoritative terminal model and parser-generated replies.
//! GTK only needs a small mirror of the modes that affect pointer, scrolling,
//! paste, and the platform clipboard. Keeping this tracker separate avoids
//! copying Core's screen grid and scrollback model into the frontend.

use vte::ansi::{Handler, NamedPrivateMode, PrivateMode, Processor};

const MAX_OSC_BYTES: usize = 64 * 1024;

/// Terminal modes that change how GTK routes user input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PaneInputModes {
    pub alternate_screen: bool,
    pub bracketed_paste: bool,
    pub mouse_reporting: bool,
    pub mouse_clicks: bool,
    pub mouse_button_motion: bool,
    pub mouse_all_motion: bool,
}

/// Minimal frontend-side parser state for input routing.
///
/// This is intentionally not a terminal emulator. Core remains the owner of
/// the terminal grid, scrollback, attention signals, and parser replies.
#[derive(Default)]
pub(crate) struct PaneInputState {
    modes: PaneInputModes,
    clipboard_set: Option<String>,
    processor: Processor,
    osc: OscScanner,
}

impl PaneInputState {
    pub(crate) fn modes(&self) -> PaneInputModes {
        self.modes
    }

    /// Feed remote output only to the mode tracker and OSC 52 scanner.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let mut processor = std::mem::take(&mut self.processor);
        for &byte in bytes {
            self.osc.advance(byte, &mut self.clipboard_set);
            processor.advance(self, byte);
        }
        self.processor = processor;
    }

    pub(crate) fn take_clipboard_set(&mut self) -> Option<String> {
        self.clipboard_set.take()
    }

    fn sync_mouse_reporting(&mut self) {
        self.modes.mouse_reporting = self.modes.mouse_clicks
            || self.modes.mouse_button_motion
            || self.modes.mouse_all_motion;
    }

    fn set_private_mode(&mut self, mode: NamedPrivateMode) {
        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                self.modes.alternate_screen = true;
            }
            NamedPrivateMode::BracketedPaste => self.modes.bracketed_paste = true,
            NamedPrivateMode::ReportMouseClicks => {
                self.modes.mouse_clicks = true;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.modes.mouse_button_motion = true;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.modes.mouse_all_motion = true;
                self.sync_mouse_reporting();
            }
            _ => {}
        }
    }

    fn unset_private_mode(&mut self, mode: NamedPrivateMode) {
        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                self.modes.alternate_screen = false;
            }
            NamedPrivateMode::BracketedPaste => self.modes.bracketed_paste = false,
            NamedPrivateMode::ReportMouseClicks => {
                self.modes.mouse_clicks = false;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.modes.mouse_button_motion = false;
                self.sync_mouse_reporting();
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.modes.mouse_all_motion = false;
                self.sync_mouse_reporting();
            }
            _ => {}
        }
    }
}

impl Handler for PaneInputState {
    fn set_private_mode(&mut self, mode: PrivateMode) {
        if let PrivateMode::Named(mode) = mode {
            self.set_private_mode(mode);
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if let PrivateMode::Named(mode) = mode {
            self.unset_private_mode(mode);
        }
    }
}

#[derive(Default)]
struct OscScanner {
    escape_pending: bool,
    body: Option<Vec<u8>>,
    body_escape_pending: bool,
    discard_body: bool,
}

impl OscScanner {
    fn advance(&mut self, byte: u8, clipboard_set: &mut Option<String>) {
        if self.body.is_some() {
            self.advance_body(byte, clipboard_set);
            return;
        }

        if self.escape_pending {
            self.escape_pending = false;
            if byte == b']' {
                self.body = Some(Vec::new());
                self.body_escape_pending = false;
                self.discard_body = false;
            } else if byte == 0x1b {
                self.escape_pending = true;
            }
            return;
        }

        if byte == 0x1b {
            self.escape_pending = true;
        }
    }

    fn advance_body(&mut self, byte: u8, clipboard_set: &mut Option<String>) {
        if self.body_escape_pending {
            self.body_escape_pending = false;
            if byte == b'\\' {
                self.finish(clipboard_set);
                return;
            }
            self.push_body_byte(0x1b);
        }

        if byte == 0x07 {
            self.finish(clipboard_set);
        } else if byte == 0x1b {
            self.body_escape_pending = true;
        } else {
            self.push_body_byte(byte);
        }
    }

    fn push_body_byte(&mut self, byte: u8) {
        if self.discard_body {
            return;
        }
        let Some(body) = self.body.as_mut() else {
            return;
        };
        if body.len() >= MAX_OSC_BYTES {
            self.discard_body = true;
        } else {
            body.push(byte);
        }
    }

    fn finish(&mut self, clipboard_set: &mut Option<String>) {
        let body = self.body.take();
        self.body_escape_pending = false;
        let discard = std::mem::take(&mut self.discard_body);
        if discard {
            return;
        }
        let Some(body) = body else {
            return;
        };
        let mut fields = body.splitn(3, |byte| *byte == b';');
        if fields.next() != Some(b"52") {
            return;
        }
        let _selection = fields.next();
        let Some(payload) = fields.next() else {
            return;
        };
        if let Some(text) = decode_osc52_payload(payload) {
            *clipboard_set = Some(text);
        }
    }
}

fn decode_osc52_payload(payload: &[u8]) -> Option<String> {
    if payload.is_empty() || payload == b"?" {
        return None;
    }
    let bytes = decode_base64(payload)?;
    String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.is_empty())
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    let filtered: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        .collect();
    if filtered.is_empty() {
        return Some(Vec::new());
    }

    let mut output = Vec::with_capacity(filtered.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in filtered {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::{PaneInputModes, PaneInputState};

    #[test]
    fn tracks_input_modes_without_screen_state() {
        let mut state = PaneInputState::default();
        state.feed(b"\x1b[?1049h\x1b[?2004h\x1b[?1000h\x1b[?1003h");
        assert_eq!(
            state.modes(),
            PaneInputModes {
                alternate_screen: true,
                bracketed_paste: true,
                mouse_reporting: true,
                mouse_clicks: true,
                mouse_button_motion: false,
                mouse_all_motion: true,
            }
        );

        state.feed(b"\x1b[?1003l\x1b[?1000l\x1b[?2004l\x1b[?1049l");
        assert_eq!(state.modes(), PaneInputModes::default());

        state.feed(b"\x1b[?1000h");
        assert!(state.modes().mouse_reporting);
        assert!(state.modes().mouse_clicks);
        assert!(!state.modes().mouse_button_motion);
        state.feed(b"\x1b[?1000l\x1b[?1002h");
        assert!(!state.modes().mouse_clicks);
        assert!(state.modes().mouse_button_motion);
    }

    #[test]
    fn scans_chunked_osc52_and_ignores_queries() {
        let mut state = PaneInputState::default();
        state.feed(b"\x1b]52;c;TVVY");
        state.feed(b"VEVSTV9PU0M1Mg==\x07");
        assert_eq!(state.take_clipboard_set().as_deref(), Some("MUXTERM_OSC52"));
        assert!(state.take_clipboard_set().is_none());

        state.feed(b"\x1b]52;c;?\x07");
        assert!(state.take_clipboard_set().is_none());
    }

    #[test]
    fn supports_string_terminator_for_osc52() {
        let mut state = PaneInputState::default();
        state.feed(b"\x1b]52;c;YQ==\x1b");
        state.feed(b"\\");
        assert_eq!(state.take_clipboard_set().as_deref(), Some("a"));
    }
}
