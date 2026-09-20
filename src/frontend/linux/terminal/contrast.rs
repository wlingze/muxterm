//! 保护与当前底色不兼容的前景，不改变应用背景、色板或 Core 数据。
//! 只跟踪 SGR/保存恢复属性，不保存屏幕；在完整控制序列边界插入前景修正，
//! 避免截断跨分包 UTF-8、OSC 和 CSI。Braille 装饰保留应用的渐变亮度。

use vte::ansi::{Attr, Color, Handler, NamedColor, Processor, Timeout};

use crate::frontend::linux::theme::{indexed_color, Rgb, Theme};

/// 这里是字节流属性过滤器，不是负责提交帧的终端。必须在每条 SGR 的
/// 原始边界处理属性；默认 Processor 会把 DEC 2026 帧缓存到结束才回调，
/// 此时修正已来不及作用于前面传给 VTE 的文字。同步帧标记仍原样交给 VTE。
#[derive(Default)]
struct ImmediateAttributes;

impl Timeout for ImmediateAttributes {
    fn set_timeout(&mut self, _: std::time::Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy)]
struct Style {
    fg: Color,
    bg: Color,
    reverse: bool,
    correction: Option<Rgb>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            reverse: false,
            correction: None,
        }
    }
}

pub(super) struct ContrastGuard {
    processor: Processor<ImmediateAttributes>,
    style: Style,
    saved: Style,
    changed: bool,
    theme: Theme,
    printed: Option<char>,
    utf8: Vec<u8>,
}

impl ContrastGuard {
    pub(super) fn new(theme: &Theme) -> Self {
        Self {
            processor: Processor::default(),
            style: Style::default(),
            saved: Style::default(),
            changed: false,
            theme: theme.clone(),
            printed: None,
            utf8: Vec::new(),
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
            // 最多暂存一个 UTF-8 字符；不能在跨包字符的中间插入 SGR。
            if byte < 0x80 && !self.utf8.is_empty() {
                out.append(&mut self.utf8);
            }
            processor.advance(self, byte);
            if byte >= 0x80 {
                self.utf8.push(byte);
                if let Some(c) = self.printed.take() {
                    let decorative = ('\u{2800}'..='\u{28ff}').contains(&c);
                    if decorative && self.style.correction.is_some() {
                        self.write_original_foreground(&mut out);
                    }
                    out.append(&mut self.utf8);
                    if decorative {
                        if let Some(rgb) = self.style.correction {
                            write_rgb(&mut out, rgb);
                        }
                    }
                } else if self.utf8.len() >= 4 {
                    out.append(&mut self.utf8);
                }
            } else {
                self.printed = None;
                out.push(byte);
            }
            if std::mem::take(&mut self.changed) {
                self.correct(&mut out);
            }
        }
        self.processor = processor;
        out
    }

    fn resolve(&self, color: Color) -> Rgb {
        let index = match color {
            Color::Named(NamedColor::Background) => return self.theme.background,
            Color::Named(NamedColor::Foreground) => return self.theme.foreground,
            Color::Spec(rgb) => return Rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => index as usize,
            Color::Named(named) => named as usize,
        };
        if index < 16 {
            self.theme.colors[index]
        } else if index < 256 {
            indexed_color(index as u32)
        } else {
            self.theme.foreground
        }
    }

    fn correct(&mut self, out: &mut Vec<u8>) {
        // 反色时逻辑前景是实际背景；修正它会给 htop 选中行/色条重新涂底。
        let correction = if self.style.reverse {
            None
        } else {
            readable_foreground(self.resolve(self.style.fg), self.resolve(self.style.bg))
        };
        if correction == self.style.correction {
            return;
        }
        match correction {
            Some(rgb) => write_rgb(out, rgb),
            None => self.write_original_foreground(out),
        }
        self.style.correction = correction;
    }

    fn write_original_foreground(&self, out: &mut Vec<u8>) {
        if self.style.fg == Color::Named(NamedColor::Foreground) {
            out.extend_from_slice(b"\x1b[39m");
        } else {
            write_rgb(out, self.resolve(self.style.fg));
        }
    }
}

fn write_rgb(out: &mut Vec<u8>, Rgb(r, g, b): Rgb) {
    out.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m").as_bytes());
}

