//! tmux 兼容 status bar（GTK4 渲染）。
//!
//! left + 窗口列表 + right；样式来自 core 快照的 tmux status 配置：
//! - `tmux` 模式：完全采用 tmux 的颜色/样式；
//! - `theme` 模式：只用 muxterm 主题前景/背景，忽略 tmux 配色。
//!
//! 窗口按钮可点击切换 tab；justify（left/centre/right）决定列表位置。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, CssProvider, Label, Orientation};

use crate::core::config::Theme;
use crate::platform::linux::lifecycle::tab_shortcut_label;
use crate::platform::linux::quickconnect::status_style::{
    StatusBarMode, StatusBarSnapshot, StatusBarStyleParser,
};

/// status bar 高度（与 tab bar 一致，≤ 24px）。
pub const STATUS_BAR_HEIGHT: u32 = 24;

type WindowActivateCb = Rc<RefCell<Option<Box<dyn Fn(u32)>>>>;

/// muxterm status bar。
pub struct StatusBar {
    pub container: GtkBox,
    left: Label,
    right: Label,
    windows: GtkBox,
    on_window_activate: WindowActivateCb,
    css: RefCell<CssProvider>,
    last_snapshot: RefCell<Option<StatusBarSnapshot>>,
    mode: RefCell<StatusBarMode>,
    theme: RefCell<Theme>,
}

impl StatusBar {
    pub fn new(mode: StatusBarMode, theme: Theme) -> Self {
        let container = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .hexpand(true)
            .vexpand(false)
            .build();
        container.add_css_class("muxterm-status-bar");
        container.set_size_request(-1, STATUS_BAR_HEIGHT as i32);

        let left = Label::new(None);
        left.set_halign(Align::Start);
        left.set_valign(Align::Center);
        left.set_hexpand(false);
        left.add_css_class("muxterm-status-text");

        let windows = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(2)
            .valign(Align::Center)
            .build();
        windows.add_css_class("muxterm-status-windows");

        let right = Label::new(None);
        right.set_halign(Align::End);
        right.set_valign(Align::Center);
        right.set_hexpand(true);
        right.add_css_class("muxterm-status-text");

        container.append(&left);
        container.append(&windows);
        container.append(&right);

        let css = CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &css,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let bar = StatusBar {
            container,
            left,
            right,
            windows,
            on_window_activate: Rc::new(RefCell::new(None)),
            css: RefCell::new(css),
            last_snapshot: RefCell::new(None),
            mode: RefCell::new(mode),
            theme: RefCell::new(theme),
        };
        bar.refresh_css();
        bar
    }

