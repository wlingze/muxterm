//! Pane 视图：VTE4 渲染 + FFI 输入/输出。
//!
//! 与 macOS TerminalManager/TerminalView 对齐：
//! - 新视图先用后端报告的 pane 字符格尺寸 resize 模型，再喂快照；
//! - 输出按 pane 合并后 feed（短窗口），避免 agent 帧被逐事件拆开；
//! - tmux/SSH 镜像模式丢弃 feed 期间解析器生成的查询应答（tmux 自己代答）；
//! - 主题 / 字号可在运行期切换。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use vte4::prelude::*;

use crate::core::config::{Rgb, Theme};
use crate::core::protocol::terminal::emulate::TerminalState;
use crate::platform::linux::quickconnect::font::FontSettings;
use crate::platform::linux::renderer::{TerminalRenderer, VteRenderer};

/// 同一 pane 输出合并后刷新的窗口（毫秒）。
pub const FEED_COALESCE_MS: u64 = 25;

/// 一个 pane 的 GTK 视图（薄包装；内部用 Rc 供合并定时器弱引用）。
pub struct PaneView {
    inner: Rc<PaneViewInner>,
}

struct PaneViewInner {
    renderer: VteRenderer,
    pane_id: Cell<u32>,
    /// 无头终端状态：捕获 OSC 10/11/12、CSI DA 等查询并生成应答。
    reply_state: RefCell<TerminalState>,
    /// 已生成、待回写 shell 的应答字节。
    pending_replies: RefCell<Vec<u8>>,
    /// tmux/SSH 镜像模式：feed 期间解析器应答一律丢弃。
    is_tmux_mirror: Cell<bool>,
    /// 待合并的输出。
    pending_feed: RefCell<Vec<u8>>,
    feed_flush_source: RefCell<Option<glib::SourceId>>,
}

impl PaneView {
    pub fn new(pane_id: u32, theme: &Theme, font: &FontSettings, is_tmux_mirror: bool) -> Self {
        let renderer = VteRenderer::new();
        renderer.apply_theme(theme);
        renderer.apply_font(font);
        PaneView {
            inner: Rc::new(PaneViewInner {
                renderer,
                pane_id: Cell::new(pane_id),
                reply_state: RefCell::new(TerminalState::new(80, 24)),
                pending_replies: RefCell::new(Vec::new()),
                is_tmux_mirror: Cell::new(is_tmux_mirror),
                pending_feed: RefCell::new(Vec::new()),
                feed_flush_source: RefCell::new(None),
            }),
        }
    }

    pub fn pane_id(&self) -> u32 {
        self.inner.pane_id.get()
    }

    pub fn set_pane_id(&self, id: u32) {
        self.inner.pane_id.set(id);
    }

    pub fn widget(&self) -> gtk4::Widget {
        self.inner.renderer.widget()
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        self.inner.renderer.terminal()
    }

    /// 按后端报告的 pane 字符格尺寸 resize 模型。
    pub fn resize_to(&self, cols: u16, rows: u16) {
        let cols = cols.max(2) as i64;
        let rows = rows.max(1) as i64;
        self.inner.renderer.terminal().set_size(cols, rows);
        self.inner
            .reply_state
            .borrow_mut()
            .resize(cols as usize, rows as usize);
    }

    /// 输出事件入队合并（同一 pane 短窗口内一次 feed）。
    pub fn feed_output(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.inner.pending_feed.borrow_mut().extend_from_slice(data);
        if self.inner.feed_flush_source.borrow().is_none() {
            let weak = Rc::downgrade(&self.inner);
            let id = glib::timeout_add_local(
                std::time::Duration::from_millis(FEED_COALESCE_MS),
                move || {
                    if let Some(inner) = weak.upgrade() {
                        flush_pending_feed(&inner);
                    }
                    glib::ControlFlow::Break
                },
            );
            *self.inner.feed_flush_source.borrow_mut() = Some(id);
        }
    }

    /// 立即把合并缓冲写入 VTE（测试 / 窗口关闭用）。
    pub fn flush_pending_feed(&self) {
        flush_pending_feed(&self.inner);
    }

    /// 用完整快照播种（首次挂载 pane 时）。播种前先按 pane 尺寸 resize。
    pub fn seed_snapshot(&self, data: &[u8], cols: u16, rows: u16) {
        if cols >= 2 || rows >= 1 {
            self.resize_to(cols, rows);
        }
        if data.is_empty() {
            return;
        }
        self.inner.renderer.terminal().reset(true, true);
        *self.inner.reply_state.borrow_mut() =
            TerminalState::new(cols.max(2) as usize, rows.max(1) as usize);
        self.inner.renderer.terminal().feed(data);
        feed_reply_state(&self.inner, data);
    }

    /// 取出待回写 shell 的查询应答字节。
    pub fn take_replies(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner.pending_replies.borrow_mut())
    }

    /// 用户按键 → 回调（由 window 转发到 FFI send_input）。
    pub fn connect_input<F: Fn(u32, &[u8]) + 'static>(&self, f: F) {
        let pid = self.inner.pane_id.clone();
        self.inner
            .renderer
            .terminal()
            .connect_commit(move |_term, text, _len| {
                f(pid.get(), text.as_bytes());
            });
    }

    /// 运行期切换主题（VTE 调色板 + 重绘）。
    pub fn apply_theme(&self, theme: &Theme) {
        self.inner.renderer.apply_theme(theme);
    }

    /// 运行期修改字号（保留 family；VTE 重算字符格）。
    pub fn set_font_size(&self, size: f32) {
        let settings = FontSettings {
            size,
            ..FontSettings::default()
        };
        self.inner.renderer.apply_font(&settings);
    }

    /// 运行期修改字体（family + size）。
    pub fn set_font(&self, font: &FontSettings) {
        self.inner.renderer.apply_font(font);
    }

    pub fn grab_focus(&self) {
        self.inner.renderer.terminal().grab_focus();
    }

    /// 测试用：VTE 当前可见/滚动缓冲纯文本。
    pub fn visible_text(&self) -> String {
        self.inner
            .renderer
            .terminal()
            .text_format(vte4::Format::Text)
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

fn flush_pending_feed(inner: &PaneViewInner) {
    *inner.feed_flush_source.borrow_mut() = None;
    let data = std::mem::take(&mut *inner.pending_feed.borrow_mut());
    if data.is_empty() {
        return;
    }
    inner.renderer.terminal().feed(&data);
    feed_reply_state(inner, &data);
}

fn feed_reply_state(inner: &PaneViewInner, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut state = inner.reply_state.borrow_mut();
    state.feed(data);
    let replies = state.take_reply();
    if !replies.is_empty() && !inner.is_tmux_mirror.get() {
        inner
            .pending_replies
            .borrow_mut()
            .extend_from_slice(&replies);
    }
}

/// 便于在闭包里共享的 PaneView 句柄。
pub type PaneViewRc = Rc<PaneView>;

/// 主题色转 hex（`rrggbb`，供 tmux refresh-client -r 上报）。
pub fn rgb_hex(c: Rgb) -> String {
    format!("{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}