fn readable_foreground(fg: Rgb, bg: Rgb) -> Option<Rgb> {
    let background = luminance(bg);
    let ratio = |color| {
        let foreground = luminance(color);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    };
    // 只救几乎不可见的文字；不要把应用本来可读的强调/弱化色全部改掉。
    if ratio(fg) >= 2.0 {
        return None;
    }
    let target = if background < 0.179 { 255.0 } else { 0.0 };
    // 只调整到可读阈值，保留色相，避免一律纯白/纯黑带来的刺眼对比。
    for step in 1..=32 {
        let blend =
            |c: u8| (f64::from(c) + (target - f64::from(c)) * f64::from(step) / 32.0).round() as u8;
        let candidate = Rgb(blend(fg.0), blend(fg.1), blend(fg.2));
        if ratio(candidate) >= 4.5 {
            return Some(candidate);
        }
    }
    Some(Rgb(target as u8, target as u8, target as u8))
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
    fn input(&mut self, c: char) {
        self.printed = Some(c);
    }
    fn terminal_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::Reset => self.style = Style::default(),
            Attr::Foreground(color) => {
                self.style.fg = color;
                // 原始 SGR 已覆盖上次注入的前景。
                self.style.correction = None;
            }
            Attr::Background(color) => self.style.bg = color,
            Attr::Reverse => self.style.reverse = true,
            Attr::CancelReverse => self.style.reverse = false,
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
    fn reverse_video_keeps_application_background_and_restores_correction() {
        let mut guard = ContrastGuard::new(&fallback_theme());
        guard.feed(b"\x1b[38;2;248;248;180m");
        let reversed = guard.feed(b"\x1b[7mHTOP");
        assert_eq!(reversed, b"\x1b[7m\x1b[38;2;248;248;180mHTOP");
        let normal = guard.feed(b"\x1b[27mTEXT");
        assert_eq!(normal, b"\x1b[27m\x1b[38;2;116;116;84mTEXT");
    }

    #[test]
    fn synchronized_codex_frame_corrects_colors_before_text_not_after_frame() {
        let body = "\x1b[0;38;2;248;248;180mCODE\x1b[0;48;2;31;31;31mINPUT中文⠁\x1b[0m";
        let mut expected = b"\x1b[?2026h".to_vec();
        expected.extend(ContrastGuard::new(&fallback_theme()).feed(body.as_bytes()));
        expected.extend_from_slice(b"\x1b[?2026l");
        let frame = format!("\x1b[?2026h{body}\x1b[?2026l");
        for split in 0..=frame.len() {
            let mut guard = ContrastGuard::new(&fallback_theme());
            let mut actual = guard.feed(&frame.as_bytes()[..split]);
            actual.extend(guard.feed(&frame.as_bytes()[split..]));
            assert_eq!(actual, expected, "synchronized frame split {split}");
        }
    }

    #[test]
    fn codex_black_composer_keeps_input_visible_and_restores_default() {
        let mut guard = ContrastGuard::new(&fallback_theme());
        let mut expected = b"\x1b[48;2;20;20;20m".to_vec();
        write_rgb(
            &mut expected,
            readable_foreground(fallback_theme().foreground, Rgb(20, 20, 20)).unwrap(),
        );
        expected.extend_from_slice(b"INPUT\x1b[49m\x1b[39mnormal");
        assert_eq!(
            guard.feed(b"\x1b[48;2;20;20;20mINPUT\x1b[49mnormal"),
            expected
        );
        assert_eq!(
            guard.feed(b"\x1b[0;48;2;216;216;216mLIGHT"),
            b"\x1b[0;48;2;216;216;216mLIGHT"
        );
    }

    #[test]
    fn fragmented_sequences_are_identical_and_readable_colors_are_preserved() {
        let readable = b"\x1b[0;38;2;12;123;234mREADABLE";
        assert_eq!(
            ContrastGuard::new(&fallback_theme()).feed(readable),
            readable
        );
        let bytes =
            "\x1b]0;title\x07\x1b[48:2::20:20:20m中文⠁\x1b[31mred\x1b[39mdefault\x1b[0m".as_bytes();
        let expected = ContrastGuard::new(&fallback_theme()).feed(bytes);
        assert!(String::from_utf8_lossy(&expected).contains("red"));
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
        let mut expected = b"\x1b[48;5;231m".to_vec();
        write_rgb(
            &mut expected,
            readable_foreground(theme.foreground, Rgb(255, 255, 255)).unwrap(),
        );
        expected.extend_from_slice(b"\x1b[8msecret");
        assert_eq!(guard.feed(b"\x1b[48;5;231m\x1b[8msecret"), expected);
    }

    #[test]
    fn explicit_dark_input_and_light_syntax_are_readable_without_extreme_colors() {
        for (fg, bg) in [
            (Rgb(20, 20, 20), Rgb(31, 31, 31)),
            (Rgb(248, 248, 242), Rgb(255, 255, 255)),
        ] {
            let fixed = readable_foreground(fg, bg).unwrap();
            let a = luminance(fixed);
            let b = luminance(bg);
            assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
            assert_ne!(fixed, Rgb(0, 0, 0));
            assert_ne!(fixed, Rgb(255, 255, 255));
        }
        let mut guard = ContrastGuard::new(&fallback_theme());
        let output = guard.feed(b"\x1b[48;2;31;31;31m\x1b[38;2;20;20;20mINPUT");
        assert!(String::from_utf8_lossy(&output).contains("20;20;20m\x1b[38;2;"));
    }

    #[test]
    fn sparkle_brightness_and_utf8_survive_every_packet_boundary() {
        let bytes =
            "\x1b]0;中文标题\x07\x1b[48;2;31;31;31m\x1b[38;2;40;40;40m⠁⢀文字\x1b[0m".as_bytes();
        let expected = ContrastGuard::new(&fallback_theme()).feed(bytes);
        assert!(String::from_utf8_lossy(&expected).contains("\x1b[38;2;40;40;40m⠁"));
        for split in 0..=bytes.len() {
            let mut guard = ContrastGuard::new(&fallback_theme());
            let mut actual = guard.feed(&bytes[..split]);
            actual.extend(guard.feed(&bytes[split..]));
            assert_eq!(actual, expected, "split {split}");
        }
    }
}