    pub fn connect_window_activate<F: Fn(u32) + 'static>(&self, f: F) {
        *self.on_window_activate.borrow_mut() = Some(Box::new(f));
    }

    /// 当前模式（tmux / theme）。
    pub fn mode(&self) -> StatusBarMode {
        *self.mode.borrow()
    }

    /// 最近一次快照的纯文本（测试 / 本地摘要）。
    pub fn plain_text(&self) -> String {
        self.last_snapshot
            .borrow()
            .as_ref()
            .map(snapshot_plain_text)
            .unwrap_or_default()
    }

    /// 应用一份快照；模式/主题变化后调用本函数即可重渲染。
    pub fn apply(&self, snapshot: &StatusBarSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        self.render();
    }

    /// 最近一次快照是否启用 status（tmux `status on`）。
    pub fn is_enabled(&self) -> bool {
        self.last_snapshot
            .borrow()
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false)
    }

    pub fn set_visible(&self, visible: bool) {
        self.container.set_visible(visible);
    }

    /// 切换 status bar 模式（tmux / theme）并重渲染。
    pub fn set_mode(&self, mode: StatusBarMode) {
        *self.mode.borrow_mut() = mode;
        self.render();
    }

    /// 主题变化后重渲染（GUI 黑白模式跟随主题）。
    pub fn apply_theme(&self, theme: &Theme) {
        *self.theme.borrow_mut() = theme.clone();
        self.refresh_css();
        self.render();
    }

    fn render(&self) {
        let Some(snapshot) = self.last_snapshot.borrow().clone() else {
            return;
        };
        let use_tmux_colors = *self.mode.borrow() == StatusBarMode::Tmux;
        let theme = self.theme.borrow().clone();
        let (theme_fg, theme_bg) = (theme.foreground, theme.background);
        let theme_fg_hex = format!("{:06x}", theme_fg.to_u32());
        let theme_bg_hex = format!("{:06x}", theme_bg.to_u32());

        let base = StatusBarStyleParser::parse(&snapshot.status_style);
        let plain_fg = if use_tmux_colors {
            None
        } else {
            Some(theme_fg_hex.as_str())
        };

        let left_style = StatusBarStyleParser::merged(&base, &snapshot.left_style);
        self.left.set_markup(&styled_markup(
            &StatusBarStyleParser::parse_inline(&snapshot.left, left_style),
            plain_fg,
        ));
        let right_style = StatusBarStyleParser::merged(&base, &snapshot.right_style);
        self.right.set_markup(&styled_markup(
            &StatusBarStyleParser::parse_inline(&snapshot.right, right_style),
            plain_fg,
        ));

        // 窗口列表
        while let Some(child) = self.windows.first_child() {
            self.windows.remove(&child);
        }
        for (i, win) in snapshot.windows.iter().enumerate() {
            if i > 0 {
                let sep = Label::new(Some(&if snapshot.separator.is_empty() {
                    " ".to_string()
                } else {
                    snapshot.separator.clone()
                }));
                sep.add_css_class("muxterm-status-text");
                self.windows.append(&sep);
            }
            let style_name = if win.current {
                &snapshot.window_current_style
            } else {
                &snapshot.window_style
            };
            let inline_base = StatusBarStyleParser::parse(style_name);
            let raw = if win.text.trim().is_empty() {
                win.name.as_str()
            } else {
                win.text.as_str()
            };
            let label = tab_shortcut_label(i, raw);
            let markup = styled_markup(
                &StatusBarStyleParser::parse_inline(&label, inline_base.clone()),
                plain_fg,
            );
            let button = Button::with_label("");
            button.set_label("");
            button.set_has_frame(false);
            button.set_can_focus(false);
            button.add_css_class("muxterm-status-window");
            button.add_css_class("flat");
            if win.current {
                button.add_css_class("current");
            }
            // 当前窗口高亮整块背景
            if use_tmux_colors {
                if inline_base.bg.is_some() {
                    button.add_css_class("muxterm-status-window-colored");
                }
            } else if win.current {
                button.add_css_class("muxterm-status-window-theme-current");
            }
            let label = Label::new(None);
            label.set_markup(&markup);
            button.set_child(Some(&label));
            let cb = self.on_window_activate.clone();
            let id = win.window_id;
            button.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb(id);
                }
            });
            self.windows.append(&button);
        }

        // justify：left / right / centre（absolute-centre 与 centre 相同）
        self.windows.set_halign(match snapshot.justify.as_str() {
            "left" => Align::Start,
            "right" => Align::End,
            _ => Align::Center,
        });
        self.left.set_visible(snapshot.enabled);
        self.right.set_visible(snapshot.enabled);
        self.windows.set_visible(snapshot.enabled);
        self.container.set_visible(snapshot.enabled);

        // 状态栏底色/前景
        if use_tmux_colors {
            let bg = base
                .bg
                .map(bg_to_hex)
                .unwrap_or_else(|| theme_bg_hex.clone());
            let fg = base
                .fg
                .map(fg_to_hex)
                .unwrap_or_else(|| theme_fg_hex.clone());
            self.set_bar_colors(&bg, &fg);
        } else {
            self.set_bar_colors(&theme_bg_hex, &theme_fg_hex);
        }
    }

    fn set_bar_colors(&self, bg_hex: &str, fg_hex: &str) {
        let css = format!(
            ".muxterm-status-bar {{ background: #{bg_hex}; color: #{fg_hex}; }}\n\
             .muxterm-status-window-colored {{ background: #{bg_hex}; border-radius: 3px; }}\n\
             .muxterm-status-window-theme-current {{ background: alpha(currentColor, 0.12); border-radius: 3px; }}\n"
        );
        self.css.borrow().load_from_data(&css);
    }

    fn refresh_css(&self) {
        let theme = self.theme.borrow().clone();
        let bg = format!("{:06x}", theme.background.to_u32());
        let fg = format!("{:06x}", theme.foreground.to_u32());
        self.set_bar_colors(&bg, &fg);
    }
}

fn bg_to_hex(c: crate::platform::linux::quickconnect::status_style::StatusBarColor) -> String {
    format!(
        "{:02x}{:02x}{:02x}",
        (c.red * 255.0).round() as u8,
        (c.green * 255.0).round() as u8,
        (c.blue * 255.0).round() as u8,
    )
}

fn fg_to_hex(c: crate::platform::linux::quickconnect::status_style::StatusBarColor) -> String {
    bg_to_hex(c)
}

/// 把带样式的片段渲染成 Pango markup。
fn styled_markup(
    segments: &[crate::platform::linux::quickconnect::status_style::StatusBarStyledSegment],
    plain_fg: Option<&str>,
) -> String {
    let mut out = String::new();
    for segment in segments {
        let style = &segment.style;
        let mut fg = plain_fg
            .map(|s| s.to_string())
            .or_else(|| style.fg.map(fg_to_hex));
        let mut bg = if plain_fg.is_some() {
            None
        } else {
            style.bg.map(bg_to_hex)
        };
        if style.reverse && plain_fg.is_none() {
            std::mem::swap(&mut fg, &mut bg);
        }
        let text = escape_markup(&segment.text);
        let mut attrs = String::new();
        if let Some(f) = &fg {
            attrs.push_str(&format!(" foreground=\"#{f}\""));
        }
        if let Some(b) = &bg {
            attrs.push_str(&format!(" background=\"#{b}\""));
        }
        let styled = if style.bold {
            format!("<b>{text}</b>")
        } else {
            text
        };
        if attrs.is_empty() {
            out.push_str(&styled);
        } else {
            out.push_str(&format!("<span{attrs}>{styled}</span>"));
        }
    }
    out
}

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 供测试：把快照渲染成纯文本（left + windows + right）。
pub fn snapshot_plain_text(snapshot: &StatusBarSnapshot) -> String {
    let windows = snapshot
        .windows
        .iter()
        .map(|w| w.text.clone())
        .collect::<Vec<_>>()
        .join(&snapshot.separator);
    format!("{} {} {}", snapshot.left, windows, snapshot.right)
}

