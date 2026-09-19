//! 仅保护「默认前景 + 应用显式背景」的可读性，不改变应用色板或 Core 数据。
//! 只跟踪 SGR/保存恢复属性，不保存屏幕；在完整控制序列边界插入前景修正，
//! 避免截断跨分包 UTF-8、OSC 和 CSI。显式前景色与隐藏文字保持原样。

use vte::ansi::{Attr, Color, Handler, NamedColor, Processor};

use crate::frontend::linux::theme::{indexed_color, Rgb, Theme};

#[derive(Clone, Copy)]
struct Style {
    default_fg: bool,
    bg: Color,
    correction: Option<Rgb>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            default_fg: true,
            bg: Color::Named(NamedColor::Background),
            correction: None,
        }
    }
}

pub(super) struct ContrastGuard {
    processor: Processor,
    style: Style,
    saved: Style,
    changed: bool,
    theme: Theme,
}

impl ContrastGuard {
    pub(super) fn new(theme: &Theme) -> Self {
        Self {
            processor: Processor::default(),
            style: Style::default(),
            saved: Style::default(),
            changed: false,
            theme: theme.clone(),
        }
    }

    pub(super) fn set_theme(&mut self, theme: &Theme) {
        self.theme = theme.clone();
        // 等下一个完整属性序列才修正，不在可能尚未结束的 CSI/OSC 中插字节。
    }

    pub(super) fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut processor = std::mem::take(&mut self.processor);
        for &byte in data {
            out.push(byte);
            processor.advance(self, byte);
            if std::mem::take(&mut self.changed) {
                self.correct(&mut out);
            }
        }
        self.processor = processor;
        out
    }

    fn background(&self) -> Option<Rgb> {
        let index = match self.style.bg {
            Color::Named(NamedColor::Background) => return None,
            Color::Spec(rgb) => return Some(Rgb(rgb.r, rgb.g, rgb.b)),
            Color::Indexed(index) => index as usize,
            Color::Named(named) => named as usize,
        };
        if index < 16 {
            Some(self.theme.colors[index])
        } else if index < 256 {
            Some(indexed_color(index as u32))
        } else {
            None
        }
    }

    fn correct(&mut self, out: &mut Vec<u8>) {
        if !self.style.default_fg {
            return;
        }
        let correction = self.background().and_then(|bg| {
            let fg = luminance(self.theme.foreground);
            let bg = luminance(bg);
            let ratio = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
            if ratio >= 3.0 {
                None
            } else if (1.05 / (bg + 0.05)) > ((bg + 0.05) / 0.05) {
                Some(Rgb(255, 255, 255))
            } else {
                Some(Rgb(0, 0, 0))
            }
        });
        if correction == self.style.correction {
            return;
        }
        match correction {
            Some(Rgb(r, g, b)) => {
                out.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m").as_bytes())
            }
            None => out.extend_from_slice(b"\x1b[39m"),
        }
        self.style.correction = correction;
    }
}

fn luminance(Rgb(r, g, b): Rgb) -> f64 {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

impl Handler for ContrastGuard {
    fn terminal_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::Reset => self.style = Style::default(),
            Attr::Foreground(color) => {
                self.style.default_fg = color == Color::Named(NamedColor::Foreground);
                // 原始 SGR 已覆盖上次注入的前景。
                self.style.correction = None;
            }
            Attr::Background(color) => self.style.bg = color,
            _ => {}
        }
        self.changed = true;
    }

    fn save_cursor_position(&mut self) {
        self.saved = self.style;
    }
    fn restore_cursor_position(&mut self) {
        self.style = self.saved;
        self.changed = true;
    }
    fn reset_state(&mut self) {
        self.style = Style::default();
        self.saved = Style::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::linux::theme::fallback_theme;

    #[test]
    fn codex_black_composer_keeps_input_visible_and_restores_default() {
        let mut guard = ContrastGuard::new(&fallback_theme());
        assert_eq!(
            guard.feed(b"\x1b[48;2;20;20;20mINPUT\x1b[49mnormal"),
            b"\x1b[48;2;20;20;20m\x1b[38;2;255;255;255mINPUT\x1b[49m\x1b[39mnormal"
        );
        assert_eq!(
            guard.feed(b"\x1b[0;48;2;216;216;216mLIGHT"),
            b"\x1b[0;48;2;216;216;216mLIGHT"
        );
    }

    #[test]
    fn fragmented_sequences_are_identical_and_explicit_colors_are_preserved() {
        let bytes =
            "\x1b]0;title\x07\x1b[48:2::20:20:20m中文⠁\x1b[31mred\x1b[39mdefault\x1b[0m".as_bytes();
        let expected = ContrastGuard::new(&fallback_theme()).feed(bytes);
        assert!(String::from_utf8_lossy(&expected).contains("\x1b[31mred"));
        for split in 0..=bytes.len() {
            let mut guard = ContrastGuard::new(&fallback_theme());
            let mut actual = guard.feed(&bytes[..split]);
            actual.extend(guard.feed(&bytes[split..]));
            assert_eq!(actual, expected, "split {split}");
        }
    }

    #[test]
    fn history_cursor_save_restore_preserves_correction() {
        let mut guard = ContrastGuard::new(&fallback_theme());
        guard.feed(b"\x1b[40m");
        guard.feed(b"\x1b7\x1b[0mhistory\x1b8");
        assert_eq!(guard.feed(b"input\x1b[49m"), b"input\x1b[49m\x1b[39m");
    }

    #[test]
    fn dark_theme_light_composer_and_hidden_text_keep_attributes() {
        let mut theme = fallback_theme();
        theme.foreground = Rgb(230, 232, 235);
        theme.background = Rgb(11, 13, 16);
        let mut guard = ContrastGuard::new(&theme);
        assert_eq!(
            guard.feed(b"\x1b[48;5;231m\x1b[8msecret"),
            b"\x1b[48;5;231m\x1b[38;2;0;0;0m\x1b[8msecret"
        );
    }
}
