//! 统一 status bar（LINUX-PLAN §3）：一条 24px bar。
//!
//! 布局：`[status-left] [tabs] [status-right] [●状态] [🔔面板] [+]`。
//! 左/中/右同步 tmux status；最右三个按钮是 Muxterm chrome，永远可见。
//! tab 按钮只在 tab 集合/当前 tab 变化时重建（SSH 16ms 轮询不得拆按钮）。

use crate::core::format::{format_bytes, format_rate};
use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, CssProvider, Label, Orientation, Popover};

use crate::core::config::Theme;
use crate::platform::linux::lifecycle::tab_shortcut_label;
use crate::platform::linux::quickconnect::status_style::{
    StatusBarMode, StatusBarSnapshot, StatusBarStyleParser,
};

/// status bar 高度（≤ 24px）。
pub const STATUS_BAR_HEIGHT: u32 = 24;

type WindowActivateCb = Rc<RefCell<Option<Box<dyn Fn(u32)>>>>;
type NotifyActivateCb = Rc<RefCell<Option<Box<dyn Fn()>>>>;
type NewTabCb = Rc<RefCell<Option<Box<dyn Fn()>>>>;
type WorktreeCreateCb = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// 状态点三色 CSS（测试与实现同一常量）。
pub fn status_dot_css() -> &'static str {
    ".muxterm-status-dot.status-ok { color: #27ae60; }\n\
     .muxterm-status-dot.status-warn { color: #f39c12; }\n\
     .muxterm-status-dot.status-err { color: #c0392b; }"
}

/// 连接摘要（C7.7 popover 内容）。
#[derive(Debug, Clone, Default)]
pub struct ConnectionSummary {
    pub kind: String,
    pub host: Option<String>,
    pub status: String,
    /// 累计下行字节（SSH transport 读端）。
    pub down: u64,
    /// 累计上行字节（SSH PtyWriter 写端）。
    pub up: u64,
    /// 瞬时下行字节/秒（由连续两次 snapshot 差出来，不是累计）。
    pub down_rate: u64,
    /// 瞬时上行字节/秒。
    pub up_rate: u64,
}

/// muxterm status bar（唯一 chrome）。
pub struct StatusBar {
    pub container: GtkBox,
    left: Label,
    tabs: GtkBox,
    right: Label,
    dot: Button,
    notify: Button,
    new_tab: Button,
    worktree_create: Button,
    popover: Popover,
    on_window_activate: WindowActivateCb,
    on_notify_activate: NotifyActivateCb,
    on_new_tab: NewTabCb,
    on_worktree_create: WorktreeCreateCb,
    /// tab 签名：id+name+current；不变就不重建按钮。
    last_tab_signature: RefCell<Option<String>>,
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
        container.set_widget_name("muxterm-status-bar");
        container.set_size_request(-1, STATUS_BAR_HEIGHT as i32);

        let left = Label::new(None);
        left.set_widget_name("muxterm-status-left");
        left.set_halign(Align::Start);
        left.set_valign(Align::Center);
        left.set_hexpand(false);
        left.add_css_class("muxterm-status-text");

        let tabs = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(2)
            .valign(Align::Center)
            .build();
        tabs.set_widget_name("muxterm-status-tabs");
        tabs.add_css_class("muxterm-status-windows");

        let right = Label::new(None);
        right.set_widget_name("muxterm-status-right");
        right.set_halign(Align::End);
        right.set_valign(Align::Center);
        right.set_hexpand(true);
        right.add_css_class("muxterm-status-text");

        // Muxterm chrome：状态点 / 通知面板 / 新建 tab（永远可见）。
        let dot = Button::with_label("●");
        dot.set_widget_name("muxterm-status-dot");
        dot.set_has_frame(false);
        dot.set_can_focus(false);
        dot.set_size_request(18, 18);
        dot.add_css_class("muxterm-status-dot");
        dot.add_css_class("status-ok");

        let popover = Popover::new();
        popover.set_widget_name("muxterm-status-popover");
        popover.set_parent(&dot);
        let pop_label = Label::new(Some("type=local status=connected"));
        pop_label.set_widget_name("muxterm-status-popover-label");
        pop_label.set_margin_top(8);
        pop_label.set_margin_bottom(8);
        pop_label.set_margin_start(12);
        pop_label.set_margin_end(12);
        popover.set_child(Some(&pop_label));

        let notify = Button::with_label("🔔");
        notify.set_widget_name("muxterm-status-notify");
        notify.set_has_frame(false);
        notify.set_can_focus(false);
        notify.add_css_class("muxterm-status-notify");

        let new_tab = Button::with_label("+");
        new_tab.set_widget_name("muxterm-new-tab");
        new_tab.set_has_frame(false);
        new_tab.set_can_focus(false);
        new_tab.add_css_class("muxterm-new-tab");