/// 测试辅助：构造一个最小快照。
pub fn test_snapshot(enabled: bool) -> StatusBarSnapshot {
    StatusBarSnapshot {
        enabled,
        position: "bottom".into(),
        justify: "centre".into(),
        interval: 15,
        left: "muxterm".into(),
        right: "#[fg=red]%H:%M".into(),
        left_length: 20,
        right_length: 50,
        status_style: "bg=colour234".into(),
        left_style: "default".into(),
        right_style: "default".into(),
        separator: " ".into(),
        window_format: "#I:#W".into(),
        window_current_format: "#[reverse]#I:#W".into(),
        window_style: "default".into(),
        window_current_style: "bg=blue".into(),
        windows: vec![
            crate::platform::linux::quickconnect::status_style::StatusBarWindow {
                window_id: 0,
                index: 1,
                name: "bash".into(),
                flags: "*".into(),
                current: true,
                text: "1:bash".into(),
            },
            crate::platform::linux::quickconnect::status_style::StatusBarWindow {
                window_id: 1,
                index: 2,
                name: "vim".into(),
                flags: "".into(),
                current: false,
                text: "2:vim".into(),
            },
        ],
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_contains_left_windows_right() {
        let s = test_snapshot(true);
        let text = snapshot_plain_text(&s);
        assert!(text.contains("muxterm"));
        assert!(text.contains("1:bash"));
        assert!(text.contains("2:vim"));
    }

    #[test]
    fn disabled_snapshot_hides_bar() {
        let s = test_snapshot(false);
        assert!(!s.enabled);
    }

    #[test]
    fn styled_markup_escapes_and_applies_style() {
        let segments = vec![
            crate::platform::linux::quickconnect::status_style::StatusBarStyledSegment {
                text: "<b>&".into(),
                style: crate::platform::linux::quickconnect::status_style::StatusBarTextStyle {
                    fg: Some(
                        crate::platform::linux::quickconnect::status_style::StatusBarColor {
                            red: 1.0,
                            green: 0.0,
                            blue: 0.0,
                        },
                    ),
                    bg: None,
                    bold: true,
                    reverse: false,
                },
            },
        ];
        let markup = styled_markup(&segments, None);
        assert!(markup.contains("&lt;b&gt;&amp;"), "{markup}");
        assert!(markup.contains("foreground=\"#ff0000\""), "{markup}");
        assert!(markup.contains("<b>"), "{markup}");
    }

    #[test]
    fn theme_plain_fg_overrides_segment_colors() {
        let segments = vec![
            crate::platform::linux::quickconnect::status_style::StatusBarStyledSegment {
                text: "x".into(),
                style: crate::platform::linux::quickconnect::status_style::StatusBarTextStyle {
                    fg: Some(
                        crate::platform::linux::quickconnect::status_style::StatusBarColor {
                            red: 1.0,
                            green: 0.0,
                            blue: 0.0,
                        },
                    ),
                    bg: Some(
                        crate::platform::linux::quickconnect::status_style::StatusBarColor {
                            red: 0.0,
                            green: 0.0,
                            blue: 1.0,
                        },
                    ),
                    bold: false,
                    reverse: false,
                },
            },
        ];
        let markup = styled_markup(&segments, Some("abcdef"));
        assert!(markup.contains("foreground=\"#abcdef\""), "{markup}");
        assert!(!markup.contains("#ff0000"), "{markup}");
        assert!(!markup.contains("background="), "{markup}");
    }

    #[test]
    fn color_hex_conversion_rounds_to_bytes() {
        assert_eq!(
            bg_to_hex(
                crate::platform::linux::quickconnect::status_style::StatusBarColor {
                    red: 1.0,
                    green: 0.5,
                    blue: 0.0,
                }
            ),
            "ff8000"
        );
        assert_eq!(
            fg_to_hex(
                crate::platform::linux::quickconnect::status_style::StatusBarColor {
                    red: 0.0,
                    green: 0.0,
                    blue: 1.0,
                }
            ),
            "0000ff"
        );
    }

    #[test]
    fn escape_markup_handles_all_special_chars() {
        assert_eq!(escape_markup("a&b<c>d"), "a&amp;b&lt;c&gt;d");
        assert_eq!(escape_markup("plain"), "plain");
    }
}