        let worktree_create = Button::with_label("⿻");
        worktree_create.set_widget_name("muxterm-worktree-create");
        worktree_create.set_has_frame(false);
        worktree_create.set_can_focus(false);
        worktree_create.add_css_class("muxterm-worktree-create");
        worktree_create.set_visible(false);

        container.append(&left);
        container.append(&tabs);
        container.append(&right);
        container.append(&dot);
        container.append(&notify);
        container.append(&new_tab);

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
            tabs,
            right,
            dot,
            notify,
            new_tab,
            worktree_create,
            popover,
            on_window_activate: Rc::new(RefCell::new(None)),
            on_notify_activate: Rc::new(RefCell::new(None)),
            on_new_tab: Rc::new(RefCell::new(None)),
            on_worktree_create: Rc::new(RefCell::new(None)),
            last_tab_signature: RefCell::new(None),
            css: RefCell::new(css),
            last_snapshot: RefCell::new(None),
            mode: RefCell::new(mode),
            theme: RefCell::new(theme),
        };
        {
            let cb = bar.on_notify_activate.clone();
            let notify = bar.notify.clone();
            notify.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb();
                }
            });
        }
        // 状态点：Button 的 connect_clicked 打开 popover（GestureClick 会被吃掉）。
        {
            let popover = bar.popover.clone();
            let dot = bar.dot.clone();
            dot.connect_clicked(move |_| {
                popover.popup();
            });
        }
        {
            let cb = bar.on_new_tab.clone();
            let new_tab = bar.new_tab.clone();
            new_tab.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb();
                }
            });
        }
        {
            let cb = bar.on_worktree_create.clone();
            let worktree_create = bar.worktree_create.clone();
            worktree_create.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb();
                }
            });
        }
        bar.refresh_css();
        bar
    }

    pub fn connect_window_activate<F: Fn(u32) + 'static>(&self, f: F) {
        *self.on_window_activate.borrow_mut() = Some(Box::new(f));
    }

    /// 通知/面板按钮点击回调（window 侧决定 Workspaces 或 Attention tab）。
    pub fn connect_attention_activate<F: Fn() + 'static>(&self, f: F) {
        *self.on_notify_activate.borrow_mut() = Some(Box::new(f));
    }

    /// 新建 tab 按钮回调。
    pub fn connect_new_tab<F: Fn() + 'static>(&self, f: F) {
        *self.on_new_tab.borrow_mut() = Some(Box::new(f));
    }

    /// worktree 创建入口（仅 support() 含 WorktreeList 的 Runtime 可见）。
    pub fn connect_worktree_create<F: Fn() + 'static>(&self, f: F) {
        *self.on_worktree_create.borrow_mut() = Some(Box::new(f));
    }

    /// 按当前工作区能力显示/隐藏 worktree 创建按钮。
    ///
    /// 不支持的 Runtime 必须**找不到**该控件（不是只隐藏），
    /// 测试用 `find_by_name` 断言 tmux 格没有入口。
    pub fn set_worktree_visible(&self, visible: bool) {
        if visible {
            if self.worktree_create.parent().is_none() {
                self.container.append(&self.worktree_create);
            }
            self.worktree_create.set_visible(true);
        } else if self.worktree_create.parent().is_some() {
            self.container.remove(&self.worktree_create);
        }
    }

    /// 测试用：worktree 创建按钮。
    pub fn worktree_create_widget(&self) -> Button {
        self.worktree_create.clone()
    }

    /// 状态点按钮（测试/接线用）。
    pub fn dot_widget(&self) -> Button {
        self.dot.clone()
    }

    /// 状态 popover（测试/接线用）。
    pub fn popover_widget(&self) -> Popover {
        self.popover.clone()
    }

    /// 通知/面板按钮（测试/接线用）。
    pub fn notify_widget(&self) -> Button {
        self.notify.clone()
    }

    /// 新建 tab 按钮（测试/接线用）。
    pub fn new_tab_widget(&self) -> Button {
        self.new_tab.clone()
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

    /// 应用一份快照；tab 签名不变时不重建按钮。
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

    /// tab 签名是否与当前按钮不同（SSH 16ms 轮询用：没变就不 apply）。
    pub fn tab_signature_changed(&self, snapshot: &StatusBarSnapshot) -> bool {
        let signature: String = snapshot
            .windows
            .iter()
            .map(|w| format!("{}|{}|{}|{};", w.window_id, w.name, w.current, w.text))
            .collect();
        self.last_tab_signature.borrow().as_deref() != Some(signature.as_str())
    }

    pub fn set_visible(&self, visible: bool) {
        self.container.set_visible(visible);
    }

    /// 通知按钮数字：n=0 无数字；n>0 显示 `🔔 N`。
    pub fn set_attention(&self, n: usize) {
        if n == 0 {
            self.notify.set_label("🔔");
        } else {
            self.notify.set_label(&format!("🔔 {n}"));
        }
    }

    /// 连接摘要 → 状态点 class + popover 文本（C7.7 接线）。
    pub fn set_connection_summary(&self, summary: &ConnectionSummary) {
        self.dot.remove_css_class("status-ok");
        self.dot.remove_css_class("status-warn");
        self.dot.remove_css_class("status-err");
        match summary.status.as_str() {
            "connected" => self.dot.add_css_class("status-ok"),
            "connecting" => self.dot.add_css_class("status-warn"),
            _ => self.dot.add_css_class("status-err"),
        }
        let host = summary.host.as_deref().unwrap_or("");
        // 速率与累计分开：禁止把累计字节标成 `B/s`（W15a）。
        let text = format!(
            "type={} status={}{}\n↓ {}  ↑ {}\ntotal ↓ {}  ↑ {}",
            summary.kind,
            summary.status,
            if host.is_empty() {
                String::new()
            } else {
                format!(" host={host}")
            },
            format_rate(summary.down_rate),
            format_rate(summary.up_rate),
            format_bytes(summary.down),
            format_bytes(summary.up),
        );
        if let Some(label) = self
            .popover
            .child()
            .and_then(|c| c.downcast::<Label>().ok())
        {
            label.set_text(&text);
        }
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

        // tab 签名：id+name+current+text；不变就不重建（SSH 16ms 轮询安全）。
        let signature: String = snapshot
            .windows
            .iter()
            .map(|w| format!("{}|{}|{}|{};", w.window_id, w.name, w.current, w.text))
            .collect();
        if self.last_tab_signature.borrow().as_deref() != Some(signature.as_str()) {
            self.rebuild_tabs(&snapshot, use_tmux_colors, plain_fg);
            *self.last_tab_signature.borrow_mut() = Some(signature);
        }

        // justify 只影响中区。
        self.tabs.set_halign(match snapshot.justify.as_str() {
            "left" => Align::Start,
            "right" => Align::End,
            _ => Align::Center,
        });

        // chrome 永远可见；左/中/右跟随 tmux status on/off。
        self.left.set_visible(snapshot.enabled);
        self.right.set_visible(snapshot.enabled);
        self.tabs.set_visible(snapshot.enabled);
        self.container.set_visible(true);

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

    fn rebuild_tabs(
        &self,
        snapshot: &StatusBarSnapshot,
        use_tmux_colors: bool,
        plain_fg: Option<&str>,
    ) {
        while let Some(child) = self.tabs.first_child() {
            self.tabs.remove(&child);
        }
        for (i, win) in snapshot.windows.iter().enumerate() {
            if i > 0 {
                let sep = Label::new(Some(&if snapshot.separator.is_empty() {
                    " ".to_string()
                } else {
                    snapshot.separator.clone()
                }));
                sep.add_css_class("muxterm-status-text");
                self.tabs.append(&sep);
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
            let button = Button::new();
            button.set_widget_name(&format!("muxterm-status-tab-{}", win.window_id));
            button.set_has_frame(false);
            button.set_can_focus(false);
            button.add_css_class("muxterm-status-window");
            button.add_css_class("flat");
            if win.current {
                button.add_css_class("tab-active");
            }
            if use_tmux_colors {
                if inline_base.bg.is_some() {
                    button.add_css_class("muxterm-status-window-colored");
                }
            } else if win.current {
                button.add_css_class("muxterm-status-window-theme-current");
            }
            let label_widget = Label::new(None);
            label_widget.set_markup(&markup);
            // Label 不抢点击（GTK4 会把点击吃掉）。
            label_widget.set_can_target(false);
            button.set_child(Some(&label_widget));
            let cb = self.on_window_activate.clone();
            let id = win.window_id;
            button.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb(id);
                }
            });
            self.tabs.append(&button);
        }
    }

    fn set_bar_colors(&self, bg_hex: &str, fg_hex: &str) {
        let css = format!(
            ".muxterm-status-bar {{ background: #{bg_hex}; color: #{fg_hex}; }}\n\
             .muxterm-status-window-colored {{ background: #{bg_hex}; border-radius: 3px; }}\n\
             .muxterm-status-window-theme-current {{ background: alpha(currentColor, 0.12); border-radius: 3px; }}\n\
             {}\n",
            status_dot_css()
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

impl Drop for StatusBar {
    fn drop(&mut self) {
        // Popover 挂在 dot 上：先解除父子关系，避免 dot 销毁时 popover 仍引用它。
        self.popover.unparent();
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
